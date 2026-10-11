//! What both GUI prototypes show, so they are compared on the same work.
//!
//! - An image in the working space (32-bit float, values above 1: HDR),
//!   through a small network (source, exposure). Each change renders twice:
//!   a **proxy** (the same network on a quarter-size source, fast) then the
//!   **exact** image. The viewport shows the proxy until the exact one lands.
//! - A **large graph** (200 nodes by default) laid out by the core's layout
//!   hints, for the graph panel's frame time.
//!
//! The image is rendered in process by the engine: the node's endpoints
//! carry orchestration, not pixels, and how pixels reach a remote surface is
//! a separate question.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::graph::SOURCE_PORT;
use helios_core::{Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_module_exposure::Exposure;

/// A test image: a horizontal ramp from 0 to 4 (two stops over white) with
/// colored bars, so clipping and exposure are visible.
pub fn hdr_image(width: u32, height: u32) -> Image {
    Image::from_fn(Region::sized(width, height), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let t = x as f32 / width as f32 * 4.0;
        let band = (y as u32 * 6 / height) as usize;
        let tint: [[f32; 3]; 6] = [
            [1.0, 1.0, 1.0],
            [1.0, 0.2, 0.2],
            [0.2, 1.0, 0.2],
            [0.2, 0.2, 1.0],
            [1.0, 0.8, 0.3],
            [0.6, 0.3, 1.0],
        ];
        t * tint[band][c as usize]
    })
}

/// The image to show: a picture file when given, else [`hdr_image`].
pub fn image(path: Option<&Path>) -> Result<Image, String> {
    match path {
        Some(p) => {
            let loaded = helios_load::load(p)?;
            if loaded.image.channels() != 4 {
                return Err(format!(
                    "{}: a raw needs demosaicing first; give a picture",
                    p.display()
                ));
            }
            Ok(loaded.image)
        }
        None => Ok(hdr_image(2048, 1365)),
    }
}

fn quarter(image: &Image) -> Image {
    let r = image.region();
    let (w, h) = ((r.width / 4).max(1), (r.height / 4).max(1));
    Image::from_fn(Region::sized(w, h), 4, |x, y, c| {
        image.pixel(x * 4, y * 4)[c as usize]
    })
}

struct Net {
    graph: Graph,
    exposure: NodeId,
    region: Region,
}

impl Net {
    fn new(image: Image) -> Self {
        let region = image.region();
        let mut graph = Graph::new();
        let src = graph.add_source("src", image);
        let exposure = graph
            .add_module("exposure", ModuleRef::new(Exposure), &Params::new())
            .expect("exposure");
        graph
            .connect(src, SOURCE_PORT, exposure, "input")
            .expect("connect");
        Net {
            graph,
            exposure,
            region,
        }
    }
}

/// One render, and how long it took.
#[derive(Clone)]
pub struct Frame {
    pub image: Arc<Image>,
    pub proxy: bool,
    pub took: Duration,
    /// The exposure it was rendered with.
    pub ev: f64,
}

/// The network both prototypes drive.
pub struct Scene {
    engine: Engine,
    exact: Net,
    proxy: Net,
}

impl Scene {
    pub fn new(image: Image) -> Self {
        Scene {
            engine: Engine::new(Arc::new(Bus::new())),
            proxy: Net::new(quarter(&image)),
            exact: Net::new(image),
        }
    }

    fn render(&mut self, proxy: bool, ev: f64) -> Result<Frame, String> {
        let engine = &self.engine;
        let net = if proxy {
            &mut self.proxy
        } else {
            &mut self.exact
        };
        net.graph
            .set_param(net.exposure, "exposure", Value::Float(ev))
            .map_err(|e| e.to_string())?;
        let start = Instant::now();
        let image = engine
            .render(&net.graph, net.exposure, "output", net.region)
            .map_err(|e| e.to_string())?;
        Ok(Frame {
            image: Arc::new(image),
            proxy,
            took: start.elapsed(),
            ev,
        })
    }

    /// The proxy of an exposure: a quarter of the pixels.
    pub fn proxy(&mut self, ev: f64) -> Result<Frame, String> {
        self.render(true, ev)
    }

    pub fn exact(&mut self, ev: f64) -> Result<Frame, String> {
        self.render(false, ev)
    }
}

/// A node of the large graph, placed for drawing.
#[derive(Clone, Debug)]
pub struct Placed {
    pub name: String,
    pub rank: usize,
    pub row: usize,
    pub principal: bool,
}

/// A large network laid out by the core: `n` nodes in chains that merge.
/// Returns the nodes and the wires (from, to) as indices.
pub fn big_graph(n: usize) -> (Vec<Placed>, Vec<(usize, usize)>) {
    let mut graph = Graph::new();
    let mut ids = Vec::new();
    let src = graph.add_source("src", Image::from_fn(Region::sized(1, 1), 4, |_, _, _| 0.0));
    ids.push(src);
    let mut wires = Vec::new();
    let mut last = src;
    for i in 1..n {
        let node = graph
            .add_module(&format!("n{i}"), ModuleRef::new(Exposure), &Params::new())
            .expect("module");
        // Every eighth node restarts from the source: parallel chains.
        let from = if i % 8 == 0 { src } else { last };
        let port = if from == src { SOURCE_PORT } else { "output" };
        graph.connect(from, port, node, "input").expect("connect");
        wires.push((from.0, node.0));
        ids.push(node);
        last = node;
    }
    let hints = graph.layout();
    let mut rows = std::collections::HashMap::<usize, usize>::new();
    let placed = hints
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let row = rows.entry(h.rank).or_default();
            let p = Placed {
                name: if i == 0 {
                    "src".into()
                } else {
                    format!("n{i}")
                },
                rank: h.rank,
                row: *row,
                principal: h.principal,
            };
            *row += 1;
            p
        })
        .collect();
    (placed, wires)
}

/// Linear working-space RGBA to display bytes (sRGB, clipped): what a
/// viewport without a float path has to do on the CPU.
pub fn to_srgb8(image: &Image) -> Vec<u8> {
    let enc = |v: f32| -> u8 {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0 + 0.5) as u8
    };
    image
        .data()
        .chunks_exact(4)
        .flat_map(|p| [enc(p[0]), enc(p[1]), enc(p[2]), 255])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_then_exact_and_a_large_graph() {
        let mut scene = Scene::new(hdr_image(256, 128));
        let p = scene.proxy(1.0).unwrap();
        let e = scene.exact(1.0).unwrap();
        assert!(p.proxy && !e.proxy);
        assert_eq!(p.image.region(), Region::sized(64, 32));
        assert_eq!(e.image.region(), Region::sized(256, 128));
        let (nodes, wires) = big_graph(200);
        assert_eq!((nodes.len(), wires.len()), (200, 199));
        assert_eq!(to_srgb8(&e.image).len(), 256 * 128 * 4);
    }
}

/// Renders off the GUI thread: each request gives a proxy then the exact
/// image; requests that pile up while rendering collapse into the latest.
pub struct Worker {
    tx: std::sync::mpsc::Sender<f64>,
}

impl Worker {
    pub fn spawn(image: Image, on_frame: impl Fn(Frame) + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<f64>();
        std::thread::spawn(move || {
            let mut scene = Scene::new(image);
            let mut next = None;
            loop {
                let mut ev = match next.take() {
                    Some(ev) => ev,
                    None => match rx.recv() {
                        Ok(ev) => ev,
                        Err(_) => return,
                    },
                };
                while let Ok(later) = rx.try_recv() {
                    ev = later;
                }
                if let Ok(frame) = scene.proxy(ev) {
                    on_frame(frame);
                }
                // A newer request skips the exact render of this one.
                if let Ok(later) = rx.try_recv() {
                    next = Some(later);
                    continue;
                }
                if let Ok(frame) = scene.exact(ev) {
                    on_frame(frame);
                }
            }
        });
        Worker { tx }
    }

    pub fn request(&self, ev: f64) {
        let _ = self.tx.send(ev);
    }
}
