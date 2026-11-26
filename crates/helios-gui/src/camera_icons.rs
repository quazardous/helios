//! The camera drawings: one generic body shape per kind of camera
//! (`helios_connect::camera::CameraKind`), drawn for Helios (no
//! product's picture), registered with egui-pop's icon set so the import
//! window shows a source's camera through the same icon path as any
//! other icon.

use egui_pop::core::icon_set::{self, Paint};
use helios_connect::camera::CameraKind;

/// Each kind's drawing, single-colour.
const DRAWINGS: &[(CameraKind, &[u8])] = &[
    (
        CameraKind::Mirrorless,
        include_bytes!("../icons/cameras/mirrorless.svg"),
    ),
    (
        CameraKind::Dslr,
        include_bytes!("../icons/cameras/dslr.svg"),
    ),
    (
        CameraKind::Compact,
        include_bytes!("../icons/cameras/compact.svg"),
    ),
    (
        CameraKind::Phone,
        include_bytes!("../icons/cameras/phone.svg"),
    ),
];

/// The icon name of a kind's drawing (`helios-camera-mirrorless`).
pub fn icon(kind: CameraKind) -> String {
    format!("helios-camera-{}", kind.id())
}

/// Registers the drawings, once per process; before the theme's install,
/// which rasterises them.
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (kind, svg) in DRAWINGS {
            if let Err(e) = icon_set::register(&icon(*kind), *svg, Paint::Mono) {
                log::warn!("the {} camera drawing: {e}", kind.id());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_camera_drawing_registers_and_resolves_by_its_name() {
        register();
        for (kind, _) in DRAWINGS {
            assert!(
                egui_pop::core::icon::registry::is_known(&icon(*kind)),
                "{kind:?}"
            );
        }
    }
}
