//! The image viewport: a node's output drawn as float data on the GPU, the
//! display transform its own last step.
//!
//! - **Renders** run on a thread of their own and never block a frame:
//!   first a proxy of the whole image at about the scale that fills the
//!   view, then, once the view has been still for a moment, the exact
//!   render of what is visible at the view's scale, in 256-pixel tiles
//!   kept between renders, so a pan renders only the new tiles. A newer
//!   view cancels the exact render under way.
//! - **The view** (zoom, centre, fit) lives in the value store at
//!   `view/viewport/*`, so keys, the palette and agents move it like any
//!   value; a drag is one gesture.
//! - **Drawing** goes through `egui_wgpu`'s paint callback on the device
//!   eframe opened: an `rgba32float` texture per layer, the display
//!   transform (to sRGB primaries, clipped, encoded) in the shader.
//! - **Pictures**: `viewport.display` is what is on screen; `viewport.scene`
//!   is the float data before the display transform.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use eframe::egui_wgpu::{self, wgpu};
use egui::{Rect, Sense, Ui};
use helios_core::{Bus, Engine, Graph, Image, NodeId, Region, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store};

/// The value paths of the view.
pub const ZOOM: &str = "view/viewport/zoom";
pub const CENTER_X: &str = "view/viewport/center_x";
pub const CENTER_Y: &str = "view/viewport/center_y";
pub const FIT: &str = "view/viewport/fit";

/// Side of a tile of the exact render, in pixels at its scale.
pub const TILE: u32 = 256;

/// How long the view stays still before the exact render, in seconds.
const SETTLE: f64 = 0.15;

/// Declares the view's values in a store.
pub fn declare(store: &mut Store) {
    let mut zoom = Descriptor::float("zoom", 0.01, 64.0, 1.0);
    zoom.unit = Some("%".into());
    zoom.display_scale = 100.0;
    zoom.precision = 0;
    store.declare(ZOOM, zoom);
    store.declare(CENTER_X, Descriptor::float("centre x", -1e6, 1e6, 0.0));
    store.declare(CENTER_Y, Descriptor::float("centre y", -1e6, 1e6, 0.0));
    store.declare(FIT, Descriptor::bool("fit to the view", true));
}

/// What the render thread did, for tests and the status bar.
#[derive(Default, Debug)]
pub struct Stats {
    pub proxies: AtomicU64,
    pub exacts: AtomicU64,
    pub tiles_rendered: AtomicU64,
    pub tiles_reused: AtomicU64,
    /// Exact renders given up for a newer view.
    pub cancelled: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Proxy,
    Exact,
}

struct Request {
    version: u64,
    graph: Arc<Graph>,
    node: NodeId,
    port: String,
    kind: Kind,
    scale: f64,
    /// In pixels at `scale`.
    region: Region,
}

/// A render, ready to draw: `image` covers `image.region()` in pixels at
/// `scale`.
#[derive(Clone)]
pub struct Layer {
    version: u64,
    kind: Kind,
    pub scale: f64,
    pub image: Image,
    generation: u64,
}

fn render_thread(
    requests: Receiver<Request>,
    results: Sender<Layer>,
    stats: Arc<Stats>,
    ctx: egui::Context,
) {
    let budget = helios_core::engine::memory_share(
        helios_core::engine::DEFAULT_CACHE_SHARE,
        helios_core::engine::physical_memory(),
    );
    let engine = Engine::with_budget(Arc::new(Bus::new()), budget);
    let mut tiles: HashMap<(u64, u64, i64, i64), Image> = HashMap::new();
    let mut waiting: Vec<Request> = Vec::new();
    let mut generation = 0;
    loop {
        if waiting.is_empty() {
            match requests.recv() {
                Ok(r) => waiting.push(r),
                Err(_) => return,
            }
        }
        // Only the newest of each kind matters.
        loop {
            match requests.try_recv() {
                Ok(r) => waiting.push(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let newest = |kind: Kind, w: &mut Vec<Request>| {
            let i = w.iter().rposition(|r| r.kind == kind)?;
            let r = w.remove(i);
            w.retain(|x| x.kind != kind);
            Some(r)
        };
        let proxy = newest(Kind::Proxy, &mut waiting);
        let exact = newest(Kind::Exact, &mut waiting);
        if let Some(r) = proxy
            && let Ok(image) = engine.render_at(&r.graph, r.node, &r.port, r.region, r.scale)
        {
            generation += 1;
            let _ = results.send(Layer {
                version: r.version,
                kind: Kind::Proxy,
                scale: r.scale,
                image,
                generation,
            });
            // Counted once sent: whoever reads the count finds the layer.
            stats.proxies.fetch_add(1, Ordering::Relaxed);
            ctx.request_repaint();
        }
        let Some(r) = exact else { continue };
        tiles.retain(|k, _| k.0 == r.version);
        let scale_key = r.scale.to_bits();
        let reg = r.region;
        let mut data = vec![0.0f32; reg.area() * 4];
        let mut cancelled = false;
        let (tx0, ty0) = (
            reg.x.div_euclid(i64::from(TILE)),
            reg.y.div_euclid(i64::from(TILE)),
        );
        let (tx1, ty1) = (
            (reg.right() - 1).div_euclid(i64::from(TILE)),
            (reg.bottom() - 1).div_euclid(i64::from(TILE)),
        );
        'tiles: for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                // A newer request: this view is gone.
                match requests.try_recv() {
                    Ok(newer) => {
                        waiting.push(newer);
                        cancelled = true;
                        break 'tiles;
                    }
                    Err(TryRecvError::Disconnected) => return,
                    Err(TryRecvError::Empty) => {}
                }
                let key = (r.version, scale_key, tx, ty);
                let tile = if let Some(t) = tiles.get(&key) {
                    stats.tiles_reused.fetch_add(1, Ordering::Relaxed);
                    t.clone()
                } else {
                    let full = Region::new(tx * i64::from(TILE), ty * i64::from(TILE), TILE, TILE);
                    let Some(within) = clip(full, r.extent_at()) else {
                        continue;
                    };
                    let Ok(t) = engine.render_at(&r.graph, r.node, &r.port, within, r.scale) else {
                        continue;
                    };
                    stats.tiles_rendered.fetch_add(1, Ordering::Relaxed);
                    tiles.insert(key, t.clone());
                    t
                };
                copy_into(&mut data, reg, &tile);
            }
        }
        if cancelled {
            stats.cancelled.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        generation += 1;
        let _ = results.send(Layer {
            version: r.version,
            kind: Kind::Exact,
            scale: r.scale,
            image: Image::new(reg, 4, data),
            generation,
        });
        stats.exacts.fetch_add(1, Ordering::Relaxed);
        ctx.request_repaint();
    }
}

impl Request {
    /// The node's extent at the request's scale.
    fn extent_at(&self) -> Region {
        let e = self
            .graph
            .extent(self.node)
            .ok()
            .flatten()
            .unwrap_or(Region::new(0, 0, 1, 1));
        scaled(e, self.scale)
    }
}

fn scaled(e: Region, scale: f64) -> Region {
    Region::new(
        0,
        0,
        ((f64::from(e.width) * scale).ceil() as u32).max(1),
        ((f64::from(e.height) * scale).ceil() as u32).max(1),
    )
}

fn clip(a: Region, b: Region) -> Option<Region> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    (x1 > x0 && y1 > y0).then(|| Region::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Copies a tile's pixels (4 channels expected) into a buffer covering `into`.
fn copy_into(data: &mut [f32], into: Region, tile: &Image) {
    let t = tile.region();
    let n = tile.channels() as usize;
    let Some(c) = clip(t, into) else { return };
    for y in c.y..c.bottom() {
        for x in c.x..c.right() {
            let src = tile.pixel(x, y);
            let at = (((y - into.y) as usize) * into.width as usize + (x - into.x) as usize) * 4;
            for k in 0..4 {
                data[at + k] = src[k.min(n - 1)];
            }
            if n < 4 {
                data[at + 3] = 1.0;
            }
        }
    }
}

/// What is shown: the graph and the node whose output it is.
struct Source {
    graph: Graph,
    shared: Arc<Graph>,
    node: NodeId,
    port: String,
    extent: Region,
}

/// The view as the widget computes it each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    /// Points on screen per image pixel.
    zoom: f64,
    center: (f64, f64),
}

pub struct Viewport {
    source: Option<Source>,
    version: u64,
    requests: Option<Sender<Request>>,
    results: Option<Receiver<Layer>>,
    proxy: Option<Layer>,
    exact: Option<Layer>,
    asked_proxy: Option<(u64, u64)>,
    asked_exact: Option<(u64, [u64; 4])>,
    last_view: Option<(View, egui::Vec2)>,
    changed_at: f64,
    gesture: Option<GestureId>,
    /// The view used last frame, for the zoom commands.
    pub effective_zoom: f64,
    pub stats: Arc<Stats>,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            source: None,
            version: 0,
            requests: None,
            results: None,
            proxy: None,
            exact: None,
            asked_proxy: None,
            asked_exact: None,
            last_view: None,
            changed_at: 0.0,
            gesture: None,
            effective_zoom: 1.0,
            stats: Arc::default(),
        }
    }
}

fn float(store: &Store, path: &str) -> f64 {
    store
        .get(path)
        .and_then(|e| e.value.as_f64())
        .unwrap_or(0.0)
}

impl Viewport {
    /// Shows a node's output; the store mirrors its parameters
    /// (`helios_values::bind_node`) and [`Viewport::sync`] follows them.
    pub fn set_source(&mut self, graph: Graph, node: NodeId, port: &str) -> Result<(), String> {
        let extent = graph
            .extent(node)
            .map_err(|e| e.to_string())?
            .ok_or("the node has no source feeding it")?;
        self.source = Some(Source {
            shared: Arc::new(graph.clone()),
            graph,
            node,
            port: port.into(),
            extent,
        });
        self.version += 1;
        self.proxy = None;
        self.exact = None;
        Ok(())
    }

    /// Brings the shown graph up to the store's node values; a change
    /// renders again.
    pub fn sync(&mut self, store: &mut Store) {
        let Some(src) = &mut self.source else { return };
        match helios_values::sync(store, &mut src.graph) {
            Ok(0) => {}
            Ok(_) => {
                src.shared = Arc::new(src.graph.clone());
                self.version += 1;
            }
            Err(e) => eprintln!("helios-gui: viewport: {e}"),
        }
    }

    /// The size of the image shown, in pixels.
    pub fn graph_extent(&self) -> Option<(f64, f64)> {
        self.source
            .as_ref()
            .map(|s| (f64::from(s.extent.width), f64::from(s.extent.height)))
    }

    /// The graph shown, to bind its nodes to a store.
    pub fn graph(&self) -> Option<&Graph> {
        self.source.as_ref().map(|s| &s.graph)
    }

    /// The float data on screen, before the display transform: the exact
    /// render when there is one, else the proxy.
    pub fn scene(&self) -> Option<&Image> {
        self.exact
            .as_ref()
            .or(self.proxy.as_ref())
            .map(|l| &l.image)
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.requests.is_some() {
            return;
        }
        let (rq, rx) = channel();
        let (tx, results) = channel();
        let stats = self.stats.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("helios-viewport".into())
            .spawn(move || render_thread(rx, tx, stats, ctx))
            .expect("a thread for the viewport");
        self.requests = Some(rq);
        self.results = Some(results);
    }

    fn receive(&mut self) {
        let Some(results) = &self.results else { return };
        while let Ok(layer) = results.try_recv() {
            if layer.version != self.version && self.proxy.is_some() {
                // Late: a newer version is on its way, keep what is shown
                // unless nothing is.
                continue;
            }
            match layer.kind {
                Kind::Proxy => self.proxy = Some(layer),
                Kind::Exact => self.exact = Some(layer),
            }
        }
    }

    fn send(&self, kind: Kind, scale: f64, region: Region) {
        let (Some(src), Some(rq)) = (&self.source, &self.requests) else {
            return;
        };
        let _ = rq.send(Request {
            version: self.version,
            graph: src.shared.clone(),
            node: src.node,
            port: src.port.clone(),
            kind,
            scale,
            region,
        });
    }

    fn write(store: &mut Store, path: &str, value: Value, gesture: Option<GestureId>) {
        let mut set = Set::new(path, Change::To(value), Origin::User);
        set.gesture = gesture;
        let _ = store.set(set);
    }

    /// Draws the viewport; reads and writes the view in `store`.
    pub fn ui(&mut self, ui: &mut Ui, store: &Mutex<Store>) {
        let ctx = ui.ctx().clone();
        self.start(&ctx);
        self.receive();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        crate::name_rect(&ctx, "viewport.display", rect);
        let background = ui.visuals().extreme_bg_color;
        ui.painter().rect_filled(rect, 0.0, background);
        let Some(extent) = self.source.as_ref().map(|s| s.extent) else {
            ui.put(
                rect,
                egui::Label::new(egui::RichText::new("No image").weak()),
            );
            return;
        };
        let (w, h) = (f64::from(extent.width), f64::from(extent.height));
        let mut store = store.lock().expect("value store lock");
        let fit = store
            .get(FIT)
            .and_then(|e| e.value.as_bool())
            .unwrap_or(true);
        let fit_zoom = (f64::from(rect.width()) / w).min(f64::from(rect.height()) / h);
        let mut view = if fit {
            View {
                zoom: fit_zoom,
                center: (w / 2.0, h / 2.0),
            }
        } else {
            View {
                zoom: float(&store, ZOOM).max(0.01),
                center: (float(&store, CENTER_X), float(&store, CENTER_Y)),
            }
        };
        // Leaving fit: the view it showed becomes the values.
        let unfit = |store: &mut Store, view: &View, g: Option<GestureId>| {
            Self::write(store, FIT, Value::Bool(false), g);
            Self::write(store, ZOOM, Value::Float(view.zoom), g);
            Self::write(store, CENTER_X, Value::Float(view.center.0), g);
            Self::write(store, CENTER_Y, Value::Float(view.center.1), g);
        };
        if response.drag_started() {
            let g = store.begin_gesture(Origin::User, "pan the image");
            if fit {
                unfit(&mut store, &view, Some(g));
            }
            self.gesture = Some(g);
        }
        if response.dragged() {
            let d = response.drag_delta();
            view.center.0 -= f64::from(d.x) / view.zoom;
            view.center.1 -= f64::from(d.y) / view.zoom;
            Self::write(
                &mut store,
                CENTER_X,
                Value::Float(view.center.0),
                self.gesture,
            );
            Self::write(
                &mut store,
                CENTER_Y,
                Value::Float(view.center.1),
                self.gesture,
            );
        }
        if response.drag_stopped()
            && let Some(g) = self.gesture.take()
        {
            store.end(g);
        }
        if response.double_clicked() {
            Self::write(&mut store, FIT, Value::Bool(true), None);
        }
        let scroll = if response.hovered() {
            ui.input(|i| i.smooth_scroll_delta.y)
        } else {
            0.0
        };
        if scroll != 0.0 {
            let g = store.begin_group(Origin::User, "zoom");
            let pointer = response.hover_pos().unwrap_or(rect.center()) - rect.center();
            let under = (
                view.center.0 + f64::from(pointer.x) / view.zoom,
                view.center.1 + f64::from(pointer.y) / view.zoom,
            );
            let zoom = (view.zoom * f64::from((scroll * 0.0015).exp())).clamp(0.01, 64.0);
            view = View {
                zoom,
                center: (
                    under.0 - f64::from(pointer.x) / zoom,
                    under.1 - f64::from(pointer.y) / zoom,
                ),
            };
            unfit(&mut store, &view, Some(g));
            store.end(g);
        }
        drop(store);
        self.effective_zoom = view.zoom;

        // Renders: a proxy that fills the view, then the exact visible part.
        let now = ctx.input(|i| i.time);
        let size = rect.size();
        if self.last_view != Some((view, size)) {
            self.last_view = Some((view, size));
            self.changed_at = now;
        }
        let ppp = f64::from(ctx.pixels_per_point());
        let fit_px = (fit_zoom * ppp).min(1.0);
        let proxy_scale = 2f64.powf(fit_px.log2().floor()).clamp(1.0 / 64.0, 1.0);
        if self.asked_proxy != Some((self.version, proxy_scale.to_bits())) {
            self.asked_proxy = Some((self.version, proxy_scale.to_bits()));
            self.send(Kind::Proxy, proxy_scale, scaled(extent, proxy_scale));
        }
        let scale = (view.zoom * ppp).min(1.0);
        let half = (
            f64::from(rect.width()) / 2.0 / view.zoom,
            f64::from(rect.height()) / 2.0 / view.zoom,
        );
        let at = |v: f64| (v * scale).floor() as i64;
        let visible = Region::new(
            at((view.center.0 - half.0).max(0.0)),
            at((view.center.1 - half.1).max(0.0)),
            1,
            1,
        );
        let x1 = ((view.center.0 + half.0).min(w) * scale).ceil() as i64;
        let y1 = ((view.center.1 + half.1).min(h) * scale).ceil() as i64;
        let key = [
            self.version,
            scale.to_bits(),
            ((visible.x as u64) << 32) | (visible.y as u64 & 0xffff_ffff),
            ((x1 as u64) << 32) | (y1 as u64 & 0xffff_ffff),
        ];
        if x1 > visible.x && y1 > visible.y {
            if now - self.changed_at >= SETTLE {
                if self.asked_exact != Some((self.version, key)) {
                    self.asked_exact = Some((self.version, key));
                    let region = Region::new(
                        visible.x,
                        visible.y,
                        (x1 - visible.x) as u32,
                        (y1 - visible.y) as u32,
                    );
                    self.send(Kind::Exact, scale, region);
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
            }
        }

        // Draw: the proxy, the exact render over it where it is.
        let base = self.proxy.clone().or_else(|| self.exact.clone());
        let Some(base) = base else { return };
        let overlay = self.exact.clone().filter(|_| self.proxy.is_some());
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Paint {
                rect,
                view,
                extent: (w as f32, h as f32),
                background,
                layers: [Some(base), overlay],
            },
        ));
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    rect: [f32; 4],
    look: [f32; 4],
    layer: [f32; 4],
    sizes: [f32; 4],
    m: [[f32; 4]; 3],
    background: [f32; 4],
}

struct Slot {
    uniform: wgpu::Buffer,
    group: Option<(wgpu::BindGroup, u64)>,
}

/// The viewport's GPU objects, kept in `egui_wgpu`'s callback resources.
pub struct Gpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    slots: [Slot; 2],
    encode: bool,
    matrix: [[f32; 4]; 3],
}

/// Installs the viewport's GPU objects on a render state (eframe's, or a
/// test harness's).
pub fn install(rs: &egui_wgpu::RenderState) {
    let device = &rs.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("helios.viewport"),
        source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("helios.viewport"),
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
        label: Some("helios.viewport"),
        bind_group_layouts: &[Some(&layout)],
        ..Default::default()
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("helios.viewport"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: rs.target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let slot = || Slot {
        uniform: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("helios.viewport"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        group: None,
    };
    rs.renderer.write().callback_resources.insert(Gpu {
        pipeline,
        layout,
        slots: [slot(), slot()],
        encode: !rs.target_format.is_srgb(),
        matrix: display_matrix(),
    });
}

/// Working space (linear Rec.2020) to the display's primaries (sRGB for
/// now; a monitor profile later), by rows.
pub fn display_matrix() -> [[f32; 4]; 3] {
    use helios_photo::color;
    let m = color::mul(
        &color::inverse(&color::SRGB_TO_XYZ).expect("invertible primaries"),
        &color::REC2020_TO_XYZ,
    );
    m.map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0])
}

/// The display transform on the CPU, as the shader does it: for tests and
/// for pictures of the scene.
pub fn display(rgb: [f32; 3]) -> [u8; 3] {
    let m = display_matrix();
    let d = [0, 1, 2].map(|i| m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2]);
    d.map(|v| {
        (helios_photo::color::srgb_encode(f64::from(v.clamp(0.0, 1.0))) * 255.0).round() as u8
    })
}

struct Paint {
    rect: Rect,
    view: View,
    extent: (f32, f32),
    background: egui::Color32,
    layers: [Option<Layer>; 2],
}

impl egui_wgpu::CallbackTrait for Paint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<Gpu>() else {
            return Vec::new();
        };
        let ppp = screen.pixels_per_point;
        let bg = egui::Rgba::from(self.background);
        for (i, layer) in self.layers.iter().enumerate() {
            let Some(layer) = layer else { continue };
            let slot = &mut gpu.slots[i];
            if slot
                .group
                .as_ref()
                .is_none_or(|(_, g)| *g != layer.generation)
            {
                let r = layer.image.region();
                let size = wgpu::Extent3d {
                    width: r.width,
                    height: r.height,
                    depth_or_array_layers: 1,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("helios.viewport.layer"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let data: Vec<f32> = if layer.image.channels() == 4 {
                    layer.image.data().to_vec()
                } else {
                    let n = layer.image.channels() as usize;
                    layer
                        .image
                        .data()
                        .chunks_exact(n)
                        .flat_map(|p| [p[0], p[1.min(n - 1)], p[2.min(n - 1)], 1.0])
                        .collect()
                };
                queue.write_texture(
                    texture.as_image_copy(),
                    bytemuck::cast_slice(&data),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(r.width * 16),
                        rows_per_image: Some(r.height),
                    },
                    size,
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("helios.viewport.layer"),
                    layout: &gpu.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: slot.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                    ],
                });
                slot.group = Some((group, layer.generation));
            }
            let r = layer.image.region();
            let texel = (1.0 / layer.scale) as f32;
            let uniform = Uniform {
                rect: [
                    self.rect.min.x * ppp,
                    self.rect.min.y * ppp,
                    self.rect.width() * ppp,
                    self.rect.height() * ppp,
                ],
                look: [
                    self.view.center.0 as f32,
                    self.view.center.1 as f32,
                    self.view.zoom as f32 * ppp,
                    if gpu.encode { 1.0 } else { 0.0 },
                ],
                layer: [r.x as f32 * texel, r.y as f32 * texel, texel, i as f32],
                sizes: [
                    r.width as f32,
                    r.height as f32,
                    self.extent.0,
                    self.extent.1,
                ],
                m: gpu.matrix,
                background: [bg.r(), bg.g(), bg.b(), 1.0],
            };
            queue.write_buffer(&slot.uniform, 0, bytemuck::bytes_of(&uniform));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = resources.get::<Gpu>() else {
            return;
        };
        pass.set_pipeline(&gpu.pipeline);
        for (i, layer) in self.layers.iter().enumerate() {
            if layer.is_none() {
                continue;
            }
            if let Some((group, _)) = &gpu.slots[i].group {
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
