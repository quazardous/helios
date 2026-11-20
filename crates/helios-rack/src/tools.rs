//! On-image tools: a shape's handles on the image, hosted by the rack
//! (`docs/design/controls.md`, sections 3.3 and 5.2, slices 12 and 13;
//! `docs/design/ui-responsibilities.md`, section 7, step 10).
//!
//! A shape's case (the circle mask, the gradient mask) is declared here as
//! a [`Tool`]: one [`ToolHandle`] per handle, an `egui_hud` handle of a
//! kind that carries its own maths (a point, a radius, a feather, an
//! angle, a band across a line: [`handle::HandleKind`]) with what Shift,
//! a double click and the wheel do, and the case's part it shows. Each
//! part is the control its parameter already has in the panel (the one
//! patched to it, else its implicit one, [`control_of`]), so a handle and
//! a panel row are two surfaces of one controller:
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
//! Helios's parameters reach the handles' maths through [`ParamsOf`] (a
//! switch reads 1.0 when on). Positions come from the shape's outline
//! (`helios_module_masks`); the viewport (egui-pop's overlay) draws the
//! handles and owns their drags, double clicks and wheel turns, and
//! `egui_hud::render::declare_handle` names their edit areas. This module
//! holds no egui.
//!
//! # A module's on-image tool in 25 lines
//!
//! A module declares its parameters to the store and its tool here, with
//! no egui type, no colour and no size: each handle is a kind from
//! `egui_hud` naming the parameters it reads and writes, and what the
//! tool adds of its own (a feather's range and double-click default,
//! narrower than the parameter's):
//!
//! ```
//! use helios_rack::handle::{Centre, Handle, HandleKind, Look};
//! use helios_rack::tools::{Tool, ToolHandle};
//!
//! let c = Centre::default; // `x`, `y`
//! let radial = Tool::new("radial-exposure", "radial_exposure")
//!     .with(ToolHandle::new(
//!         "center",
//!         Handle::new("centre", HandleKind::point(c())).look(Look::Move),
//!     ))
//!     .with(ToolHandle::new(
//!         "radius",
//!         Handle::new("radius", HandleKind::radius(c(), "radius")),
//!     ))
//!     .with(
//!         ToolHandle::new(
//!             "feather",
//!             Handle::new("feather", HandleKind::feather(c(), "radius", "feather"))
//!                 .reset(Some(25.0)),
//!         )
//!         .range(0.0, 200.0),
//!     );
//! assert_eq!(radial.handle("feather").unwrap().writes(), ["feather"]);
//! ```
//!
//! The rest is the rack's: [`Drag`] makes each drag one undo step that
//! Escape ([`Drag::cancel`]) gives up, [`reset`] and [`Turn`] the double
//! click and the wheel, and the module's render reads the store as the
//! drag writes it. `tests/radial_exposure.rs` drives this example
//! headless.

use egui_hud::handle::{self, AngleParam, Centre, Handle, HandleKind, Look, Reach};
use helios_controls::flow::{Cause, ChangeEvent, Steps, Verdict};
use helios_controls::{Control, HeliosRegistry as _, Registry, implicit};
use helios_core::{Params, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store, node_path};

pub use egui_hud::handle::{FINE, Point, wrap};

/// Helios's parameters as the handles read them: a number as it is, a
/// switch 1.0 when on (0.0 off), and `name.i` the `i`-th value of a list
/// (a path's points).
#[derive(Clone, Copy, Debug)]
pub struct ParamsOf<'a>(pub &'a Params);

impl handle::Params for ParamsOf<'_> {
    fn float(&self, name: &str) -> Option<f64> {
        if let Some(v) = self.0.get(name) {
            return match v {
                Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                v => v.as_f64(),
            };
        }
        let (list, i) = name.rsplit_once('.')?;
        let i: usize = i.parse().ok()?;
        self.0.get(list)?.as_list()?.get(i).copied()
    }
}

/// A handle of a shape's case: the `egui_hud` handle with its maths, and
/// the case's part it shows.
#[derive(Clone, Debug)]
pub struct ToolHandle {
    pub handle: Handle,
    /// The case's part it shows (`center`, `radius`, `feather`,
    /// `rotation`).
    pub part: &'static str,
    /// What its first parameter may be on the image, inside the
    /// parameter's own range: a tool's own bounds (a feather no wider
    /// than this tool wants). Drags, wheel turns and resets stay in it.
    pub range: Option<(f64, f64)>,
}

impl ToolHandle {
    /// `handle`, showing the case's part `part`.
    pub fn new(part: &'static str, handle: Handle) -> Self {
        Self {
            handle,
            part,
            range: None,
        }
    }

    /// Holds its first parameter within `min..=max` on the image.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }

    /// `v` for its first parameter `p`, held in its range; other
    /// parameters as they are.
    fn held(&self, p: &str, v: f64) -> f64 {
        match self.range {
            Some((lo, hi)) if self.writes().first().is_some_and(|w| w == p) => v.clamp(lo, hi),
            _ => v,
        }
    }

    /// What a drag to `to` writes, from the parameters at its start,
    /// held in its range.
    pub fn drag(&self, start: &Params, to: Point) -> Vec<(String, f64)> {
        self.handle
            .drag(&ParamsOf(start), to)
            .into_iter()
            .map(|(p, v)| {
                let v = self.held(&p, v);
                (p, v)
            })
            .collect()
    }

    /// Where it sits on the image, from the shape's parameters.
    pub fn position(&self, params: &Params) -> Point {
        self.handle.position(&ParamsOf(params))
    }

    /// As the shape's outline names it (`centre`, `radius`...).
    pub fn name(&self) -> &str {
        &self.handle.name
    }

    /// The parameters it writes, the first the one it adjusts.
    pub fn writes(&self) -> Vec<String> {
        self.handle.writes()
    }
}

/// A shape's case on the image: its handles.
#[derive(Clone, Debug)]
pub struct Tool {
    /// The case (`circle-mask`).
    pub control: &'static str,
    /// The module whose nodes it edits (`mask_circle`).
    pub module: &'static str,
    pub handles: Vec<ToolHandle>,
}

impl Tool {
    /// The case `control` of `module`'s nodes, with no handle yet.
    pub fn new(control: &'static str, module: &'static str) -> Self {
        Self {
            control,
            module,
            handles: Vec::new(),
        }
    }

    /// With one more handle.
    pub fn with(mut self, handle: ToolHandle) -> Self {
        self.handles.push(handle);
        self
    }

    /// The handle named `name`.
    pub fn handle(&self, name: &str) -> Option<&ToolHandle> {
        self.handles.iter().find(|h| h.name() == name)
    }
}

/// The circle mask's case.
pub const CIRCLE_MASK: &str = "circle-mask";
/// The ellipse mask's case.
pub const ELLIPSE_MASK: &str = "ellipse-mask";
/// The gradient mask's case.
pub const GRADIENT_MASK: &str = "gradient-mask";

/// Every shape case with handles declared here.
pub fn all() -> Vec<Tool> {
    vec![circle_mask(), ellipse_mask(), gradient_mask()]
}

/// The case of a module's nodes, if it has one here.
pub fn of_module(module: &str) -> Option<Tool> {
    all().into_iter().find(|t| t.module == module)
}

/// A shape's centre or anchor: a point at `x`, `y`, drawn as the whole
/// shape's move.
fn centre(part: &'static str) -> ToolHandle {
    ToolHandle::new(
        part,
        Handle::new("centre", HandleKind::point(Centre::default())).look(Look::Move),
    )
}

/// The circle mask: its centre (a point, Shift along the axes), its
/// radius (a size the wheel scales) and its feather past the radius
/// (reset to none, scaled by the wheel).
pub fn circle_mask() -> Tool {
    Tool {
        control: CIRCLE_MASK,
        module: "mask_circle",
        handles: vec![
            centre("center"),
            ToolHandle::new(
                "radius",
                Handle::new("radius", HandleKind::radius(Centre::default(), "radius")),
            ),
            ToolHandle::new(
                "feather",
                Handle::new(
                    "feather",
                    HandleKind::feather(Centre::default(), "radius", "feather"),
                ),
            ),
        ],
    }
}

/// The ellipse mask: its centre (a point, Shift along the axes), each
/// radius with the rotation it sets (Shift to 15° steps around the
/// centre, scaled by the wheel) and its feather past the first radius
/// (in proportion to the larger radius when `proportional` is on; reset
/// to none, scaled by the wheel).
pub fn ellipse_mask() -> Tool {
    let radius = |name: &str, offset: f64| {
        ToolHandle::new(
            "radii",
            Handle::new(
                name,
                HandleKind::Radius {
                    centre: Centre::default(),
                    radius: name.into(),
                    angle: Some(AngleParam::new("rotation").offset(offset)),
                    turns: true,
                },
            ),
        )
    };
    Tool {
        control: ELLIPSE_MASK,
        module: "mask_ellipse",
        handles: vec![
            centre("center"),
            radius("radius_x", 0.0),
            radius("radius_y", 90.0),
            ToolHandle::new(
                "feather",
                Handle::new(
                    "feather",
                    HandleKind::Feather {
                        centre: Centre::default(),
                        radius: "radius_x".into(),
                        feather: "feather".into(),
                        angle: Some(AngleParam::new("rotation")),
                        proportional: Some(handle::Proportional {
                            switch: "proportional".into(),
                            other: "radius_y".into(),
                        }),
                    },
                ),
            ),
        ],
    }
}

/// The gradient mask: its anchor (a point, Shift along the axes), its
/// line's rotation (counter-clockwise on screen; Shift to 15° steps,
/// reset to 0°, turned by the wheel) and its band's width (twice the
/// handle's distance to the line, reset to none, scaled by the wheel).
pub fn gradient_mask() -> Tool {
    let angle = AngleParam::new("rotation").y_up();
    Tool {
        control: GRADIENT_MASK,
        module: "mask_gradient",
        handles: vec![
            centre("anchor"),
            ToolHandle::new(
                "rotation",
                Handle::new(
                    "rotation",
                    HandleKind::angle(Centre::default(), angle.clone(), Reach::Fixed(50.0)),
                )
                .reset(Some(0.0)),
            ),
            ToolHandle::new(
                "feather",
                Handle::new(
                    "feather",
                    HandleKind::line(Centre::default(), angle, "feather"),
                ),
            ),
        ],
    }
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
            node: helios_controls::flow::node_of(path),
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
fn values_of(store: &Store, node: &str, params: &[String]) -> Vec<(String, f64)> {
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
    handle: ToolHandle,
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
        tool: &Tool,
        node: &str,
        handle: &str,
        start: Params,
        origin: Origin,
    ) -> Option<Drag> {
        let h = tool.handle(handle)?.clone();
        let before = values_of(store, node, &h.writes());
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
    pub fn handle(&self) -> &ToolHandle {
        &self.handle
    }

    /// Writes what the handle at `to` shows, inside the gesture; the
    /// store's refusal (another origin holds a parameter) when it has one.
    pub fn update(&self, store: &mut Store, to: Point) -> Result<(), String> {
        for (p, v) in self.handle.drag(&self.start, to) {
            write(
                store,
                &node_path(&self.node, &p),
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
    tool: &Tool,
    node: &str,
    handle: &str,
    origin: Origin,
) -> bool {
    let Some((h, (p, to))) = tool
        .handle(handle)
        .and_then(|h| Some((h, h.handle.reset_value()?)))
    else {
        return false;
    };
    let to = h.held(&p, to);
    let before = values_of(store, node, std::slice::from_ref(&p));
    let g = store.begin_group(origin.clone(), &format!("reset {node} {handle}"));
    let _ = write(store, &node_path(node, &p), to, &origin, g);
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
        tool: &Tool,
        node: &str,
        handle: &str,
        origin: Origin,
        now: f64,
    ) -> Option<Turn> {
        let h = tool.handle(handle)?;
        h.handle.wheel?;
        let before = values_of(store, node, &h.writes());
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
        h: &ToolHandle,
        node: &str,
        params: &Params,
        delta: f64,
        now: f64,
    ) {
        self.last = now;
        if let Some((p, v)) = h.handle.wheeled(&ParamsOf(params), delta) {
            let v = h.held(&p, v);
            let _ = write(store, &node_path(node, &p), v, &self.origin, self.gesture);
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
    use egui_hud::handle::{Snap, Wheel};

    fn circle() -> Params {
        Params::new()
            .with("x", Value::Float(100.0))
            .with("y", Value::Float(50.0))
            .with("radius", Value::Float(20.0))
            .with("feather", Value::Float(5.0))
    }

    fn drag(t: &Tool, h: &str, p: &Params, to: Point) -> Vec<(String, f64)> {
        t.handle(h).unwrap().handle.drag(&ParamsOf(p), to)
    }

    fn pairs(w: &[(String, f64)]) -> Vec<(&str, f64)> {
        w.iter().map(|(k, v)| (k.as_str(), *v)).collect()
    }

    #[test]
    fn helios_params_read_numbers_switches_and_list_values() {
        use egui_hud::handle::Params as _;
        let p = Params::new()
            .with("radius", Value::Int(3))
            .with("proportional", Value::Bool(true))
            .with("off", Value::Bool(false))
            .with("points", Value::List(vec![1.0, 2.0, 3.0]));
        let r = ParamsOf(&p);
        assert_eq!(r.float("radius"), Some(3.0));
        assert_eq!(r.float("proportional"), Some(1.0));
        assert_eq!(r.float("off"), Some(0.0));
        assert_eq!(r.float("points.2"), Some(3.0));
        assert_eq!(r.float("points.3"), None);
        assert_eq!(r.float("missing"), None);
    }

    #[test]
    fn a_circle_s_handles_write_what_they_show() {
        let t = circle_mask();
        let p = circle();
        assert_eq!(
            pairs(&drag(&t, "centre", &p, [3.0, 4.0])),
            [("x", 3.0), ("y", 4.0)]
        );
        assert_eq!(
            pairs(&drag(&t, "radius", &p, [130.0, 90.0])),
            [("radius", 50.0)]
        );
        assert_eq!(
            pairs(&drag(&t, "feather", &p, [70.0, 10.0])),
            [("feather", 30.0)]
        );
        // Inside the radius: no feather.
        assert_eq!(
            pairs(&drag(&t, "feather", &p, [101.0, 50.0])),
            [("feather", 0.0)]
        );
    }

    #[test]
    fn an_ellipse_s_radii_set_its_rotation() {
        let t = ellipse_mask();
        let p = Params::new()
            .with("x", Value::Float(0.0))
            .with("y", Value::Float(0.0))
            .with("radius_x", Value::Float(40.0))
            .with("radius_y", Value::Float(20.0));
        assert_eq!(
            pairs(&drag(&t, "radius_x", &p, [0.0, 30.0])),
            [("radius_x", 30.0), ("rotation", 90.0)]
        );
        assert_eq!(
            pairs(&drag(&t, "radius_y", &p, [0.0, 30.0])),
            [("radius_y", 30.0), ("rotation", 0.0)]
        );
        assert_eq!(
            pairs(&drag(&t, "feather", &p, [-50.0, 0.0])),
            [("feather", 10.0)]
        );
        // A switch reads 1.0: the feather in proportion to the larger radius.
        let p = p.with("proportional", Value::Bool(true));
        assert_eq!(
            pairs(&drag(&t, "feather", &p, [-60.0, 0.0])),
            [("feather", 20.0)]
        );
    }

    #[test]
    fn a_gradient_s_handles_write_its_angle_and_band() {
        let t = gradient_mask();
        let p = Params::new()
            .with("x", Value::Float(0.0))
            .with("y", Value::Float(0.0))
            .with("rotation", Value::Float(0.0))
            .with("feather", Value::Float(10.0));
        // Straight up on screen (y down): 90°.
        assert_eq!(
            pairs(&drag(&t, "rotation", &p, [0.0, -100.0])),
            [("rotation", 90.0)]
        );
        // At 0° the band is across: twice the distance from the line.
        assert_eq!(
            pairs(&drag(&t, "feather", &p, [40.0, -15.0])),
            [("feather", 30.0)]
        );
        // Turned 30°, the band's normal turns with it.
        let p = p.with("rotation", Value::Float(30.0));
        let r = 30f64.to_radians();
        let n = [-r.sin(), -r.cos()];
        let w = drag(&t, "feather", &p, [10.0 * n[0], 10.0 * n[1]]);
        assert!((w[0].1 - 20.0).abs() < 1e-9, "{w:?}");
    }

    #[test]
    fn the_cases_keep_their_gestures() {
        let g = |t: &Tool, h: &str| {
            let h = &t.handle(h).unwrap().handle;
            (h.look, h.snap, h.reset, h.wheel)
        };
        let c = circle_mask();
        assert_eq!(g(&c, "centre"), (Look::Move, Some(Snap::Axes), None, None));
        assert_eq!(
            g(&c, "radius"),
            (Look::Size, None, None, Some(Wheel::Scale))
        );
        assert_eq!(
            g(&c, "feather"),
            (Look::Feather, None, Some(0.0), Some(Wheel::Scale))
        );
        let e = ellipse_mask();
        assert_eq!(
            g(&e, "radius_y"),
            (Look::Size, Some(Snap::Angle), None, Some(Wheel::Scale))
        );
        assert_eq!(e.handle("radius_x").unwrap().part, "radii");
        let gr = gradient_mask();
        assert_eq!(
            g(&gr, "rotation"),
            (
                Look::Rotate,
                Some(Snap::Angle),
                Some(0.0),
                Some(Wheel::Turn)
            )
        );
        assert_eq!(
            g(&gr, "feather"),
            (Look::Feather, None, Some(0.0), Some(Wheel::Scale))
        );
        assert_eq!(gr.handle("centre").unwrap().part, "anchor");
    }

    #[test]
    fn the_wheel_scales_sizes_and_turns_angles() {
        let t = gradient_mask();
        let p = Params::new()
            .with("rotation", Value::Float(179.0))
            .with("feather", Value::Float(10.0));
        let wheel = |h: &str, d| t.handle(h).unwrap().handle.wheeled(&ParamsOf(&p), d);
        let (_, r) = wheel("rotation", 20.0).unwrap();
        assert!((r - -179.0).abs() < 1e-9, "{r}");
        let (_, f) = wheel("feather", 100.0).unwrap();
        assert!((f - 10.0 * 0.2_f64.exp()).abs() < 1e-9);
        assert!(wheel("centre", 1.0).is_none());
    }
}
