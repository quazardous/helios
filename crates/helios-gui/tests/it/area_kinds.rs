//! Panel and area kinds (`docs/design/panel-kinds.md`): every panel
//! declares whether it views, controls or mixes; every edit of the photo
//! made with the pointer comes from an area egui-pop's registry lists as
//! an edit area (`pop.ui.areas`), never from one that only shows or
//! navigates, nor from one nobody declared.
//!
//! The invariant is checked frame by frame ([`Audit`]): the values of
//! the edit written by the user since the last frame (not `view/...`,
//! not transient) must come from the area under the pointer's press,
//! the topmost one registered there, declared `edit`, and, when it names
//! the path it writes, writing that path.

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::core::areas::{self, AreaKind, AreaRecord, AreaSpec};
use egui_pop::model::panels::PanelKind;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Set, Store};
use serde_json::json;

const EV: &str = "node/exposure/exposure";

/// Checks the edit's writes frame by frame against the area registry.
struct Audit {
    /// The store's sequence number at the last check.
    seq: u64,
    /// Where the pointer's button went down, while it is held.
    press: Option<Pos2>,
}

impl Audit {
    fn new(store: &Store) -> Audit {
        Audit {
            seq: store.seq(),
            press: None,
        }
    }

    /// The paths of the edit the user wrote since the last check.
    fn writes(&self, store: &Store) -> Vec<String> {
        store
            .list("")
            .into_iter()
            .filter(|(p, e)| {
                e.seq > self.seq
                    && !p.starts_with("view/")
                    && !e.descriptor.transient
                    && e.origin == Some(Origin::User)
            })
            .map(|(p, _)| p.to_string())
            .collect()
    }

    /// After a frame: the edit's writes since the last check came from an
    /// edit area under `at` (the press), as the registry lists the
    /// frame's areas. Returns the area they came from, if any wrote.
    fn check(
        &mut self,
        ctx: &egui::Context,
        store: &Store,
        at: Option<Pos2>,
    ) -> Result<Option<AreaRecord>, String> {
        let writes = self.writes(store);
        self.seq = store.seq();
        if writes.is_empty() {
            return Ok(None);
        }
        let at = at.ok_or_else(|| format!("{writes:?} written with no pointer press"))?;
        let listed = areas::list(ctx);
        let area = listed
            .iter()
            .rev()
            .find(|a| a.rect.contains(at))
            .ok_or_else(|| format!("{writes:?} written from {at:?}, under no declared area"))?;
        if area.kind != AreaKind::Edit || area.inert {
            return Err(format!(
                "{writes:?} written from `{}`, a {} area{}",
                area.id,
                area.kind.as_str(),
                if area.inert { " (inert)" } else { "" }
            ));
        }
        if let Some(path) = area.path.as_ref().filter(|p| p.contains('/'))
            && !writes.contains(path)
        {
            return Err(format!(
                "`{}` declares it writes {path}, but {writes:?} were written",
                area.id
            ));
        }
        Ok(Some(area.clone()))
    }

    /// One frame of `h`, checked; `pressed` says a button went down
    /// (`Some(true)`) or up (`Some(false)`) at `at` this frame.
    fn step(
        &mut self,
        h: &mut Harness<'_, Shell>,
        at: Pos2,
        pressed: Option<bool>,
    ) -> Option<AreaRecord> {
        if let Some(p) = pressed {
            h.input_mut().events.push(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: p,
                modifiers: Modifiers::NONE,
            });
            if p {
                self.press = Some(at);
            }
        } else {
            h.hover_at(at);
        }
        h.step();
        let from = self.press;
        if pressed == Some(false) {
            self.press = None;
        }
        let store = h.state().values().lock().unwrap();
        self.check(&h.ctx, &store, from)
            .unwrap_or_else(|e| panic!("an edit from an undeclared area: {e}"))
    }

    /// A whole drag from `from` to `to`, every frame checked; the edit
    /// areas the writes came from.
    fn drag(&mut self, h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) -> Vec<AreaRecord> {
        let mut out = Vec::new();
        out.extend(self.step(h, from, None));
        out.extend(self.step(h, from, Some(true)));
        for i in 1..=5 {
            out.extend(self.step(h, from + (to - from) * (i as f32 / 5.0), None));
        }
        out.extend(self.step(h, to, Some(false)));
        out.extend(self.step(h, to, None));
        out
    }
}

/// Frames until the preview has landed and is drawn.
fn settle(h: &mut Harness<'_, Shell>) {
    for _ in 0..4 {
        h.step();
        assert!(
            h.state().preview().wait(std::time::Duration::from_secs(30)),
            "the preview renders"
        );
    }
    h.step();
    h.step();
}

/// A ramp through an exposure, then a circle mask blending it.
fn shell(mask: bool) -> Shell {
    let (w, hgt) = (900u32, 600u32);
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(w, hgt), 4, |x, _, c| match c {
            0..=2 => 0.02 + 0.3 * x as f32 / w as f32,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    if !mask {
        shell.set_view_source(g, exp, "output").unwrap();
        return shell;
    }
    let combine = g
        .add_module(
            "mask",
            ModuleRef::new(helios_module_masks::MaskCombine),
            &Params::new(),
        )
        .unwrap();
    let blend = g
        .add_module(
            "blend",
            ModuleRef::new(helios_module_masks::MaskBlend),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(combine, "mask", blend, "mask").unwrap();
    shell.set_view_source(g, blend, "output").unwrap();
    shell
}

fn harness_on(shell: Shell, panel: &str) -> Harness<'static, Shell> {
    egui_pop::api::palettes::set_hidden(shell.app().handle(), true);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    h.state_mut()
        .run(&format!("{panel}.focus"), json!(null), Origin::User)
        .unwrap();
    settle(&mut h);
    h
}

fn audit(h: &Harness<'_, Shell>) -> Audit {
    Audit::new(&h.state().values().lock().unwrap())
}

fn area(h: &Harness<'_, Shell>, id: &str) -> AreaRecord {
    areas::find(&h.ctx, id).unwrap_or_else(|| {
        let ids: Vec<String> = areas::list(&h.ctx).into_iter().map(|a| a.id).collect();
        panic!("no area `{id}` among {ids:?}")
    })
}

fn f(h: &Harness<'_, Shell>, path: &str) -> f64 {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get(path)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

#[test]
fn every_panel_declares_its_kind() {
    let kinds: Vec<(String, PanelKind)> = helios_panels_with_all()
        .into_iter()
        .map(|p| (p.id.as_str().to_string(), p.kind))
        .collect();
    let of = |id: &str| kinds.iter().find(|(p, _)| p == id).map(|(_, k)| *k);
    for (id, kind) in [
        ("helios.params", PanelKind::Control),
        ("helios.modules", PanelKind::Control),
        ("helios.masks", PanelKind::Control),
        ("helios.graph", PanelKind::Control),
        ("helios.decisions", PanelKind::Control),
        ("helios.workshops", PanelKind::Control),
        ("helios.viewport", PanelKind::Mixed),
        ("helios.scopes", PanelKind::Mixed),
        ("helios.navigator", PanelKind::View),
        ("helios.queue", PanelKind::View),
        ("helios.log", PanelKind::View),
        ("helios.vault", PanelKind::View),
        ("helios.filmstrip", PanelKind::View),
    ] {
        assert_eq!(of(id), Some(kind), "{id}");
    }
    // Every panel Helios declares is in the table, none left to the
    // default by omission.
    for (id, _) in &kinds {
        assert!(
            helios_gui::PANEL_KINDS.iter().any(|(p, _)| p == id),
            "{id} has no declared kind"
        );
    }
}

fn helios_panels_with_all() -> Vec<egui_pop::model::panels::PanelSpec> {
    helios_gui::helios_panels_with(helios_gui::Experimental { workshops: true })
}

#[test]
fn a_parameter_s_slider_edits_from_its_declared_area() {
    let mut h = harness_on(shell(false), "helios.params");
    let slider = area(&h, &format!("param:{EV}"));
    assert_eq!(slider.kind, AreaKind::Edit);
    assert_eq!(slider.path.as_deref(), Some(EV));
    let from = slider.rect.center();
    let mut a = audit(&h);
    let from_areas = a.drag(&mut h, from, from + vec2(50.0, 0.0));
    assert!(f(&h, EV) > 0.0, "the drag moved the exposure");
    assert!(
        from_areas.iter().any(|r| r.id == slider.id),
        "{from_areas:?}"
    );
}

#[test]
fn the_histogram_edits_with_its_grip_and_the_waveform_only_shows() {
    let mut h = harness_on(shell(false), "helios.scopes");
    let hist = areas::list(&h.ctx)
        .into_iter()
        .find(|a| a.id.starts_with("histogram:"))
        .expect("the histogram is declared");
    assert_eq!(hist.kind, AreaKind::Edit);
    assert_eq!(hist.panel.as_deref(), Some("helios.scopes"));
    assert!(!hist.grips.is_empty(), "its grip shows at rest");
    let control = hist.control.clone().unwrap_or_default();
    assert!(control.to_lowercase().contains("exposure"), "{control}");
    let mut a = audit(&h);
    let from = hist.rect.center();
    let from_areas = a.drag(&mut h, from, from + vec2(hist.rect.width() / 8.0, 0.0));
    assert!(f(&h, EV) > 1.0, "the drag moved the exposure");
    assert!(from_areas.iter().any(|r| r.id == hist.id), "{from_areas:?}");
    // The mode buttons navigate; the waveform only shows.
    let mode = area(&h, "scopes.mode.waveform");
    assert_eq!(mode.kind, AreaKind::Navigate);
    let c = mode.rect.center();
    a.step(&mut h, c, None);
    a.step(&mut h, c, Some(true));
    a.step(&mut h, c, Some(false));
    settle(&mut h);
    let wave = area(&h, "scopes.waveform");
    assert_eq!(wave.kind, AreaKind::Display);
    assert!(wave.grips.is_empty());
    // A drag on it writes nothing.
    let before = f(&h, EV);
    let c = wave.rect.center();
    assert!(a.drag(&mut h, c, c + vec2(40.0, 0.0)).is_empty());
    assert!((f(&h, EV) - before).abs() < 1e-12);
}

#[test]
fn a_mask_handle_edits_its_parameter_and_the_image_only_navigates() {
    let mut h = harness_on(shell(true), "helios.viewport");
    h.state_mut()
        .run("mask.add.circle", json!(null), Origin::User)
        .unwrap();
    h.step();
    h.step();
    let handles: Vec<AreaRecord> = areas::list(&h.ctx)
        .into_iter()
        .filter(|a| a.id.starts_with("mask.handle."))
        .collect();
    assert!(!handles.is_empty(), "the circle's handles are declared");
    let radius = handles
        .iter()
        .find(|a| a.path.as_deref().is_some_and(|p| p.ends_with("/radius")))
        .unwrap_or_else(|| panic!("a radius handle among {handles:?}"))
        .clone();
    assert_eq!(radius.kind, AreaKind::Edit);
    assert!(
        radius.control.is_some() && radius.value.is_some(),
        "{radius:?}"
    );
    let path = radius.path.clone().unwrap();
    let before = f(&h, &path);
    let mut a = audit(&h);
    let from = radius.rect.center();
    let from_areas = a.drag(&mut h, from, from + vec2(30.0, 0.0));
    assert!((f(&h, &path) - before).abs() > 1e-6, "the drag moved it");
    assert!(
        from_areas.iter().any(|r| r.id == radius.id),
        "{from_areas:?}"
    );
    // A pan of the image elsewhere moves the view, never the edit.
    let image = area(&h, "viewport.image");
    assert_eq!(image.kind, AreaKind::Navigate);
    let corner = image.rect.left_top() + vec2(30.0, 30.0);
    assert!(a.drag(&mut h, corner, corner + vec2(40.0, 20.0)).is_empty());
}

/// The audit itself: a write from a widget nobody declared, or from one
/// declared as navigate, fails it; the same write from an edit area
/// passes.
#[test]
fn an_edit_from_an_undeclared_area_fails_the_audit() {
    for declared in [None, Some(AreaKind::Navigate), Some(AreaKind::Edit)] {
        let mut store = Store::new();
        store.declare(EV, Descriptor::float("exposure", -4.0, 4.0, 0.0));
        let store = std::sync::Mutex::new(store);
        let mut h = Harness::builder()
            .with_size(vec2(300.0, 100.0))
            .build_ui(|ui| {
                let mut s = store.lock().unwrap();
                let mut v = s.get(EV).unwrap().value.as_f64().unwrap();
                let r = ui.add(egui::Slider::new(&mut v, -4.0..=4.0));
                if r.changed() {
                    s.set(Set::new(EV, Change::To(Value::Float(v)), Origin::User))
                        .unwrap();
                }
                if let Some(kind) = declared {
                    areas::register(ui, &r, AreaSpec::new("test.slider", kind).path(EV));
                }
            });
        h.run();
        let mut a = Audit::new(&store.lock().unwrap());
        let at = h.get_by_role(egui::accesskit::Role::Slider).rect().center();
        h.hover_at(at);
        h.step();
        h.input_mut().events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.hover_at(at + vec2(60.0, 0.0));
        h.step();
        let checked = a.check(&h.ctx, &store.lock().unwrap(), Some(at));
        if declared == Some(AreaKind::Edit) {
            let area = checked.expect("an edit area passes");
            assert_eq!(area.map(|a| a.id).as_deref(), Some("test.slider"));
        } else {
            let e = checked.expect_err("refused");
            assert!(e.contains(EV), "{e}");
        }
    }
}
