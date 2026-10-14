//! Helios's places and modes on egui-pop's context stack (its ADR 0012),
//! declared here, without egui, so the desktop and the MCP server read
//! the same frames (see `docs/gui.md`, "Places and modes").
//!
//! - **Places**: the library ([`LIBRARY`]), the darkroom on an image
//!   ([`DARKROOM`], its payload `{"image": id}` or `{"file": path}`),
//!   entered from the library or opened alone, and the workspace's own
//!   network ([`WORKSPACE`], its payload the workspace's key). The root is
//!   the workspace the window shows.
//! - **Modes**: culling (an orientation, set), mask editing (drawing a
//!   path, pushed and never restored) and before-and-after compare
//!   (pushed).
//!
//! Each is mapped to an arrangement, or, for the short pushed modes, to
//! the panels it shows, so toggling them never moves the user's panels.
//! The stack of a workspace is egui-pop's `context` document beside its
//! layout, `<config>/ui/workspaces/<key>/context.json`.

use std::path::{Path, PathBuf};

use egui_pop_model::context::{FrameRegistry, FrameSpec, Verdict};
use egui_pop_model::ids::{FrameId, GroupId, PanelId};

/// The library: the vault's images to browse, rate and cull.
pub const LIBRARY: FrameId = FrameId::from_static("helios.library");
/// The darkroom: one image edited (`{"image": id}` or `{"file": path}`).
pub const DARKROOM: FrameId = FrameId::from_static("helios.darkroom");
/// The workspace's network, edited (its payload: the workspace's key).
pub const WORKSPACE: FrameId = FrameId::from_static("helios.workspace");

/// Culling: flag, rate and label from anywhere, the image first.
pub const CULLING: FrameId = FrameId::from_static("helios.culling");
/// Drawing a mask's path in the image.
pub const MASK_EDITING: FrameId = FrameId::from_static("helios.mask-editing");
/// The image before and after its edit, side by side.
pub const COMPARE: FrameId = FrameId::from_static("helios.compare");

/// How the user works: culling (more orientations to come).
pub const ORIENTATION: GroupId = GroupId::from_static("orientation");
/// A tool of the image (drawing a mask's path).
pub const TOOL: GroupId = GroupId::from_static("tool");
/// Compare.
pub const COMPARING: GroupId = GroupId::from_static("compare");

/// The arrangement of the library (egui-pop's template of that name).
pub const LIBRARY_ARRANGEMENT: &str = "Library";
/// The arrangement of the workspace's network.
pub const NETWORK_ARRANGEMENT: &str = "Network";
/// The image-first arrangement: culling.
pub const IMAGE_ARRANGEMENT: &str = "Image";

/// The Masks panel, shown while a mask is edited.
pub const MASKS_PANEL: &str = "helios.masks";
/// The image, shown while comparing.
pub const IMAGE_PANEL: &str = "helios.viewport";

/// Helios's frames, as declared (the desktop adds its keys).
pub fn frames() -> Vec<FrameSpec> {
    vec![
        FrameSpec::place(LIBRARY, "Library").arrangement(LIBRARY_ARRANGEMENT),
        // The darkroom keeps the layout the user had: it starts from the
        // workspace's own, and remembers what was picked there.
        FrameSpec::place(DARKROOM, "Darkroom"),
        FrameSpec::place(WORKSPACE, "Network").arrangement(NETWORK_ARRANGEMENT),
        FrameSpec::mode(CULLING, ORIENTATION, "Culling").arrangement(IMAGE_ARRANGEMENT),
        FrameSpec::mode(MASK_EDITING, TOOL, "Mask editing")
            .transient()
            .show(PanelId::from_static(MASKS_PANEL)),
        FrameSpec::mode(COMPARE, COMPARING, "Compare").show(PanelId::from_static(IMAGE_PANEL)),
    ]
}

/// The workspace's network guarded: entered only on the workspace
/// `shown` names (the one the window shows, the one an agent works on).
pub fn guard_network(
    spec: FrameSpec,
    shown: impl Fn() -> Option<String> + Send + Sync + 'static,
) -> FrameSpec {
    if spec.id != WORKSPACE {
        return spec;
    }
    spec.can_enter(move |c| match shown() {
        Some(key) if c.payload.as_str() == Some(key.as_str()) => Verdict::Go,
        Some(key) => Verdict::Refuse(format!("the network to edit is this workspace's (`{key}`)")),
        None => Verdict::Refuse("no workspace is open".into()),
    })
}

/// The frames of the workspace `key`: entering its network checks the
/// payload is that workspace.
pub fn registry(key: &str) -> FrameRegistry {
    let mut r = FrameRegistry::new();
    for f in frames() {
        let key = key.to_string();
        r.insert(guard_network(f, move || Some(key.clone())))
            .expect("Helios's frames are declared once");
    }
    r
}

/// The darkroom's payload for an image of the vault.
pub fn image_payload(id: &str) -> serde_json::Value {
    serde_json::json!({ "image": id })
}

/// The darkroom's payload for a file opened on its own.
pub fn file_payload(path: &Path) -> serde_json::Value {
    serde_json::json!({ "file": path })
}

/// What a darkroom payload opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// An image of the vault, by id.
    Image(String),
    /// A file on its own.
    File(PathBuf),
}

impl Opened {
    pub fn of(payload: &serde_json::Value) -> Option<Opened> {
        if let Some(id) = payload["image"].as_str() {
            return Some(Opened::Image(id.to_string()));
        }
        payload["file"]
            .as_str()
            .map(|p| Opened::File(PathBuf::from(p)))
    }

    pub fn payload(&self) -> serde_json::Value {
        match self {
            Opened::Image(id) => image_payload(id),
            Opened::File(p) => file_payload(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_are_declared_once_with_their_groups() {
        let r = registry("k");
        assert_eq!(r.len(), 6);
        let groups = r.groups();
        assert_eq!(groups.len(), 3);
        assert!(r.get(&MASK_EDITING).unwrap().transient);
    }

    #[test]
    fn a_payload_says_what_the_darkroom_opens() {
        let p = image_payload("ab12");
        assert_eq!(Opened::of(&p), Some(Opened::Image("ab12".into())));
        let f = Opened::File("/tmp/x.png".into());
        assert_eq!(Opened::of(&f.payload()), Some(f));
        assert_eq!(Opened::of(&serde_json::Value::Null), None);
    }
}
