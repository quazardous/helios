//! GUI prototype in egui (eframe, wgpu): a float viewport drawn by a wgpu
//! shader, dockable and detachable panels (egui_dock, plus native windows
//! through egui viewports), the layout saved and restored, a 200-node graph.
//!
//!     helios-proto-egui [--image FILE] [--layout FILE] [--bench FRAMES]
//!
//! `--bench` drives the app for a number of frames (changing the exposure
//! every 10 frames), prints the measures as JSON and quits.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32, Rect, Sense, Stroke, Vec2};
use eframe::egui_wgpu::{self, wgpu};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use helios_proto_scene::{Frame, Placed, Worker, big_graph};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Tab {
    Viewport,
    Graph,
    Params,
    Stats,
}

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Tab::Viewport => "Viewport",
            Tab::Graph => "Graph",
            Tab::Params => "Params",
            Tab::Stats => "Stats",
        }
    }
}

fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let tree = dock.main_surface_mut();
    let [view, _] = tree.split_right(NodeIndex::root(), 0.75, vec![Tab::Params, Tab::Stats]);
    tree.split_below(view, 0.65, vec![Tab::Graph]);
    dock
}

/// What the measures collect.
#[derive(Default, Serialize)]
struct Measures {
    frames: u64,
    frame_ms: Vec<f64>,
    graph_ms: Vec<f64>,
    upload_ms: Vec<f64>,
    proxy_ms: Vec<f64>,
    exact_ms: Vec<f64>,
}

fn summary(v: &[f64]) -> serde_json::Value {
    if v.is_empty() {
        return serde_json::Value::Null;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    serde_json::json!({
        "n": s.len(), "mean": (mean * 1000.0).round() / 1000.0,
        "p50": s[s.len() / 2], "p95": s[(s.len() * 95 / 100).min(s.len() - 1)], "max": s[s.len() - 1],
    })
}

/// GPU side of the viewport, kept in egui-wgpu's callback resources.
struct ViewportGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    image: Option<(wgpu::BindGroup, u64)>,
    encode: bool,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    origin: [f32; 2],
    pan: [f32; 2],
    zoom: f32,
    encode: f32,
    size: [f32; 2],
}

impl ViewportGpu {
    fn new(rs: &egui_wgpu::RenderState) -> Self {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shared/viewport.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: rs.target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ViewportGpu {
            pipeline,
            layout,
            uniform,
            image: None,
            encode: !rs.target_format.is_srgb(),
        }
    }
}

/// One paint of the viewport: a new image to upload (by generation), and
/// where to draw.
struct ViewportPaint {
    frame: Option<(Frame, u64)>,
    rect: Rect,
    pan: Vec2,
    zoom: f32,
    upload_ms: Arc<Mutex<Vec<f64>>>,
}

impl egui_wgpu::CallbackTrait for ViewportPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let gpu: &mut ViewportGpu = resources.get_mut().expect("viewport resources");
        if let Some((frame, generation)) = &self.frame
            && gpu.image.as_ref().is_none_or(|(_, g)| g != generation)
        {
            let start = Instant::now();
            let r = frame.image.region();
            let size = wgpu::Extent3d {
                width: r.width,
                height: r.height,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("image"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(frame.image.data()),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(r.width * 16),
                    rows_per_image: Some(r.height),
                },
                size,
            );
            let view = texture.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("image"),
                layout: &gpu.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: gpu.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                ],
            });
            gpu.image = Some((group, *generation));
            self.upload_ms
                .lock()
                .unwrap()
                .push(start.elapsed().as_secs_f64() * 1000.0);
        }
        if let Some((frame, _)) = &self.frame {
            let ppp = screen.pixels_per_point;
            // Proxies are a quarter of the size: drawn four times larger.
            let scale = if frame.proxy { 4.0 } else { 1.0 };
            let r = frame.image.region();
            let uniform = ViewUniform {
                origin: [self.rect.min.x * ppp, self.rect.min.y * ppp],
                pan: [self.pan.x * ppp, self.pan.y * ppp],
                zoom: self.zoom * ppp * scale,
                encode: if gpu.encode { 1.0 } else { 0.0 },
                size: [r.width as f32, r.height as f32],
            };
            queue.write_buffer(&gpu.uniform, 0, bytemuck::bytes_of(&uniform));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let gpu: &ViewportGpu = resources.get().expect("viewport resources");
        let Some((group, _)) = &gpu.image else { return };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

struct App {
    dock: DockState<Tab>,
    layout_file: PathBuf,
    worker: Worker,
    latest: Arc<Mutex<Option<(Frame, u64)>>>,
    ev: f64,
    pan: Vec2,
    zoom: f32,
    graph: (Vec<Placed>, Vec<(usize, usize)>),
    graph_pan: Vec2,
    popped: Vec<Tab>,
    measures: Measures,
    upload_ms: Arc<Mutex<Vec<f64>>>,
    bench: Option<u64>,
    last_frame: Instant,
    seen: u64,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Result<Self, String> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("this prototype needs the wgpu renderer")?;
        rs.renderer
            .write()
            .callback_resources
            .insert(ViewportGpu::new(rs));
        let image = helios_proto_scene::image(args.image.as_deref())?;
        let latest: Arc<Mutex<Option<(Frame, u64)>>> = Arc::default();
        let (sink, ctx) = (latest.clone(), cc.egui_ctx.clone());
        let worker = Worker::spawn(image, move |frame| {
            let mut slot = sink.lock().unwrap();
            let generation = slot.as_ref().map_or(1, |(_, g)| g + 1);
            *slot = Some((frame, generation));
            ctx.request_repaint();
        });
        worker.request(0.0);
        let dock = std::fs::read_to_string(&args.layout)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(default_layout);
        Ok(App {
            dock,
            layout_file: args.layout,
            worker,
            latest,
            ev: 0.0,
            pan: Vec2::ZERO,
            zoom: 0.5,
            graph: big_graph(args.nodes),
            graph_pan: Vec2::new(20.0, 20.0),
            popped: Vec::new(),
            measures: Measures::default(),
            upload_ms: Arc::default(),
            bench: args.bench,
            last_frame: Instant::now(),
            seen: 0,
        })
    }
}

/// Draws the panels; holds what they need for one frame.
struct Panels<'a> {
    app: &'a mut App,
    frame: Option<(Frame, u64)>,
    pop: Option<Tab>,
}

impl Panels<'_> {
    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
        let app = &mut *self.app;
        if response.dragged() {
            app.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                app.zoom = (app.zoom * (1.0 + scroll * 0.002)).clamp(0.05, 16.0);
            }
        }
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            ViewportPaint {
                frame: self.frame.clone(),
                rect,
                pan: app.pan,
                zoom: app.zoom,
                upload_ms: app.upload_ms.clone(),
            },
        ));
    }

    fn graph(&mut self, ui: &mut egui::Ui) {
        let start = Instant::now();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
        let app = &mut *self.app;
        if response.dragged() {
            app.graph_pan += response.drag_delta();
        }
        let painter = ui.painter_at(rect);
        let (nodes, wires) = &app.graph;
        let at = |p: &Placed| {
            rect.min + app.graph_pan + Vec2::new(p.rank as f32 * 70.0, p.row as f32 * 34.0)
        };
        let size = Vec2::new(56.0, 22.0);
        for (from, to) in wires {
            let a = at(&nodes[*from]) + Vec2::new(size.x, size.y / 2.0);
            let b = at(&nodes[*to]) + Vec2::new(0.0, size.y / 2.0);
            let bend = Vec2::new(24.0, 0.0);
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [a, a + bend, b - bend, b],
                false,
                Color32::TRANSPARENT,
                Stroke::new(1.0, Color32::from_gray(120)),
            ));
        }
        for p in nodes.iter() {
            let r = Rect::from_min_size(at(p), size);
            if !rect.intersects(r) {
                continue;
            }
            let fill = if p.principal {
                Color32::from_rgb(70, 90, 140)
            } else {
                Color32::from_gray(60)
            };
            painter.rect_filled(r, 4.0, fill);
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                &p.name,
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        app.measures
            .graph_ms
            .push(start.elapsed().as_secs_f64() * 1000.0);
    }

    fn params(&mut self, ui: &mut egui::Ui) {
        let app = &mut *self.app;
        let slider = egui::Slider::new(&mut app.ev, -3.0..=3.0).text("Exposure");
        if ui.add(slider).changed() {
            app.worker.request(app.ev);
        }
        if ui.button("Reset view").clicked() {
            app.pan = Vec2::ZERO;
            app.zoom = 0.5;
        }
    }

    fn stats(&mut self, ui: &mut egui::Ui) {
        let m = &self.app.measures;
        let last = |v: &[f64]| v.last().copied().unwrap_or(0.0);
        ui.label(format!("frame {:.2} ms", last(&m.frame_ms)));
        ui.label(format!("graph panel {:.2} ms", last(&m.graph_ms)));
        if let Some((f, _)) = &self.frame {
            ui.label(format!(
                "{} render {:.1} ms at {:+.2} EV",
                if f.proxy { "proxy" } else { "exact" },
                f.took.as_secs_f64() * 1000.0,
                f.ev
            ));
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, tab: Tab) {
        match tab {
            Tab::Viewport => self.viewport(ui),
            Tab::Graph => self.graph(ui),
            Tab::Params => self.params(ui),
            Tab::Stats => self.stats(ui),
        }
    }
}

impl TabViewer for Panels<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(tab.name())
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.name().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        self.show(ui, *tab);
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Tab, _: egui_dock::NodePath) {
        if ui.button("Open in its own window").clicked() {
            self.pop = Some(*tab);
        }
    }

    fn scroll_bars(&self, _: &Tab) -> [bool; 2] {
        [false, false]
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.measures
            .frame_ms
            .push((now - self.last_frame).as_secs_f64() * 1000.0);
        self.last_frame = now;
        self.measures.frames += 1;
        let frame = self.latest.lock().unwrap().clone();
        if let Some((f, generation)) = &frame
            && *generation != self.seen
        {
            self.seen = *generation;
            let ms = f.took.as_secs_f64() * 1000.0;
            if f.proxy {
                self.measures.proxy_ms.push(ms);
            } else {
                self.measures.exact_ms.push(ms);
            }
        }
        let ctx = ui.ctx().clone();
        if let Some(limit) = self.bench {
            if self.measures.frames % 10 == 0 {
                self.ev = ((self.measures.frames / 10) % 7) as f64 - 3.0;
                self.worker.request(self.ev);
            }
            if self.measures.frames == limit {
                self.report();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }

        let mut dock = std::mem::replace(&mut self.dock, DockState::new(vec![]));
        let popped = self.popped.clone();
        let mut panels = Panels {
            app: self,
            frame,
            pop: None,
        };
        egui::CentralPanel::no_frame().show(ui, |ui| {
            DockArea::new(&mut dock)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut panels);
        });
        // Panels opened in their own native window; closing it docks them back.
        let mut back = Vec::new();
        for tab in popped {
            let id = egui::ViewportId::from_hash_of(tab.name());
            let builder = egui::ViewportBuilder::default()
                .with_title(format!("Helios: {}", tab.name()))
                .with_inner_size([520.0, 380.0]);
            ctx.show_viewport_immediate(id, builder, |ui, _class| {
                egui::CentralPanel::default().show(ui, |ui| panels.show(ui, tab));
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    back.push(tab);
                }
            });
        }
        if let Some(tab) = panels.pop.take()
            && let Some(found) = dock.find_tab(&tab)
        {
            dock.remove_tab(found);
            self.popped.push(tab);
        }
        for tab in back {
            self.popped.retain(|t| *t != tab);
            dock.push_to_focused_leaf(tab);
        }
        self.dock = dock;
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_layout();
    }

    fn on_exit(&mut self) {
        self.save_layout();
    }
}

impl App {
    fn save_layout(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.dock) {
            let _ = std::fs::write(&self.layout_file, text);
        }
    }

    fn report(&mut self) {
        self.measures.upload_ms = self.upload_ms.lock().unwrap().clone();
        let m = &self.measures;
        let report = serde_json::json!({
            "toolkit": "egui 0.36 (eframe, wgpu)",
            "frames": m.frames,
            "frame_ms": summary(&m.frame_ms[1..]),
            "graph_panel_ms": summary(&m.graph_ms),
            "upload_ms": summary(&m.upload_ms),
            "proxy_ms": summary(&m.proxy_ms),
            "exact_ms": summary(&m.exact_ms),
            "nodes": self.graph.0.len(),
        });
        println!("{report}");
    }
}

struct Args {
    image: Option<PathBuf>,
    layout: PathBuf,
    bench: Option<u64>,
    nodes: usize,
}

fn args() -> Result<Args, String> {
    let mut args = Args {
        image: None,
        layout: PathBuf::from("layout-egui.json"),
        bench: None,
        nodes: 200,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--image" => args.image = Some(value()?.into()),
            "--layout" => args.layout = value()?.into(),
            "--bench" => args.bench = Some(value()?.parse().map_err(|e| format!("--bench: {e}"))?),
            "--nodes" => args.nodes = value()?.parse().map_err(|e| format!("--nodes: {e}"))?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() -> Result<(), String> {
    let args = args()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Helios prototype (egui)")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    let mut options = options;
    if args.bench.is_some() {
        // Measures frame cost, not the display's refresh rate.
        options.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native(
        "helios-proto-egui",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc, args)?) as Box<dyn eframe::App>)),
    )
    .map_err(|e| e.to_string())
}
