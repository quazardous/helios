//! Helios's panels on egui-pop's workbench (`egui_pop::api::workbench`):
//! each declared panel is drawn in its area as tabs and splits over
//! `egui_dock`, under its rules (`PanelSpec`: the areas it may be in, its
//! sizes, whether it detaches into an auxiliary window), and the layout
//! changes only through egui-pop's steps (`workbench.move`,
//! `workbench.focus`...). Helios gives each panel its content and offers
//! its layouts as arrangement templates.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui::Ui;
use egui_pop::api::{Ctx, Plugin, PluginError};
use egui_pop::model::ids::{PanelId, PluginId};
use egui_pop::model::layout::{Anchor, AnchorEdge, Arrangement, FloatingGroup, Layout, Node};
use egui_pop::model::panels::{Area, PanelSpec};

use crate::name_rect;

/// Draws a panel's content.
pub type Content = Box<dyn FnMut(&mut Ui)>;

/// Each panel's content, by panel id; a panel without one shows a
/// placeholder.
pub(crate) type Contents = Rc<RefCell<BTreeMap<String, Content>>>;

pub const PANELS: PluginId = PluginId::from_static("helios.workbench");

/// The image-first arrangement: the image, the filmstrip under it, every
/// other panel closed (small screens, culling).
pub const IMAGE_FIRST: &str = "Image";

/// The library's arrangement: the vault's grid in the centre, the stacks
/// and duplicates beside it, every other panel closed.
pub const LIBRARY: &str = helios_template::contexts::LIBRARY_ARRANGEMENT;

/// The workspace's arrangement: its network in the centre, the modules
/// and parameters beside it, every other panel closed.
pub const NETWORK: &str = helios_template::contexts::NETWORK_ARRANGEMENT;

/// The darkroom's arrangement: the workspace's default layout (each
/// panel open by default in its own area), the image shown in the
/// centre.
pub const DARKROOM: &str = helios_template::contexts::DARKROOM_ARRANGEMENT;

/// Helios's panels, declared to egui-pop's workbench.
pub(crate) struct Panels {
    pub specs: Vec<PanelSpec>,
    pub contents: Contents,
}

impl Plugin for Panels {
    fn id(&self) -> PluginId {
        PANELS
    }

    fn title(&self) -> String {
        "Helios areas".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        for spec in &self.specs {
            let contents = self.contents.clone();
            let (id, title) = (spec.id.to_string(), spec.title.clone());
            ctx.panel(spec.clone(), move |ui, _| {
                // Named for pictures (`gui.snapshot`) and tests.
                name_rect(ui.ctx(), &id, ui.max_rect());
                let before = ui.memory(egui::Memory::focused);
                match contents.borrow_mut().get_mut(&id) {
                    Some(c) => c(ui),
                    None => placeholder(ui, &id, &title),
                }
                // A widget of the panel took the keyboard: kept through
                // the rest of the pass (`keep_focus_taken`).
                let after = ui.memory(egui::Memory::focused);
                if let Some(taken) = after.filter(|_| after != before) {
                    let window = ui.ctx().viewport_id();
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(focus_taken_id(), (window, taken)));
                }
            })?;
        }
        ctx.arrangement(IMAGE_FIRST, image_first(&self.specs));
        ctx.arrangement(LIBRARY, library(&self.specs));
        ctx.arrangement(NETWORK, network(&self.specs));
        ctx.arrangement(DARKROOM, darkroom(&self.specs));
        Ok(())
    }
}

fn focus_taken_id() -> egui::Id {
    egui::Id::new("helios.focus_taken")
}

/// Gives the keyboard back to the widget of a panel that took it this
/// pass, when something drawn after the panel dropped it (call it once
/// the workbench is drawn).
///
/// egui-pop's palettes read the focused widget's response after drawing
/// their panels (Escape there gives the keyboard back), and building a
/// response surrenders the focus of a widget the click did not hover: a
/// tree takes the focus when one of its rows is clicked, so in a palette
/// it lost it in the same pass and its keys (Delete, F2) went nowhere.
/// Docked, nothing reads it after the panel and the focus stays.
pub(crate) fn keep_focus_taken(ctx: &egui::Context) {
    let Some((window, taken)) = ctx.data_mut(|d| {
        let taken = d.get_temp::<(egui::ViewportId, egui::Id)>(focus_taken_id());
        d.remove::<(egui::ViewportId, egui::Id)>(focus_taken_id());
        taken
    }) else {
        return;
    };
    // Only a focus dropped in this window, never one another widget took
    // since.
    if window == ctx.viewport_id() && ctx.memory(egui::Memory::focused).is_none() {
        ctx.memory_mut(|m| m.request_focus(taken));
    }
}

/// A panel waiting for its widget: its title, in the interface's
/// language.
fn placeholder(ui: &mut Ui, id: &str, title: &str) {
    let title = egui_pop::core::strings::panel_title(ui.ctx(), id, title);
    ui.centered_and_justified(|ui| {
        ui.weak(title);
    });
}

/// The image-first layout of these panels: the image alone in the
/// centre, the filmstrip below, the rest closed.
pub fn image_first(specs: &[PanelSpec]) -> Layout {
    let id = |s: &str| PanelId::new(s).expect("Helios's panel ids are valid");
    let shown = ["helios.viewport", "helios.filmstrip"];
    Layout {
        areas: vec![
            (Area::Center, Node::tab(id("helios.viewport"))),
            (Area::Bottom, Node::tab(id("helios.filmstrip"))),
        ],
        hidden: specs
            .iter()
            .filter(|s| !shown.contains(&s.id.as_str()))
            .map(|s| s.id.clone())
            .collect(),
        ..Layout::default()
    }
}

/// A layout showing these panels where they go, every other one closed.
fn only(specs: &[PanelSpec], shown: &[(Area, &str)]) -> Layout {
    let id = |s: &str| PanelId::new(s).expect("Helios's panel ids are valid");
    Layout {
        areas: shown.iter().map(|(a, p)| (*a, Node::tab(id(p)))).collect(),
        hidden: specs
            .iter()
            .filter(|s| !shown.iter().any(|(_, p)| *p == s.id.as_str()))
            .map(|s| s.id.clone())
            .collect(),
        ..Layout::default()
    }
}

/// The darkroom layout of these panels (see [`DARKROOM`]): each panel
/// open by default in its own area, as tabs in declaration order, the
/// image shown in the centre; the others closed.
pub fn darkroom(specs: &[PanelSpec]) -> Layout {
    let image = "helios.viewport";
    let areas = [Area::Center, Area::StartSide, Area::EndSide, Area::Bottom]
        .into_iter()
        .filter_map(|area| {
            let panels: Vec<PanelId> = specs
                .iter()
                .filter(|s| s.open && s.area == area)
                .map(|s| s.id.clone())
                .collect();
            let active = panels.iter().position(|p| p.as_str() == image).unwrap_or(0);
            (!panels.is_empty()).then_some((area, Node::Tabs { panels, active }))
        })
        .collect();
    Layout {
        areas,
        hidden: specs
            .iter()
            .filter(|s| !s.open)
            .map(|s| s.id.clone())
            .collect(),
        ..Layout::default()
    }
}

/// The library layout of these panels (see [`LIBRARY`]).
pub fn library(specs: &[PanelSpec]) -> Layout {
    only(
        specs,
        &[
            (Area::Center, "helios.vault"),
            (Area::EndSide, "helios.stacks"),
        ],
    )
}

/// The workspace's network layout of these panels (see [`NETWORK`]).
pub fn network(specs: &[PanelSpec]) -> Layout {
    only(
        specs,
        &[
            (Area::Center, "helios.graph"),
            (Area::StartSide, "helios.modules"),
            (Area::EndSide, "helios.params"),
        ],
    )
}

/// Where the palettes a new layout gathers stick at first: the
/// workbench's end edge, at the top.
const GATHERED: Anchor = Anchor {
    x: AnchorEdge::End,
    y: AnchorEdge::Start,
    offset: [0.0, 0.0],
};

/// Gathers a new layout's palettes where Helios opens them (2026-10-28):
/// stacked in one group against the end edge, at the top, out of the
/// image's way, as an image editor's palettes start against the screen's
/// edge. The group is anchored there (egui-pop's `Anchor`), so it keeps
/// to that edge as the window is resized, and moves, docks or comes
/// apart as any group does. Once the areas are drawn, the shell moves it
/// beside the end side bar ([`beside_the_end_side`]).
///
/// egui-pop floats each palette of a new layout alone, cascaded from the
/// workbench's top left (`FloatingGroup::cascade`), over the image: a
/// layout whose palettes are exactly that is taken for a new one. Any
/// other (loaded, arranged, moved by the user) is left as it is. Gives
/// the group's first panel when it gathered them.
pub fn gather_palettes(layout: &mut Layout) -> Option<PanelId> {
    let cascaded = !layout.floating.is_empty()
        && layout.floating.iter().enumerate().all(|(n, g)| {
            g.is_lone()
                && g.pos == FloatingGroup::cascade(n)
                && g.anchor.is_none()
                && !g.pinned
                && g.size.is_none()
        });
    if !cascaded {
        return None;
    }
    let members: Vec<_> = layout.floating.drain(..).flat_map(|g| g.members).collect();
    let lead = members[0].panel.clone();
    layout.floating.push(FloatingGroup {
        members,
        arrangement: Arrangement::Stack,
        active: 0,
        // Unused on the anchored axes: the edges place it.
        pos: [0.0, 0.0],
        pinned: false,
        anchor: GATHERED,
        size: None,
    });
    Some(lead)
}

/// Moves the palettes [`gather_palettes`] put at the workbench's end edge
/// to `offset` points from it, the end side bar's width: they then lie
/// against the image's end edge, the end side bar left in sight. A group
/// moved, anchored elsewhere or gone since is left as it is. Says whether
/// it changed the layout.
pub fn beside_the_end_side(layout: &mut Layout, lead: &PanelId, offset: f32) -> bool {
    match layout.floating_mut(lead) {
        Some(g) if g.anchor == GATHERED && offset > 0.0 => {
            g.anchor.offset[0] = offset;
            true
        }
        _ => false,
    }
}

/// An area by its name: egui-pop's (`center`, `start_side`, `end_side`,
/// `bottom`), or the one Helios's layouts used before them (`editors`,
/// `primary_side_bar`, `secondary_side_bar`, `bottom_panel`).
pub fn area_named(name: &str) -> Option<Area> {
    egui_pop::api::workbench::area_from(name).or(match name {
        "editors" => Some(Area::Center),
        "primary_side_bar" => Some(Area::StartSide),
        "secondary_side_bar" => Some(Area::EndSide),
        "bottom_panel" => Some(Area::Bottom),
        _ => None,
    })
}
