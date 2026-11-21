//! The storage's settings, declared here: the rules that keep places out
//! of the image map ([`crate::exclude`]).

use helios_config::{Concern, Kind, Setting};

/// The image map's exclusion rules.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "system.map.exclude",
        concern: Concern::Machine,
        label: "Places never recorded",
        about: "Rules added to the image map's blacklist, as a JSON list: each `{\"glob\": \"**/Private/**\"}` matches the absolute path, or with `\"volume\": \"<id>\"` the path inside that volume; a glob starting with `!` re-includes what a default excludes. The last rule that matches wins.",
        kind: Kind::Text { default: "" },
    },
    Setting {
        key: "system.map.defaults",
        concern: Concern::Machine,
        label: "Default blacklist",
        about: "Whether the image map leaves out the system's folders, Helios's own data, caches, build trees, hidden folders, trash and other photo applications' libraries; off keeps only the rules above.",
        kind: Kind::Toggle {
            default: true,
            on: "on",
            off: "off",
        },
    },
];
