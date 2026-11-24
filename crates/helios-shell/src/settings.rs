//! The desktop's settings: the interface's choices and the desktop's
//! memory budgets (the photos' kept states, the grids' thumbnail
//! textures). helios-gui reads them; they are declared here, in the
//! desktop shell's toolkit-free crate, because the node keeps the
//! person's layer that holds them and must read it whole, and helios-gui
//! sits above the node.

use helios_config::{Concern, Kind, Setting};

/// The desktop's settings.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "system.cache.states",
        concern: Concern::Machine,
        label: "Kept photo states",
        about: "Memory the photos' computed states may hold (the shown photo's full-size step, kept so the fit and a zoom do not compute it again, and the recent photos'); 0 means 12.5 % of the machine's memory, between 1 and 4 GiB.",
        kind: Kind::Number {
            min: 0.0,
            max: 1_048_576.0,
            default: 0.0,
            unit: "MiB",
            integer: true,
        },
    },
    Setting {
        key: "system.cache.textures",
        concern: Concern::Machine,
        label: "Thumbnail textures",
        about: "Graphics memory the grids' thumbnails may hold; beyond it, the thumbnails out of view are freed, those seen longest ago first, and read back from the thumbnail cache when they come back.",
        kind: Kind::Number {
            min: 16.0,
            max: 65_536.0,
            default: 512.0,
            unit: "MiB",
            integer: true,
        },
    },
    Setting {
        key: "interface.panels.queue_when_empty",
        concern: Concern::User,
        label: "Queue panel without a queue",
        about: "What the queue panel does when the selected node has no queue.",
        kind: Kind::Toggle {
            default: true,
            on: "shown, empty",
            off: "hidden",
        },
    },
    Setting {
        key: "interface.graph.zoom",
        concern: Concern::User,
        label: "Graph view opens on",
        about: "Whether the graph view opens on the overview or the full detail.",
        kind: Kind::Choice {
            options: &["overview", "detail"],
            default: "detail",
        },
    },
    Setting {
        key: "import.show_in_library",
        concern: Concern::User,
        label: "After an import, show it in the Library",
        about: "When an import you started ends, the Library shows its photos, all of them selected. Never for automatic imports; while you edit a photo, its notice offers it instead.",
        kind: Kind::Toggle {
            default: true,
            on: "on",
            off: "off",
        },
    },
    Setting {
        key: "interface.experimental.workshops",
        concern: Concern::User,
        label: "Workshops",
        about: "Experimental: the Workshops panel and commands, to find, apply and aim macros by what they are for. May change; what it saves stays a macro. Takes effect at the next start.",
        kind: Kind::Toggle {
            default: false,
            on: "on",
            off: "off",
        },
    },
    Setting {
        key: "library.double_click",
        concern: Concern::User,
        label: "Double click shows the photo in",
        about: "Where a double click or Enter in a gallery shows the photo: the primary surface (the Image), the surface you used last, the surface in the same window as the gallery, a new Compare each time, or ask each time. With two surfaces open, the one it reaches wears a target mark.",
        kind: Kind::Choice {
            options: &[
                "primary",
                "last_focused",
                "same_window",
                "always_new",
                "ask",
            ],
            default: "primary",
        },
    },
];
