//! GUI prototype in iced (wgpu): a float viewport drawn by a wgpu shader
//! widget, panes (pane_grid) that can be moved, resized and opened in their
//! own native window (iced's daemon: one view per window), the layout saved
//! and restored, a 200-node graph on a canvas.
//!
//!     helios-proto-iced [--image FILE] [--layout FILE] [--bench FRAMES]
//!
//! `--bench` drives the app for a number of frames (changing the exposure
//! every 10 frames), prints the measures as JSON and quits.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use helios_proto_scene::{Frame, Placed, Worker, big_graph};
use iced::mouse;
use iced::wgpu;
use iced::widget::canvas::{self, Canvas, Path, Stroke};
use iced::widget::pane_grid::{self, Axis, Configuration, PaneGrid};
use iced::widget::shader::{self, Primitive, Viewport};
use iced::widget::{button, column, container, slider, text};
use iced::window;
use iced::{Color, Element, Length, Point, Rectangle, Size, Subscription, Task, Vector};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// The layout as saved: pane_grid has no serialization of its own, so its
/// tree is mirrored here.
#[derive(Serialize, Deserialize)]
enum Saved {
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Saved>,
        b: Box<Saved>,
    },
    Pane(Tab),
}

fn default_layout() -> Saved {
    Saved::Split {
        vertical: true,
        ratio: 0.75,
        a: Box::new(Saved::Split {
            vertical: false,
            ratio: 0.65,
            a: Box::new(Saved::Pane(Tab::Viewport)),
            b: Box::new(Saved::Pane(Tab::Graph)),
        }),
        b: Box::new(Saved::Split {
            vertical: false,
            ratio: 0.5,
            a: Box::new(Saved::Pane(Tab::Params)),
            b: Box::new(Saved::Pane(Tab::Stats)),
        }),
    }
}

fn to_configuration(saved: &Saved) -> Configuration<Tab> {
    match saved {
        Saved::Pane(tab) => Configuration::Pane(*tab),
        Saved::Split {
            vertical,
            ratio,
            a,
            b,
        } => Configuration::Split {
            axis: if *vertical {
                Axis::Vertical
            } else {
                Axis::Horizontal
            },
            ratio: *ratio,
            a: Box::new(to_configuration(a)),
            b: Box::new(to_configuration(b)),
        },
    }
}

fn from_state(state: &pane_grid::State<Tab>, node: &pane_grid::Node) -> Saved {
    match node {
        pane_grid::Node::Pane(pane) => Saved::Pane(*state.get(*pane).expect("a pane of the grid")),
        pane_grid::Node::Split {
            axis, ratio, a, b, ..
        } => Saved::Split {
            vertical: *axis == Axis::Vertical,
            ratio: *ratio,
            a: Box::new(from_state(state, a)),
            b: Box::new(from_state(state, b)),
        },
    }
}

/// A rendered frame as a message (frames are not `Debug`).
#[derive(Clone)]
struct Rendered(Arc<Frame>);

impl fmt::Debug for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rendered(proxy: {})", self.0.proxy)
    }
}

#[derive(Clone)]
struct Handle(Arc<Worker>);

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Worker")
    }
}

#[derive(Clone, Debug)]
enum Message {
    Ready(Handle),
    Rendered(Rendered),
    Exposure(f64),
    Dragged(pane_grid::DragEvent),
    Resized(pane_grid::ResizeEvent),
    PopOut(pane_grid::Pane),
    Opened(window::Id),
    Closed(window::Id),
    Tick(Instant),
    GraphPan(Vector),
}

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

enum Window {
    Main,
    Popped(Tab),
}

struct App {
    main: window::Id,
    windows: BTreeMap<window::Id, Window>,
    panes: pane_grid::State<Tab>,
    layout_file: PathBuf,
    worker: Option<Handle>,
    frame: Option<(Arc<Frame>, u64)>,
    ev: f64,
    graph: Arc<(Vec<Placed>, Vec<(usize, usize)>)>,
    graph_pan: Vector,
    measures: Measures,
    timings: Arc<Mutex<Timings>>,
    bench: Option<u64>,
    last_frame: Instant,
}

/// Measures taken while drawing (the canvas and the shader run outside
/// `update`).
#[derive(Default)]
struct Timings {
    graph_ms: Vec<f64>,
    upload_ms: Vec<f64>,
}

static ARGS: std::sync::OnceLock<Args> = std::sync::OnceLock::new();

impl App {
    fn boot() -> (Self, Task<Message>) {
        let args = ARGS.get().expect("arguments");
        let saved = std::fs::read_to_string(&args.layout)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(default_layout);
        let (main, open) = window::open(window::Settings {
            size: Size::new(1280.0, 800.0),
            ..Default::default()
        });
        let mut windows = BTreeMap::new();
        windows.insert(main, Window::Main);
        (
            App {
                main,
                windows,
                panes: pane_grid::State::with_configuration(to_configuration(&saved)),
                layout_file: args.layout.clone(),
                worker: None,
                frame: None,
                ev: 0.0,
                graph: Arc::new(big_graph(args.nodes)),
                graph_pan: Vector::new(20.0, 20.0),
                measures: Measures::default(),
                timings: Arc::default(),
                bench: args.bench,
                last_frame: Instant::now(),
            },
            open.map(Message::Opened),
        )
    }

    fn save_layout(&self) {
        let saved = from_state(&self.panes, self.panes.layout());
        if let Ok(text) = serde_json::to_string_pretty(&saved) {
            let _ = std::fs::write(&self.layout_file, text);
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ready(handle) => {
                handle.0.request(self.ev);
                self.worker = Some(handle);
            }
            Message::Rendered(Rendered(frame)) => {
                let ms = frame.took.as_secs_f64() * 1000.0;
                if frame.proxy {
                    self.measures.proxy_ms.push(ms);
                } else {
                    self.measures.exact_ms.push(ms);
                }
                let generation = self.frame.as_ref().map_or(1, |(_, g)| g + 1);
                self.frame = Some((frame, generation));
            }
            Message::Exposure(ev) => {
                self.ev = ev;
                if let Some(w) = &self.worker {
                    w.0.request(ev);
                }
            }
            Message::Dragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.panes.drop(pane, target);
                self.save_layout();
            }
            Message::Dragged(_) => {}
            Message::Resized(pane_grid::ResizeEvent { split, ratio }) => {
                self.panes.resize(split, ratio);
            }
            Message::PopOut(pane) => {
                if self.panes.len() > 1
                    && let Some((tab, _)) = self.panes.close(pane)
                {
                    let (id, open) = window::open(window::Settings {
                        size: Size::new(520.0, 380.0),
                        ..Default::default()
                    });
                    self.windows.insert(id, Window::Popped(tab));
                    return open.map(Message::Opened);
                }
            }
            Message::Opened(_) => {}
            Message::Closed(id) => match self.windows.remove(&id) {
                // Closing a panel's window docks it back next to the first pane.
                Some(Window::Popped(tab)) => {
                    let first = self.panes.iter().next().map(|(pane, _)| *pane);
                    if let Some(first) = first {
                        self.panes.split(Axis::Horizontal, first, tab);
                    }
                }
                Some(Window::Main) => {
                    self.save_layout();
                    return iced::exit();
                }
                None => {}
            },
            Message::GraphPan(delta) => self.graph_pan = self.graph_pan + delta,
            Message::Tick(now) => {
                self.measures
                    .frame_ms
                    .push((now - self.last_frame).as_secs_f64() * 1000.0);
                self.last_frame = now;
                self.measures.frames += 1;
                if let Some(limit) = self.bench {
                    if self.measures.frames % 10 == 0 {
                        let ev = ((self.measures.frames / 10) % 7) as f64 - 3.0;
                        return self.update(Message::Exposure(ev));
                    }
                    if self.measures.frames >= limit {
                        self.report();
                        return iced::exit();
                    }
                }
            }
        }
        Task::none()
    }

    fn report(&mut self) {
        let t = self.timings.lock().unwrap();
        let m = &self.measures;
        let report = serde_json::json!({
            "toolkit": "iced 0.14 (wgpu)",
            "frames": m.frames,
            "frame_ms": summary(&m.frame_ms[1..]),
            "graph_panel_ms": summary(&t.graph_ms),
            "upload_ms": summary(&t.upload_ms),
            "proxy_ms": summary(&m.proxy_ms),
            "exact_ms": summary(&m.exact_ms),
            "nodes": self.graph.0.len(),
        });
        println!("{report}");
    }

    fn panel(&self, tab: Tab) -> Element<'_, Message> {
        match tab {
            Tab::Viewport => shader::Shader::new(ViewportProgram {
                frame: self.frame.clone(),
                timings: self.timings.clone(),
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            Tab::Graph => Canvas::new(GraphProgram {
                graph: self.graph.clone(),
                pan: self.graph_pan,
                timings: self.timings.clone(),
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            Tab::Params => column![
                text("Exposure"),
                slider(-3.0..=3.0, self.ev, Message::Exposure).step(0.01),
            ]
            .spacing(8)
            .padding(8)
            .into(),
            Tab::Stats => {
                let last = |v: &[f64]| v.last().copied().unwrap_or(0.0);
                let t = self.timings.lock().unwrap();
                let render = self.frame.as_ref().map_or(String::new(), |(f, _)| {
                    format!(
                        "{} render {:.1} ms at {:+.2} EV",
                        if f.proxy { "proxy" } else { "exact" },
                        f.took.as_secs_f64() * 1000.0,
                        f.ev
                    )
                });
                column![
                    text(format!("frame {:.2} ms", last(&self.measures.frame_ms))),
                    text(format!("graph panel {:.2} ms", last(&t.graph_ms))),
                    text(render),
                ]
                .spacing(4)
                .padding(8)
                .into()
            }
        }
    }

    fn view(&self, id: window::Id) -> Element<'_, Message> {
        match self.windows.get(&id) {
            Some(Window::Popped(tab)) => container(self.panel(*tab)).padding(4).into(),
            _ => PaneGrid::new(&self.panes, |pane, tab, _maximized| {
                let title = pane_grid::TitleBar::new(text(tab.name()))
                    .controls(pane_grid::Controls::new(
                        button(text("pop out").size(12)).on_press(Message::PopOut(pane)),
                    ))
                    .padding(4)
                    .style(container::rounded_box);
                pane_grid::Content::new(self.panel(*tab)).title_bar(title)
            })
            .spacing(4)
            .on_drag(Message::Dragged)
            .on_resize(8, Message::Resized)
            .into(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let image = ARGS.get().and_then(|a| a.image.clone());
        let mut subs = vec![
            Subscription::run_with(image, render_worker),
            window::close_events().map(Message::Closed),
        ];
        // Asking for every frame redraws continuously: only when measuring.
        if self.bench.is_some() {
            subs.push(window::frames().map(Message::Tick));
        }
        Subscription::batch(subs)
    }

    fn title(&self, id: window::Id) -> String {
        match self.windows.get(&id) {
            Some(Window::Popped(tab)) => format!("Helios: {}", tab.name()),
            _ => "Helios prototype (iced)".into(),
        }
    }
}

/// The render worker as a stream of messages.
fn render_worker(image: &Option<PathBuf>) -> impl iced::futures::Stream<Item = Message> + use<> {
    let image = image.clone();
    iced::stream::channel(16, async move |mut output| {
        use iced::futures::SinkExt;
        let picture = match helios_proto_scene::image(image.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("helios-proto-iced: {e}");
                return;
            }
        };
        let sink = output.clone();
        let worker = Worker::spawn(picture, move |frame| {
            let _ = sink
                .clone()
                .try_send(Message::Rendered(Rendered(Arc::new(frame))));
        });
        let _ = output.send(Message::Ready(Handle(Arc::new(worker)))).await;
        std::future::pending::<()>().await;
    })
}

/// Pan and zoom of the viewport, kept by the widget.
#[derive(Default)]
struct ViewState {
    pan: Vector,
    zoom: Option<f32>,
    drag: Option<Point>,
}

struct ViewportProgram {
    frame: Option<(Arc<Frame>, u64)>,
    timings: Arc<Mutex<Timings>>,
}

impl shader::Program<Message> for ViewportProgram {
    type State = ViewState;
    type Primitive = ViewportPrimitive;

    fn update(
        &self,
        state: &mut ViewState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        let iced::Event::Mouse(event) = event else {
            return None;
        };
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                state.drag = cursor.position_in(bounds);
                state.drag.map(|_| shader::Action::capture())
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.drag = None;
                None
            }
            mouse::Event::CursorMoved { .. } => {
                let (Some(from), Some(to)) = (state.drag, cursor.position_in(bounds)) else {
                    return None;
                };
                state.pan = state.pan + (to - from);
                state.drag = Some(to);
                Some(shader::Action::request_redraw().and_capture())
            }
            mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => {
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * 40.0,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                let zoom = state.zoom.unwrap_or(0.5);
                state.zoom = Some((zoom * (1.0 + y * 0.002)).clamp(0.05, 16.0));
                Some(shader::Action::request_redraw().and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &ViewState, _: mouse::Cursor, _: Rectangle) -> ViewportPrimitive {
        ViewportPrimitive {
            frame: self.frame.clone(),
            pan: state.pan,
            zoom: state.zoom.unwrap_or(0.5),
            timings: self.timings.clone(),
        }
    }
}

struct ViewportPrimitive {
    frame: Option<(Arc<Frame>, u64)>,
    pan: Vector,
    zoom: f32,
    timings: Arc<Mutex<Timings>>,
}

impl fmt::Debug for ViewportPrimitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ViewportPrimitive")
    }
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

struct ViewportGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    image: Option<(wgpu::BindGroup, u64)>,
    encode: bool,
}

impl shader::Pipeline for ViewportGpu {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
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
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
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
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
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
            encode: !format.is_srgb(),
        }
    }
}

impl Primitive for ViewportPrimitive {
    type Pipeline = ViewportGpu;

    fn prepare(
        &self,
        gpu: &mut ViewportGpu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let Some((frame, generation)) = &self.frame else {
            return;
        };
        if gpu.image.as_ref().is_none_or(|(_, g)| g != generation) {
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
            self.timings
                .lock()
                .unwrap()
                .upload_ms
                .push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let scale = viewport.scale_factor();
        let proxy = if frame.proxy { 4.0 } else { 1.0 };
        let r = frame.image.region();
        let uniform = ViewUniform {
            origin: [bounds.x * scale, bounds.y * scale],
            pan: [self.pan.x * scale, self.pan.y * scale],
            zoom: self.zoom * scale * proxy,
            encode: if gpu.encode { 1.0 } else { 0.0 },
            size: [r.width as f32, r.height as f32],
        };
        queue.write_buffer(&gpu.uniform, 0, bytemuck::bytes_of(&uniform));
    }

    fn draw(&self, gpu: &ViewportGpu, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some((group, _)) = &gpu.image else {
            return true;
        };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

struct GraphProgram {
    graph: Arc<(Vec<Placed>, Vec<(usize, usize)>)>,
    pan: Vector,
    timings: Arc<Mutex<Timings>>,
}

#[derive(Default)]
struct GraphState {
    drag: Option<Point>,
}

impl canvas::Program<Message> for GraphProgram {
    type State = GraphState;

    fn update(
        &self,
        state: &mut GraphState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let iced::Event::Mouse(event) = event else {
            return None;
        };
        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                state.drag = cursor.position_in(bounds);
                state.drag.map(|_| canvas::Action::capture())
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.drag = None;
                None
            }
            mouse::Event::CursorMoved { .. } => {
                let (Some(from), Some(to)) = (state.drag, cursor.position_in(bounds)) else {
                    return None;
                };
                state.drag = Some(to);
                Some(canvas::Action::publish(Message::GraphPan(to - from)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _: &GraphState,
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let start = Instant::now();
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (nodes, wires) = &*self.graph;
        let at = |p: &Placed| Point::new(p.rank as f32 * 70.0, p.row as f32 * 34.0) + self.pan;
        let size = Size::new(56.0, 22.0);
        let wires_path = Path::new(|b| {
            for (from, to) in wires {
                let a = at(&nodes[*from]) + Vector::new(size.width, size.height / 2.0);
                let c = at(&nodes[*to]) + Vector::new(0.0, size.height / 2.0);
                b.move_to(a);
                b.bezier_curve_to(a + Vector::new(24.0, 0.0), c - Vector::new(24.0, 0.0), c);
            }
        });
        frame.stroke(
            &wires_path,
            Stroke::default()
                .with_width(1.0)
                .with_color(Color::from_rgb8(120, 120, 120)),
        );
        let visible = Rectangle::new(Point::ORIGIN, bounds.size());
        for p in nodes.iter() {
            let r = Rectangle::new(at(p), size);
            if !visible.intersects(&r) {
                continue;
            }
            let fill = if p.principal {
                Color::from_rgb8(70, 90, 140)
            } else {
                Color::from_rgb8(60, 60, 60)
            };
            frame.fill(
                &Path::rounded_rectangle(r.position(), r.size(), 4.0.into()),
                fill,
            );
            frame.fill_text(canvas::Text {
                content: p.name.clone(),
                position: r.center(),
                color: Color::WHITE,
                size: 11.0.into(),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        let geometry = frame.into_geometry();
        self.timings
            .lock()
            .unwrap()
            .graph_ms
            .push(start.elapsed().as_secs_f64() * 1000.0);
        vec![geometry]
    }
}

#[derive(Clone)]
struct Args {
    image: Option<PathBuf>,
    layout: PathBuf,
    bench: Option<u64>,
    nodes: usize,
}

fn args() -> Result<Args, String> {
    let mut args = Args {
        image: None,
        layout: PathBuf::from("layout-iced.json"),
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
    ARGS.set(args()?).ok();
    iced::daemon(App::boot, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .run()
        .map_err(|e| e.to_string())
}
