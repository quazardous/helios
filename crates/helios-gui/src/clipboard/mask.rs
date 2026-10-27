//! `helios.mask`: drawn shapes of a mask, with the size of the image
//! they were drawn on.
//!
//! A copy takes the selected shape of the mask edited, or every shape of
//! that mask when none is selected, each with its parameters as the
//! store holds them now, and the source image's size. Shapes are in
//! full-resolution pixels of the image (`docs/masks.md`, "Drawn
//! shapes"), so a paste onto an image of another size scales them, as
//! darktable's normalised coordinates do: positions by the width and the
//! height, lengths (radii, feathers, a brush's radius) by the smaller
//! side; angles stay. When the aspect ratio differs, a shape keeps its
//! relative place but an ellipse or a circle is not stretched with the
//! image: the paste says so. A paste adds the shapes to the mask edited
//! (`mask.paste`), named as new shapes are (`circle2` when `circle` is
//! taken).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::AppHandle;
use egui_pop::api::clipboard::{ClipItem, ClipSource, ClipTarget, FormatId, Part, Paste, TEXT};
use helios_core::{Params, Value};
use helios_template::format::NodeDesc;
use helios_values::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use super::MASK;

/// The type's version.
pub const VERSION: u32 = 1;

/// One shape copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Its node's name where it was copied.
    pub name: String,
    pub module: String,
    pub params: Params,
}

/// What a copy of shapes holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// The size of the image they were drawn on, in pixels.
    pub size: [f64; 2],
    pub shapes: Vec<Shape>,
}

impl Clip {
    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or_default()
    }

    pub fn from_json(data: &Json) -> Result<Clip, String> {
        serde_json::from_value(data.clone()).map_err(|e| format!("not mask shapes: {e}"))
    }

    /// Its text fallback: the shapes' names and the image's size.
    pub fn text(&self) -> String {
        let names: Vec<&str> = self.shapes.iter().map(|s| s.name.as_str()).collect();
        format!(
            "{} (drawn on {}×{})",
            names.join(", "),
            self.size[0],
            self.size[1]
        )
    }

    /// What it is called ("a shape", "3 shapes").
    pub fn title(&self) -> String {
        match self.shapes.len() {
            1 => "a shape".into(),
            n => format!("{n} shapes"),
        }
    }

    pub fn item(&self) -> ClipItem {
        ClipItem::new(MASK, VERSION, self.to_json(), &self.title())
            .fallback(TEXT, Json::String(self.text()))
    }

    /// Its shapes scaled to an image of `to` pixels, and what the paste
    /// loses: the aspect ratio when it differs.
    pub fn scaled_to(&self, to: [f64; 2]) -> (Vec<Shape>, Option<String>) {
        let shapes = self
            .shapes
            .iter()
            .map(|s| Shape {
                params: scaled(&s.module, &s.params, self.size, to),
                ..s.clone()
            })
            .collect();
        (shapes, distorted(self.size, to))
    }
}

/// The parts paste special offers: each shape.
pub fn parts(payload: &Json) -> Vec<Part> {
    Clip::from_json(payload)
        .map(|c| {
            c.shapes
                .iter()
                .map(|s| Part::new(&s.name, &s.name))
                .collect()
        })
        .unwrap_or_default()
}

/// The aspect ratio's change, when there is one.
pub fn distorted(from: [f64; 2], to: [f64; 2]) -> Option<String> {
    let (a, b) = (from[0] / from[1], to[0] / to[1]);
    ((a - b).abs() > 1e-6 * a.max(b)).then(|| "the aspect ratio: check the shapes' sizes".into())
}

/// Values a point of a list holds: path points are six coordinates
/// (corner, handle before, handle after), brush points the same then a
/// radius, a hardness and a density.
fn point_layout(module: &str) -> Option<(usize, usize)> {
    match module {
        "mask_path" => Some((helios_module_masks::shape::PATH_POINT_VALUES, 6)),
        "mask_brush" => Some((helios_module_masks::shape::BRUSH_POINT_VALUES, 6)),
        _ => None,
    }
}

/// A shape's parameters drawn on an image of `from` pixels, scaled to
/// one of `to`: positions by the width and the height, lengths by the
/// smaller side, angles and the rest as they are.
pub fn scaled(module: &str, params: &Params, from: [f64; 2], to: [f64; 2]) -> Params {
    let (sx, sy) = (to[0] / from[0], to[1] / from[1]);
    let k = to[0].min(to[1]) / from[0].min(from[1]);
    let mut out = Params::new();
    for (name, v) in params.iter() {
        let v = match (name, v) {
            ("x", Value::Float(x)) => Value::Float(x * sx),
            ("y", Value::Float(y)) => Value::Float(y * sy),
            ("radius" | "radius_x" | "radius_y" | "feather", Value::Float(r)) => {
                Value::Float(r * k)
            }
            ("points", Value::List(l)) => match point_layout(module) {
                Some((each, coords)) => Value::List(
                    l.iter()
                        .enumerate()
                        .map(|(i, v)| match i % each {
                            j if j < coords && j % 2 == 0 => v * sx,
                            j if j < coords => v * sy,
                            // A brush point's radius.
                            j if j == coords => v * k,
                            _ => *v,
                        })
                        .collect(),
                ),
                None => v.clone(),
            },
            _ => v.clone(),
        };
        out.set(name, v);
    }
    out
}

/// Shapes as nodes of a network (no wires): what a mask pasted into the
/// graph view becomes.
pub fn as_nodes(clip: &Clip) -> super::nodes::Clip {
    super::nodes::Clip {
        nodes: clip
            .shapes
            .iter()
            .map(|s| NodeDesc {
                name: s.name.clone(),
                module: Some(s.module.clone()),
                params: s.params.clone(),
                ..NodeDesc::default()
            })
            .collect(),
        ..super::nodes::Clip::default()
    }
}

/// The masks' endpoint, for the Masks panel and the image: copies the
/// selected shape (or the mask's shapes), pastes into the mask edited
/// through `mask.paste`.
pub struct Endpoint {
    values: Arc<Mutex<Store>>,
    viewport: Rc<RefCell<crate::viewport::Viewport>>,
}

impl Endpoint {
    pub fn new(
        values: Arc<Mutex<Store>>,
        viewport: Rc<RefCell<crate::viewport::Viewport>>,
    ) -> Self {
        Self { values, viewport }
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a copy takes now.
    pub fn clip(&self) -> Result<Clip, String> {
        let viewport = self
            .viewport
            .try_borrow()
            .map_err(|_| "the image is busy".to_string())?;
        let graph = viewport.graph().ok_or("no image is open")?;
        let [w, h] = viewport.image_size().ok_or("the image is not read yet")?;
        let store = self.store();
        let mask = crate::masks::current_mask(graph, &store).ok_or("the image has no mask")?;
        let selected = match store.get(crate::masks::SHAPE).map(|e| &e.value) {
            Some(Value::Text(t)) => t.clone(),
            _ => String::new(),
        };
        let all = crate::masks::members(graph, &store, &mask);
        let chosen: Vec<_> = if all.iter().any(|m| m.node == selected) {
            all.into_iter().filter(|m| m.node == selected).collect()
        } else {
            all
        };
        if chosen.is_empty() {
            return Err(format!("the mask `{mask}` has no drawn shape"));
        }
        Ok(Clip {
            size: [f64::from(w), f64::from(h)],
            shapes: chosen
                .into_iter()
                .map(|m| Shape {
                    name: m.node,
                    module: m.module,
                    params: m.params,
                })
                .collect(),
        })
    }

    /// The size of the image pasted onto, when it has a mask to draw in.
    fn target(&self) -> Option<[f64; 2]> {
        let viewport = self.viewport.try_borrow().ok()?;
        let graph = viewport.graph()?;
        crate::masks::current_mask(graph, &self.store())?;
        let [w, h] = viewport.image_size()?;
        Some([f64::from(w), f64::from(h)])
    }
}

/// Says the Masks panel or the image has the focus.
pub fn focus(app: &AppHandle, endpoint: &str, widget: egui::Id) {
    egui_pop::api::clipboard::bind(app, endpoint, widget);
}

impl ClipSource for Endpoint {
    fn copy(&self, _: &AppHandle) -> Option<ClipItem> {
        self.clip().ok().map(|c| c.item())
    }

    fn can_copy(&self, _: &AppHandle) -> bool {
        self.clip().is_ok()
    }
}

impl ClipTarget for Endpoint {
    fn accepts(&self, _: &AppHandle) -> Vec<FormatId> {
        if self.target().is_some() {
            vec![MASK]
        } else {
            Vec::new()
        }
    }

    fn paste(&self, app: &AppHandle, p: &Paste<'_>) -> Result<Json, String> {
        let mut clip = Clip::from_json(&p.repr.data)?;
        clip.shapes.retain(|s| p.wants(&s.name));
        if clip.shapes.is_empty() {
            return Err("no shape to paste".into());
        }
        let to = self
            .target()
            .ok_or("no image with a mask to draw in is open")?;
        let (_, lost) = clip.scaled_to(to);
        let names: Vec<&str> = clip.shapes.iter().map(|s| s.name.as_str()).collect();
        let plan = format!(
            "adds {} scaled from {}×{} to {}×{}",
            names.join(", "),
            clip.size[0],
            clip.size[1],
            to[0],
            to[1]
        );
        if p.dry_run {
            return Ok(json!(plan));
        }
        // The mask's network is the image's: the shell adds the shapes at
        // the next frame, scaled to the image open then.
        super::defer(
            app,
            "mask.paste",
            json!({ "clip": clip.to_json() }),
            p.origin,
        );
        Ok(json!({ "plan": plan, "lost": lost }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle() -> Params {
        Params::new()
            .with("x", Value::Float(3000.0))
            .with("y", Value::Float(1000.0))
            .with("radius", Value::Float(400.0))
            .with("feather", Value::Float(40.0))
            .with("opacity", Value::Float(0.5))
            .with("invert", Value::Bool(true))
    }

    #[test]
    fn positions_scale_by_each_side_and_lengths_by_the_smaller() {
        // 6000×4000 onto 3000×2000: half everywhere.
        let p = scaled("mask_circle", &circle(), [6000.0, 4000.0], [3000.0, 2000.0]);
        assert_eq!(p.get("x"), Some(&Value::Float(1500.0)));
        assert_eq!(p.get("y"), Some(&Value::Float(500.0)));
        assert_eq!(p.get("radius"), Some(&Value::Float(200.0)));
        assert_eq!(p.get("feather"), Some(&Value::Float(20.0)));
        // What is not a size stays.
        assert_eq!(p.get("opacity"), Some(&Value::Float(0.5)));
        assert_eq!(p.get("invert"), Some(&Value::Bool(true)));
        assert_eq!(distorted([6000.0, 4000.0], [3000.0, 2000.0]), None);
        // Onto a portrait 4000×6000: the place is relative, the radius
        // follows the smaller side (4000 → 4000), and the change is said.
        let p = scaled("mask_circle", &circle(), [6000.0, 4000.0], [4000.0, 6000.0]);
        assert!((p.float("x") - 2000.0).abs() < 1e-9);
        assert!((p.float("y") - 1500.0).abs() < 1e-9);
        assert_eq!(p.get("radius"), Some(&Value::Float(400.0)));
        assert!(distorted([6000.0, 4000.0], [4000.0, 6000.0]).is_some());
    }

    #[test]
    fn path_and_brush_points_scale_their_coordinates() {
        let path = Params::new()
            .with(
                "points",
                Value::List(vec![10.0, 20.0, 11.0, 21.0, 12.0, 22.0]),
            )
            .with("feather", Value::Float(4.0));
        let p = scaled("mask_path", &path, [100.0, 100.0], [200.0, 50.0]);
        assert_eq!(
            p.get("points"),
            Some(&Value::List(vec![20.0, 10.0, 22.0, 10.5, 24.0, 11.0]))
        );
        assert_eq!(p.get("feather"), Some(&Value::Float(2.0)));
        // A brush point: six coordinates, a radius, a hardness, a density.
        let brush = Params::new().with(
            "points",
            Value::List(vec![10.0, 20.0, 10.0, 20.0, 10.0, 20.0, 8.0, 0.5, 1.0]),
        );
        let p = scaled("mask_brush", &brush, [100.0, 100.0], [200.0, 200.0]);
        assert_eq!(
            p.get("points"),
            Some(&Value::List(vec![
                20.0, 40.0, 20.0, 40.0, 20.0, 40.0, 16.0, 0.5, 1.0
            ]))
        );
    }

    #[test]
    fn a_clip_reads_as_text_and_becomes_nodes() {
        let clip = Clip {
            size: [6000.0, 4000.0],
            shapes: vec![Shape {
                name: "circle".into(),
                module: "mask_circle".into(),
                params: circle(),
            }],
        };
        assert_eq!(clip.text(), "circle (drawn on 6000×4000)");
        assert_eq!(clip.title(), "a shape");
        let back = Clip::from_json(&clip.to_json()).unwrap();
        assert_eq!(back, clip);
        let (shapes, lost) = clip.scaled_to([3000.0, 2000.0]);
        assert_eq!(shapes[0].params.get("radius"), Some(&Value::Float(200.0)));
        assert_eq!(lost, None);
        let nodes = as_nodes(&clip);
        assert_eq!(nodes.names(), ["circle"]);
        assert_eq!(nodes.nodes[0].module.as_deref(), Some("mask_circle"));
        assert_eq!(parts(&clip.to_json())[0].id, "circle");
    }
}
