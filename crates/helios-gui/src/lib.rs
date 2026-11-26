//! Helios's desktop: an egui-pop application around Helios's panels (on
//! egui-pop's workbench), its commands run by Helios's kernel, and the
//! framework's own pictures and recordings of it.

/// The time one pass of an interface-thread point's listeners should take
/// (bus-contract's `PointDecl::budget`): a pass over it is said as
/// `bus.overBudget` and logged, never failed. 4 ms leaves most of a
/// 60 Hz frame to drawing.
pub const INTERFACE_PASS_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

pub mod account_gone;
pub mod activity;
pub mod agent_preset;
pub mod agent_state;
pub mod app_quit;
pub use helios_storage_ui::backup_end;
pub mod caches;
pub mod calibrate;
pub mod caption;
pub mod catalogue;
pub mod clipboard;
pub mod command_policy;
pub mod comparison;
pub mod config;
pub mod contexts;
pub mod controls;
pub mod decisions;
pub mod destructive;
pub mod dialogs;
pub mod display;
pub mod display_surfaces;
pub mod duplicates;
pub mod edit_marks;
pub mod edit_snapshots;
pub mod ending;
pub mod engine_source;
pub mod event_loop;
pub mod export;
pub mod export_start;
pub mod graph;
pub mod handle_snap;
pub mod head_render;
pub mod history;
pub mod host;
pub mod i18n;
pub mod image_map;
pub mod image_open;
pub mod import_browser;
pub mod import_end;
pub mod import_remote;
pub mod import_scan;
pub mod importers;
pub mod jobs;
pub mod keymaps;
pub mod keys;
pub mod library;
pub mod library_changed;
pub mod library_history;
pub mod library_views;
pub mod library_write;
pub mod log_panel;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod macro_interface;
pub mod mask_display;
pub mod mask_tree;
pub mod masks;
pub mod modules;
pub mod native_menu;
pub mod navigator;
/// The local node the desktop embeds, from the node's crate.
pub use helios_node_core::node;
pub mod node_jobs;
/// The node's volume watch, from the node's crate.
pub use helios_node_core::volumes as node_volumes;
pub mod forget;
pub mod node_events;
pub mod params;
pub mod persist;
pub mod photo_panels;
pub mod platform;
pub mod plugin_controls;
pub mod plugin_refusals;
pub mod plugin_tools;
mod plugins;
pub mod pointer_choice;
pub mod preview;
pub mod privacy;
pub mod queue;
pub mod quick_open;
pub mod quit;
pub mod rebuilt_history;
pub mod recent_filters;
pub mod relay;
pub mod report;
pub mod reset;
pub mod rule_editor;
pub mod rule_editor_ui;
pub mod schemas;
pub mod scopes;
pub mod settings_changed;
pub mod shell;
pub mod smart_album;
pub mod snapshot;
pub mod splash;
pub mod stack_review;
pub mod stall;
pub mod start;
pub mod status_items;
pub mod workflow_queue;
pub use helios_storage_ui::read as storage_read;
pub mod templates;
pub mod theme;
pub mod theme_editor;
pub mod thumb_textures;
pub mod thumbnails;
pub mod tips;
pub mod values_changed;
pub mod viewport;
pub mod warm;
pub mod window_commands;
mod windows;
mod workbench;
pub mod workshop_run;
pub mod workshops;
pub mod workspace_applied;

pub use shell::{Shell, headless, name_rect};
pub use workbench::{IMAGE_FIRST, area_named, beside_the_end_side, gather_palettes, image_first};

use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::{Area, Instances, PanelKind, PanelSpec};

/// Every fact kind the desktop's buses carry, by the table of the crate
/// that publishes it: what `main` declares on them ([`helios_core::Bus::declare`]).
/// A node's own kinds, relayed, were checked by the node.
pub const FACTS: &[&[helios_core::FactDecl]] = &[
    helios_core::engine::FACTS,
    helios_core::scheduler::FACTS,
    helios_core::jobs::FACTS,
    helios_decision::FACTS,
    helios_export::FACTS,
    helios_library::FACTS,
    helios_node_core::FACTS,
    helios_values::FACTS,
    helios_vault::FACTS,
    helios_vault_sqlite::FACTS,
    host::FACTS,
    relay::FACTS,
];

/// A bus of the desktop, its fact kinds declared ([`FACTS`]): a kind
/// published that no table holds stops a debug build.
pub fn bus() -> std::sync::Arc<helios_core::Bus> {
    let bus = helios_core::Bus::new();
    bus.declare(FACTS.iter().copied());
    std::sync::Arc::new(bus)
}

/// The panels that float over the image as palettes (egui-pop's
/// `PanelSpec::palette`): floating in a new layout, docked from their
/// header or `<panel>.dock`, floated again from their tab's menu or
/// `<panel>.float`, hidden and shown again with Tab.
pub const FLOATABLE: [&str; 3] = ["helios.masks", "helios.scopes", "helios.navigator"];

/// The panels that follow the photo shown: from the moment another photo
/// is selected until their new data lands, the workbench shows them as
/// not current ([`photo_panels`]).
pub const FOLLOWS_PHOTO: [&str; 2] = ["helios.scopes", "helios.navigator"];

/// The icon of each of Helios's panels, by name in egui-pop's icon
/// registry: what the activity bar and the folded side bars show
/// (`PanelSpec::icon`). A Phosphor glyph (`phosphor:<name>`) where
/// egui-pop's built-in set has none.
pub const PANEL_ICONS: &[(&str, &str)] = &[
    ("helios.viewport", "phosphor:image"),
    ("helios.graph", "phosphor:graph"),
    ("helios.vault", "phosphor:images-square"),
    ("helios.filters", "phosphor:funnel"),
    ("helios.modules", "phosphor:puzzle-piece"),
    ("helios.masks", "phosphor:selection"),
    ("helios.navigator", "phosphor:compass"),
    ("helios.params", "phosphor:sliders-horizontal"),
    ("helios.scopes", "phosphor:chart-bar"),
    ("helios.filmstrip", "phosphor:film-strip"),
    ("helios.queue", "phosphor:queue"),
    ("helios.log", "phosphor:scroll"),
    ("helios.compare", "phosphor:square-split-horizontal"),
    ("helios.stacks", "phosphor:stack"),
    ("helios.duplicates", "copy"),
    ("helios.decisions", "phosphor:seal-question"),
    (workshops::PANEL, "phosphor:flask"),
];

/// What each panel does to the photo's edit (`PanelSpec::kind`,
/// `docs/design/panel-kinds.md`): a **control** panel is there to edit
/// it, a **mixed** one shows and holds an edit area or two (the Image's
/// mask handles, the Scopes' histogram drag), a **view** one only shows
/// and navigates (the Library's marks are catalogue data, not the
/// render). egui-pop's chrome marks the panels that edit.
pub const PANEL_KINDS: &[(&str, PanelKind)] = &[
    ("helios.viewport", PanelKind::Mixed),
    ("helios.graph", PanelKind::Control),
    ("helios.vault", PanelKind::View),
    ("helios.filters", PanelKind::View),
    ("helios.modules", PanelKind::Control),
    ("helios.masks", PanelKind::Control),
    ("helios.navigator", PanelKind::View),
    ("helios.params", PanelKind::Control),
    ("helios.scopes", PanelKind::Mixed),
    ("helios.filmstrip", PanelKind::View),
    ("helios.queue", PanelKind::View),
    ("helios.log", PanelKind::View),
    ("helios.compare", PanelKind::View),
    ("helios.stacks", PanelKind::View),
    ("helios.duplicates", PanelKind::View),
    ("helios.decisions", PanelKind::Control),
    (workshops::PANEL, PanelKind::Control),
];

/// The panels that open as many times as asked (egui-pop's panel
/// instances, `<panel>#<n>`): the Library's Results, each new one a
/// Library view of its own ([`library_views`]), and Compare, each new
/// one a comparison of its own ([`display_surfaces`]). The others are
/// single.
pub const MULTIPLE: &[&str] = &[library::RESULTS_PANEL, display_surfaces::COMPARE];

/// The panels Helios declares today, with the areas each may be moved to
/// (egui-pop's placement rules); each shows a placeholder until its
/// widget lands. Every panel may be detached into a window of its own;
/// the [`FLOATABLE`] ones are palettes.
/// The culling panels (Compare, Stacks, Duplicates) and the Navigator
/// start closed: a template's layout, or the user, opens them.
pub fn helios_panels() -> Vec<PanelSpec> {
    helios_panels_with(Experimental::default())
}

/// The experimental features switched on for this run (read once, when
/// the desktop starts): each declares its panel only when on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Experimental {
    /// Workshops (`interface.experimental.workshops`, see
    /// [`workshops`]).
    pub workshops: bool,
}

/// [`helios_panels`], with the panels of the experimental features
/// switched on: the Workshops panel (end side, closed until opened).
pub fn helios_panels_with(experimental: Experimental) -> Vec<PanelSpec> {
    use Area::{Bottom, Center, EndSide, StartSide};
    let sides = [StartSide, EndSide, Bottom];
    let anywhere = [Center, StartSide, EndSide, Bottom];
    [
        ("helios.viewport", "Image", Center, &[Center][..], true),
        ("helios.graph", "Graph", Center, &[Center][..], true),
        ("helios.vault", "Library", StartSide, &anywhere[..], true),
        // Anywhere, as its Results: docked above them in the middle,
        // it draws itself as a bar (#5078 A2).
        ("helios.filters", "Filters", StartSide, &anywhere[..], true),
        ("helios.modules", "Modules", StartSide, &sides[..], true),
        ("helios.masks", "Masks", StartSide, &sides[..], true),
        (
            "helios.navigator",
            "Navigator",
            StartSide,
            &sides[..],
            false,
        ),
        ("helios.params", "Parameters", EndSide, &sides[..], true),
        ("helios.scopes", "Scopes", EndSide, &sides[..], true),
        ("helios.filmstrip", "Filmstrip", Bottom, &sides[..], true),
        ("helios.queue", "Queue", Bottom, &[][..], true),
        ("helios.log", "Log", Bottom, &[][..], true),
        ("helios.compare", "Compare", Center, &anywhere[..], false),
        ("helios.stacks", "Stacks", EndSide, &sides[..], false),
        (
            "helios.duplicates",
            "Duplicates",
            EndSide,
            &sides[..],
            false,
        ),
        ("helios.decisions", "Decisions", EndSide, &sides[..], false),
        (workshops::PANEL, "Workshops", EndSide, &sides[..], false),
    ]
    .into_iter()
    .filter(|(id, ..)| *id != workshops::PANEL || experimental.workshops)
    .map(|(id, title, area, areas, open)| {
        let id = PanelId::new(id).expect("Helios's panel ids are valid");
        let floats = FLOATABLE.contains(&id.as_str());
        let multiple = MULTIPLE.contains(&id.as_str());
        let icon = PANEL_ICONS.iter().find(|(p, _)| *p == id.as_str());
        let kind = PANEL_KINDS
            .iter()
            .find(|(p, _)| *p == id.as_str())
            .map_or(PanelKind::View, |(_, k)| *k);
        let mut spec = PanelSpec::new(id, title, area).only_in(areas).kind(kind);
        if floats {
            spec = spec.palette();
        }
        if let Some((_, icon)) = icon {
            spec = spec.icon(icon);
        }
        if !open {
            spec = spec.closed();
        }
        if multiple {
            spec = spec.instances(Instances::Multiple);
        }
        // The sizes Helios's areas open at.
        match area {
            Center => spec,
            Bottom => spec.default_size(200.0),
            _ => spec.default_size(260.0),
        }
    })
    .collect()
}
