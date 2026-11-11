//! Handles on the image: the image surface of a shape's controls
//! (`docs/design/controls.md`, sections 3.3 and 5.2, slices 12 and 13).
//!
//! A shape's case (the circle mask, the gradient mask) is declared here as
//! a [`Gizmo`]: one [`HandleSpec`] per handle, naming the case's part it
//! shows and the parameters it writes, what Shift does to its drag, what
//! a double click resets it to and what the wheel does to it. Each part is
//! the control its parameter already has in the panel (the one patched to
//! it, else its implicit one, [`control_of`]), so a handle and a panel
//! row are two surfaces of one controller:
//!
//! - **one gesture**: a drag is a store gesture holding the parameters it
//!   writes against other origins, the same the panel's slider opens
//!   (the panel's marks show the row held meanwhile);
//! - **one undo step** per drag, double click or wheel turn, put back
//!   with no trace on Escape;
//! - **one change step**: each finished change runs the `controls.change`
//!   step with the part's control, as a panel row's does, and a hook's
//!   veto or redirect holds on the image as in the panel;
//! - **the same verbs**: the agents' `mask.handle.move` drags through
//!   [`Drag`], `param.set` and `controls.*` reach the same parameters.
//!
//! Positions come from the shape's outline (`helios_module_masks`); the
//! viewport (egui-pop's overlay) draws the handles and says their drags,
//! double clicks and wheel turns. This module holds no egui.

use helios_core::{Params, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store, node_path};

use crate::flow::{Cause, ChangeEvent, Steps, Verdict};
use crate::model::Control;
use crate::registry::Registry;
use crate::{HeliosRegistry, implicit};

/// A point in image pixels.
pub type Point = [f64; 2];

/// A rotation snaps to steps of this many degrees with Shift held.
pub const ANGLE_STEP: f64 = 15.0;
/// With Ctrl held a handle moves this share of the pointer.
pub const FINE: f64 = 0.1;
/// A wheel unit scales a size or a feather by `exp(WHEEL_RATIO)`.
pub const WHEEL_RATIO: f64 = 0.002;
/// A wheel unit turns a rotation by this many degrees.
pub const WHEEL_TURN: f64 = 0.1;

/// What Shift does to a handle's drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    /// Turns it around the shape's centre to [`ANGLE_STEP`] steps.
    Angle,
    /// Holds its move to the axes and diagonals.
    Axes,
}

/// What the wheel over a handle does to its first parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wheel {
    /// Scales it by a ratio, never below 0 (a size, a feather).
    Scale,
    /// Turns it by degrees, kept in (-180, 180].
    Turn,
}

/// The parameters a drag to a point writes, from the parameters at the
/// gesture's start.
pub type Apply = fn(&Params, Point) -> Vec<(&'static str, f64)>;

/// A handle of a shape's case.
#[derive(Clone, Copy, Debug)]
pub struct HandleSpec {
    /// As the shape's outline names it (`centre`, `radius`...).
    pub name: &'static str,
    /// The case's part it shows (`center`, `radius`, `feather`,
    /// `rotation`).
    pub part: &'static str,
    /// The parameters it writes, the first the one it adjusts.
    pub writes: &'static [&'static str],
    pub shift: Option<Shift>,
    /// A double click puts its first parameter back to this.
    pub reset: Option<f64>,
    pub wheel: Option<Wheel>,
    pub apply: Apply,
}

/// A shape's case on the image: its handles.
#[derive(Clone, Debug)]
pub struct Gizmo {
    /// The case (`circle-mask`).
    pub control: &'static str,
    /// The module whose nodes it edits (`mask_circle`).
    pub module: &'static str,
    pub handles: Vec<HandleSpec>,
}

impl Gizmo {
    /// The handle named `name`.
    pub fn handle(&self, name: &str) -> Option<&HandleSpec> {
        self.handles.iter().find(|h| h.name == name)
    }
}

/// The circle mask's case.
pub const CIRCLE_MASK: &str = "circle-mask";
/// The ellipse mask's case.
pub const ELLIPSE_MASK: &str = "ellipse-mask";
/// The gradient mask's case.
pub const GRADIENT_MASK: &str = "gradient-mask";

/// Every shape case with handles declared here.
pub fn all() -> Vec<Gizmo> {
    vec![circle_mask(), ellipse_mask(), gradient_mask()]
}

/// The case of a module's nodes, if it has one here.
pub fn of_module(module: &str) -> Option<Gizmo> {
    all().into_iter().find(|g| g.module == module)
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: Point, k: f64) -> Point {
    [a[0] * k, a[1] * k]
}

fn len(a: Point) -> f64 {
    a[0].hypot(a[1])
}

/// An angle in degrees, in (-180, 180].
fn degrees(d: Point) -> f64 {
    d[1].atan2(d[0]).to_degrees()
}

/// An angle in degrees kept in (-180, 180].
pub fn wrap(deg: f64) -> f64 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// A shape's centre or anchor (`x`, `y`).
pub fn centre_of(params: &Params) -> Point {
    let n = |k: &str| params.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    [n("x"), n("y")]
}

fn centre(_: &Params, to: Point) -> Vec<(&'static str, f64)> {
    vec![("x", to[0]), ("y", to[1])]
}

/// The circle mask: its centre (a point, Shift along the axes), its
/// radius (a size the wheel scales) and its feather past the radius
/// (reset to none, scaled by the wheel).
pub fn circle_mask() -> Gizmo {
    Gizmo {
        control: CIRCLE_MASK,
        module: "mask_circle",
        handles: vec![
            HandleSpec {
                name: "centre",
                part: "center",
                writes: &["x", "y"],
                shift: Some(Shift::Axes),
                reset: None,
                wheel: None,
                apply: centre,
            },
            HandleSpec {
                name: "radius",
                part: "radius",
                writes: &["radius"],
                shift: None,
                reset: None,
                wheel: Some(Wheel::Scale),
                apply: |p, to| vec![("radius", len(sub(to, centre_of(p))))],
            },
            HandleSpec {
                name: "feather",
                part: "feather",
                writes: &["feather"],
                shift: None,
                reset: Some(0.0),
                wheel: Some(Wheel::Scale),
                apply: |p, to| {
                    let d = len(sub(to, centre_of(p)));
                    vec![("feather", (d - p.float("radius")).max(0.0))]
                },
            },
        ],
    }
}

/// The ellipse mask: its centre (a point, Shift along the axes), each
/// radius with the rotation it sets (Shift to 15° steps around the
/// centre, scaled by the wheel) and its feather past the first radius
/// (in proportion to the larger radius when `proportional` is on; reset
/// to none, scaled by the wheel).
pub fn ellipse_mask() -> Gizmo {
    Gizmo {
        control: ELLIPSE_MASK,
        module: "mask_ellipse",
        handles: vec![
            HandleSpec {
                name: "centre",
                part: "center",
                writes: &["x", "y"],
                shift: Some(Shift::Axes),
                reset: None,
                wheel: None,
                apply: centre,
            },
            HandleSpec {
                name: "radius_x",
                part: "radii",
                writes: &["radius_x", "rotation"],
                shift: Some(Shift::Angle),
                reset: None,
                wheel: Some(Wheel::Scale),
                apply: |p, to| {
                    let d = sub(to, centre_of(p));
                    vec![("radius_x", len(d)), ("rotation", degrees(d))]
                },
            },
            HandleSpec {
                name: "radius_y",
                part: "radii",
                writes: &["radius_y", "rotation"],
                shift: Some(Shift::Angle),
                reset: None,
                wheel: Some(Wheel::Scale),
                apply: |p, to| {
                    let d = sub(to, centre_of(p));
                    vec![("radius_y", len(d)), ("rotation", wrap(degrees(d) - 90.0))]
                },
            },
            HandleSpec {
                name: "feather",
                part: "feather",
                writes: &["feather"],
                shift: None,
                reset: Some(0.0),
                wheel: Some(Wheel::Scale),
                apply: |p, to| {
                    let d = len(sub(to, centre_of(p)));
                    let (a, b) = (p.float("radius_x"), p.float("radius_y"));
                    let proportional = p.get("proportional").and_then(Value::as_bool) == Some(true);
                    let feather = if proportional && a > 0.0 {
                        (d / a - 1.0) * a.max(b)
                    } else {
                        d - a
                    };
                    vec![("feather", feather.max(0.0))]
                },
            },
        ],
    }
}

/// The gradient mask: its anchor (a point, Shift along the axes), its
/// line's rotation (Shift to 15° steps, reset to 0°, turned by the
/// wheel) and its band's width (twice the handle's distance to the line,
/// reset to none, scaled by the wheel).
pub fn gradient_mask() -> Gizmo {
    Gizmo {
        control: GRADIENT_MASK,
        module: "mask_gradient",
        handles: vec![
            HandleSpec {
                name: "centre",
                part: "anchor",
                writes: &["x", "y"],
                shift: Some(Shift::Axes),
                reset: None,
                wheel: None,
                apply: centre,
            },
            HandleSpec {
                name: "rotation",
                part: "rotation",
                writes: &["rotation"],
                shift: Some(Shift::Angle),
                reset: Some(0.0),
                wheel: Some(Wheel::Turn),
                apply: |p, to| {
                    let d = sub(to, centre_of(p));
                    vec![("rotation", degrees([d[0], -d[1]]))]
                },
            },
            HandleSpec {
                name: "feather",
                part: "feather",
                writes: &["feather"],
                shift: None,
                reset: Some(0.0),
                wheel: Some(Wheel::Scale),
                apply: |p, to| {
                    let d = sub(to, centre_of(p));
                    let r = p.float("rotation").to_radians();
                    let n = [-r.sin(), -r.cos()];
                    vec![("feather", 2.0 * (d[0] * n[0] + d[1] * n[1]).abs())]
                },
            },
        ],
    }
}

/// Where a handle dragged from `start` to `to` goes: Ctrl (`fine`) moves
/// it a tenth of the pointer, then Shift (`shift`) holds it as its
/// [`Shift`] says (an angle turns around `pivot`).
pub fn snapped(
    rule: Option<Shift>,
    pivot: Point,
    start: Point,
    to: Point,
    shift: bool,
    fine: bool,
) -> Point {
    let mut to = to;
    if fine {
        to = add(start, scale(sub(to, start), FINE));
    }
    if shift {
        to = match rule {
            Some(Shift::Angle) => {
                let d = sub(to, pivot);
                let a = (degrees(d) / ANGLE_STEP).round() * ANGLE_STEP;
                let r = len(d);
                add(pivot, [r * a.to_radians().cos(), r * a.to_radians().sin()])
            }
            Some(Shift::Axes) => {
                let d = sub(to, start);
                let a = (degrees(d) / 45.0).round() * 45.0;
                let u = [a.to_radians().cos(), a.to_radians().sin()];
                add(start, scale(u, d[0] * u[0] + d[1] * u[1]))
            }
            None => to,
        };
    }
    to
}

/// What `delta` wheel units over a handle write: its first parameter,
/// scaled or turned from `params`.
pub fn wheeled(h: &HandleSpec, params: &Params, delta: f64) -> Option<(&'static str, f64)> {
    let wheel = h.wheel?;
    let p = *h.writes.first()?;
    let v = params.float(p);
    Some(match wheel {
        Wheel::Scale => (p, (v * (delta * WHEEL_RATIO).exp()).max(0.0)),
        Wheel::Turn => (p, wrap(v + delta * WHEEL_TURN)),
    })
}

/// The control a parameter is edited by, on every surface: the one
/// patched to it, else its implicit one (as the panel's row has it).
pub fn control_of(registry: &Registry, path: &str, d: &Descriptor) -> Control {
    registry
        .control_for(path, d)
        .cloned()
        .unwrap_or_else(|| implicit(path, d))
}

/// Runs the `controls.change` step on what a gesture changed (`(path,
/// from)` pairs, as the store holds them now), each with its parameter's
/// control. `None`: vetoed. Otherwise the values a hook redirected.
fn change_step(
    store: &Store,
    registry: &Registry,
    steps: &Steps,
    changed: &[(String, f64)],
    cause: Cause,
    device: &str,
) -> Option<Vec<(String, f64)>> {
    let mut redirected = Vec::new();
    for (path, from) in changed {
        let Some(e) = store.get(path) else { continue };
        let Some(to) = e.value.as_f64() else { continue };
        if to == *from {
            continue;
        }
        let control = control_of(registry, path, &e.descriptor);
        let event = ChangeEvent {
            control: control.id.clone(),
            node: crate::flow::node_of(path),
            path: path.clone(),
            from: *from,
            to,
            cause,
            device: device.into(),
        };
        match steps.run(&event) {
            Verdict::Veto => return None,
            Verdict::Redirect(v) => redirected.push((path.clone(), v)),
            Verdict::Go => {}
        }
    }
    Some(redirected)
}

fn write(
    store: &mut Store,
    path: &str,
    v: f64,
    origin: &Origin,
    gesture: GestureId,
) -> Result<(), String> {
    store
        .set(Set::new(path, Change::To(Value::Float(v)), origin.clone()).gesture(gesture))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The values of `params` under `node`'s paths, as the store holds them.
fn values_of(store: &Store, node: &str, params: &[&str]) -> Vec<(String, f64)> {
    params
        .iter()
        .filter_map(|p| {
            let path = node_path(node, p);
            let v = store.get(&path)?.value.as_f64()?;
            Some((path, v))
        })
        .collect()
}

/// Ends a gesture or group through the `controls.change`
/// step: one undo step, or nothing when a hook vetoes it.
fn finish(
    store: &mut Store,
    registry: &Registry,
    steps: &Steps,
    gesture: GestureId,
    before: &[(String, f64)],
    origin: &Origin,
    cause: Cause,
) -> bool {
    let device = match origin {
        Origin::User => "pointer",
        Origin::Agent(_) => "agent",
        Origin::Script(_) => "script",
        Origin::Midi => "midi",
        Origin::Remote(_) => "remote",
    };
    match change_step(store, registry, steps, before, cause, device) {
        None => {
            store.cancel(gesture);
            false
        }
        Some(redirected) => {
            for (path, v) in redirected {
                let _ = write(store, &path, v, origin, gesture);
            }
            store.end(gesture);
            true
        }
    }
}

/// A handle's drag: one store gesture, held against other origins.
#[derive(Debug)]
pub struct Drag {
    gesture: GestureId,
    node: String,
    handle: HandleSpec,
    /// The shape's parameters at the gesture's start.
    start: Params,
    /// The written parameters' paths and values before the gesture.
    before: Vec<(String, f64)>,
    origin: Origin,
}

impl Drag {
    /// Begins a drag of `handle` on `node` (its parameters `start`), as
    /// `origin`: the undo step is `drag <node> <handle>`.
    pub fn begin(
        store: &mut Store,
        gizmo: &Gizmo,
        node: &str,
        handle: &str,
        start: Params,
        origin: Origin,
    ) -> Option<Drag> {
        let h = *gizmo.handle(handle)?;
        let before = values_of(store, node, h.writes);
        let gesture = store.begin_gesture(origin.clone(), &format!("drag {node} {handle}"));
        Some(Drag {
            gesture,
            node: node.into(),
            handle: h,
            start,
            before,
            origin,
        })
    }

    /// The node dragged on.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// The handle dragged.
    pub fn handle(&self) -> &HandleSpec {
        &self.handle
    }

    /// Writes what the handle at `to` shows, inside the gesture; the
    /// store's refusal (another origin holds a parameter) when it has one.
    pub fn update(&self, store: &mut Store, to: Point) -> Result<(), String> {
        for (p, v) in (self.handle.apply)(&self.start, to) {
            write(
                store,
                &node_path(&self.node, p),
                v,
                &self.origin,
                self.gesture,
            )?;
        }
        Ok(())
    }

    /// Ends the drag through the `controls.change` step: one undo step,
    /// or nothing when vetoed. Whether it was kept.
    pub fn commit(self, store: &mut Store, registry: &Registry, steps: &Steps) -> bool {
        finish(
            store,
            registry,
            steps,
            self.gesture,
            &self.before,
            &self.origin,
            Cause::Drag,
        )
    }

    /// Gives the drag up: the values before it come back, no undo step.
    pub fn cancel(self, store: &mut Store) {
        store.cancel(self.gesture);
    }
}

/// A double click on `handle` of `node`: its first parameter back to
/// the handle's reset value, one undo step (`reset <node> <handle>`).
/// Whether anything was written.
pub fn reset(
    store: &mut Store,
    registry: &Registry,
    steps: &Steps,
    gizmo: &Gizmo,
    node: &str,
    handle: &str,
    origin: Origin,
) -> bool {
    let Some(h) = gizmo.handle(handle) else {
        return false;
    };
    let (Some(to), Some(p)) = (h.reset, h.writes.first()) else {
        return false;
    };
    let before = values_of(store, node, &[p]);
    let g = store.begin_group(origin.clone(), &format!("reset {node} {handle}"));
    let _ = write(store, &node_path(node, p), to, &origin, g);
    finish(store, registry, steps, g, &before, &origin, Cause::Reset)
}

/// The wheel over a handle: one group while it turns (the caller ends it
/// after a pause with [`Turn::end`]).
#[derive(Debug)]
pub struct Turn {
    gesture: GestureId,
    before: Vec<(String, f64)>,
    origin: Origin,
    /// When the last unit came, in seconds.
    pub last: f64,
}

impl Turn {
    /// Opens the group of a wheel turn over `handle` of `node`
    /// (`adjust <node> <handle>`).
    pub fn begin(
        store: &mut Store,
        gizmo: &Gizmo,
        node: &str,
        handle: &str,
        origin: Origin,
        now: f64,
    ) -> Option<Turn> {
        let h = gizmo.handle(handle)?;
        h.wheel?;
        let before = values_of(store, node, h.writes);
        let gesture = store.begin_group(origin.clone(), &format!("adjust {node} {handle}"));
        Some(Turn {
            gesture,
            before,
            origin,
            last: now,
        })
    }

    /// Writes `delta` wheel units over `handle` of `node`, from the
    /// shape's parameters now.
    pub fn add(
        &mut self,
        store: &mut Store,
        h: &HandleSpec,
        node: &str,
        params: &Params,
        delta: f64,
        now: f64,
    ) {
        self.last = now;
        if let Some((p, v)) = wheeled(h, params, delta) {
            let _ = write(store, &node_path(node, p), v, &self.origin, self.gesture);
        }
    }

    /// Ends the turn through the `controls.change` step: one undo step.
    pub fn end(self, store: &mut Store, registry: &Registry, steps: &Steps) -> bool {
        finish(
            store,
            registry,
            steps,
            self.gesture,
            &self.before,
            &self.origin,
            Cause::Wheel,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle() -> Params {
        Params::new()
            .with("x", Value::Float(100.0))
            .with("y", Value::Float(50.0))
            .with("radius", Value::Float(20.0))
            .with("feather", Value::Float(5.0))
    }

    #[test]
    fn a_circle_s_handles_write_what_they_show() {
        let g = circle_mask();
        let p = circle();
        let apply = |h: &str, to| (g.handle(h).unwrap().apply)(&p, to);
        assert_eq!(apply("centre", [3.0, 4.0]), [("x", 3.0), ("y", 4.0)]);
        assert_eq!(apply("radius", [130.0, 90.0]), [("radius", 50.0)]);
        assert_eq!(apply("feather", [70.0, 10.0]), [("feather", 30.0)]);
        // Inside the radius: no feather.
        assert_eq!(apply("feather", [101.0, 50.0]), [("feather", 0.0)]);
    }

    #[test]
    fn an_ellipse_s_radii_set_its_rotation() {
        let g = ellipse_mask();
        let p = Params::new()
            .with("x", Value::Float(0.0))
            .with("y", Value::Float(0.0))
            .with("radius_x", Value::Float(40.0))
            .with("radius_y", Value::Float(20.0));
        let apply = |h: &str, to| (g.handle(h).unwrap().apply)(&p, to);
        assert_eq!(
            apply("radius_x", [0.0, 30.0]),
            [("radius_x", 30.0), ("rotation", 90.0)]
        );
        assert_eq!(
            apply("radius_y", [0.0, 30.0]),
            [("radius_y", 30.0), ("rotation", 0.0)]
        );
        assert_eq!(apply("feather", [-50.0, 0.0]), [("feather", 10.0)]);
        let p = p.with("proportional", Value::Bool(true));
        let f = (g.handle("feather").unwrap().apply)(&p, [-60.0, 0.0]);
        assert_eq!(f, [("feather", 20.0)]);
    }

    #[test]
    fn a_gradient_s_handles_write_its_angle_and_band() {
        let g = gradient_mask();
        let p = Params::new()
            .with("x", Value::Float(0.0))
            .with("y", Value::Float(0.0))
            .with("rotation", Value::Float(0.0))
            .with("feather", Value::Float(10.0));
        let apply = |h: &str, to| (g.handle(h).unwrap().apply)(&p, to);
        // Straight up on screen (y down): 90°.
        assert_eq!(apply("rotation", [0.0, -100.0]), [("rotation", 90.0)]);
        // At 0° the band is across: twice the distance from the line.
        assert_eq!(apply("feather", [40.0, -15.0]), [("feather", 30.0)]);
    }

    #[test]
    fn shift_snaps_and_ctrl_moves_finely() {
        let a = snapped(
            Some(Shift::Angle),
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 3.0],
            true,
            false,
        );
        let deg = degrees(a);
        assert!((deg / 15.0 - (deg / 15.0).round()).abs() < 1e-9, "{deg}");
        let ax = snapped(
            Some(Shift::Axes),
            [0.0, 0.0],
            [0.0, 0.0],
            [60.0, 8.0],
            true,
            false,
        );
        assert_eq!(ax, [60.0, 0.0]);
        let fine = snapped(None, [0.0, 0.0], [0.0, 0.0], [100.0, 0.0], false, true);
        assert_eq!(fine, [10.0, 0.0]);
    }

    #[test]
    fn the_wheel_scales_sizes_and_turns_angles() {
        let g = gradient_mask();
        let p = Params::new()
            .with("rotation", Value::Float(179.0))
            .with("feather", Value::Float(10.0));
        let (_, r) = wheeled(g.handle("rotation").unwrap(), &p, 20.0).unwrap();
        assert!((r - -179.0).abs() < 1e-9, "{r}");
        let (_, f) = wheeled(g.handle("feather").unwrap(), &p, 100.0).unwrap();
        assert!((f - 10.0 * 0.2_f64.exp()).abs() < 1e-9);
        assert!(wheeled(g.handle("centre").unwrap(), &p, 1.0).is_none());
    }
}
