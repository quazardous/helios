//! Helios's module catalogue: every processing module, listed once.
//!
//! Each module keeps its own code in its crate under `modules/`; this crate
//! only gathers what each one provides, so that the node, the desktop, the
//! templates and calibration all read the same list instead of naming the
//! modules themselves:
//!
//! - its **id** and its **node entry** (the [`ModuleRef`] the node
//!   instantiates under that id): [`registry`], [`module`];
//! - whether the desktop's **templates** may name it: [`desktop`];
//! - its **control patches** (the controls its parameters are drawn with):
//!   [`patches`].
//!
//! A consumer that needs only some modules (calibration's look chain, for
//! one) filters this list by id and says why; it does not list modules of
//! its own. The module crates are re-exported under their short names
//! ([`exposure`], [`masks`]…) for code that uses a module's own API.
//!
//! A test checks that every crate under `modules/` has its entries here.

use std::collections::BTreeMap;

use helios_controls::Patch;
use helios_core::ModuleRef;

pub use helios_module_blur as blur;
pub use helios_module_cameracolour as cameracolour;
pub use helios_module_cameratone as cameratone;
pub use helios_module_channelmixerrgb as channelmixerrgb;
pub use helios_module_chartprofile as chartprofile;
pub use helios_module_colorin as colorin;
pub use helios_module_demosaic as demosaic;
pub use helios_module_exposure as exposure;
pub use helios_module_flip as flip;
pub use helios_module_highlights as highlights;
pub use helios_module_masks as masks;
pub use helios_module_rawprepare as rawprepare;
pub use helios_module_sigmoid as sigmoid;
pub use helios_module_vkdt as vkdt;
pub use helios_module_whitebalance as whitebalance;

/// One module as the catalogue lists it.
#[derive(Clone, Copy)]
pub struct Entry {
    /// The module's id: its node name, and its manifest's name.
    pub id: &'static str,
    /// The crate (under `modules/`) the module comes from.
    pub krate: &'static str,
    /// The module, ready to instantiate.
    pub module: fn() -> ModuleRef,
    /// Whether the desktop's templates may name it (see [`desktop`]).
    pub desktop: bool,
    /// The control patches it declares, if any.
    pub patches: Option<fn() -> Vec<Patch>>,
}

impl Entry {
    const fn new(id: &'static str, krate: &'static str, module: fn() -> ModuleRef) -> Self {
        Self {
            id,
            krate,
            module,
            desktop: true,
            patches: None,
        }
    }

    /// Not named by the desktop's templates.
    const fn node_only(mut self) -> Self {
        self.desktop = false;
        self
    }

    const fn patches(mut self, patches: fn() -> Vec<Patch>) -> Self {
        self.patches = Some(patches);
        self
    }
}

const RAWPREPARE: &str = "helios-module-rawprepare";
const WHITEBALANCE: &str = "helios-module-whitebalance";
const HIGHLIGHTS: &str = "helios-module-highlights";
const DEMOSAIC: &str = "helios-module-demosaic";
const COLORIN: &str = "helios-module-colorin";
const CHARTPROFILE: &str = "helios-module-chartprofile";
const CAMERACOLOUR: &str = "helios-module-cameracolour";
const CAMERATONE: &str = "helios-module-cameratone";
const FLIP: &str = "helios-module-flip";
const CHANNELMIXERRGB: &str = "helios-module-channelmixerrgb";
const EXPOSURE: &str = "helios-module-exposure";
const SIGMOID: &str = "helios-module-sigmoid";
const BLUR: &str = "helios-module-blur";
const VKDT: &str = "helios-module-vkdt";
const MASKS: &str = "helios-module-masks";

/// Every module Helios links, once.
///
/// The ones marked `node_only` are left out of the desktop's templates
/// (as before this catalogue existed): `decide_exposure` is a decision
/// node a model or a human answers through the node; `blur` is the
/// framework's test case for neighbourhood reads; the `vkdt_*` ports are
/// checked against vkdt by `helios-parity` and not offered yet; and
/// `mask_object` is answered by the node's resolver.
static ENTRIES: &[Entry] = &[
    Entry::new("rawprepare", RAWPREPARE, || {
        ModuleRef::new(rawprepare::RawPrepare)
    }),
    Entry::new("whitebalance", WHITEBALANCE, || {
        ModuleRef::new(whitebalance::WhiteBalance)
    }),
    Entry::new("highlights", HIGHLIGHTS, || {
        ModuleRef::new(highlights::Highlights)
    }),
    Entry::new("demosaic", DEMOSAIC, || ModuleRef::new(demosaic::Demosaic)),
    Entry::new("colorin", COLORIN, || ModuleRef::new(colorin::ColorIn)),
    Entry::new("chartprofile", CHARTPROFILE, || {
        ModuleRef::new(chartprofile::ChartProfile)
    }),
    Entry::new("cameracolour", CAMERACOLOUR, || {
        ModuleRef::new(cameracolour::CameraColour)
    }),
    Entry::new("cameratone", CAMERATONE, || {
        ModuleRef::new(cameratone::CameraTone)
    }),
    Entry::new("flip", FLIP, || ModuleRef::new(flip::Flip)),
    Entry::new("channelmixerrgb", CHANNELMIXERRGB, || {
        ModuleRef::new(channelmixerrgb::ChannelMixerRgb)
    }),
    Entry::new("exposure", EXPOSURE, || ModuleRef::new(exposure::Exposure))
        .patches(exposure::patches),
    Entry::new("decide_exposure", EXPOSURE, || {
        ModuleRef::new(exposure::DecideExposure)
    })
    .node_only(),
    Entry::new("sigmoid", SIGMOID, || ModuleRef::new(sigmoid::Sigmoid)),
    Entry::new("blur", BLUR, || ModuleRef::new(blur::Blur)).node_only(),
    Entry::new("vkdt_exposure", VKDT, || ModuleRef::new(vkdt::VkdtExposure)).node_only(),
    Entry::new("vkdt_usm", VKDT, || ModuleRef::new(vkdt::VkdtUsm)).node_only(),
    Entry::new("vkdt_contrast", VKDT, || {
        ModuleRef::composite(vkdt::VkdtContrast)
    })
    .node_only(),
    Entry::new("vkdt_grade", VKDT, || ModuleRef::new(vkdt::VkdtGrade)).node_only(),
    Entry::new("vkdt_llap", VKDT, || ModuleRef::composite(vkdt::VkdtLlap)).node_only(),
    Entry::new("vkdt_filmcurv", VKDT, || ModuleRef::new(vkdt::VkdtFilmcurv)).node_only(),
    Entry::new("vkdt_colour", VKDT, || ModuleRef::new(vkdt::VkdtColour)).node_only(),
    Entry::new("mask_combine", MASKS, || ModuleRef::new(masks::MaskCombine)),
    Entry::new("mask_blend", MASKS, || ModuleRef::new(masks::MaskBlend)),
    Entry::new("mask_feather", MASKS, || ModuleRef::new(masks::MaskFeather)),
    Entry::new("mask_blur", MASKS, || ModuleRef::new(masks::MaskBlur)),
    Entry::new("mask_levels", MASKS, || ModuleRef::new(masks::MaskLevels)),
    Entry::new("mask_from_luma", MASKS, || {
        ModuleRef::new(masks::MaskFromLuma)
    }),
    Entry::new("mask_parametric", MASKS, || {
        ModuleRef::new(masks::MaskParametric)
    }),
    Entry::new("mask_raster", MASKS, || ModuleRef::new(masks::MaskRaster)),
    Entry::new("mask_circle", MASKS, || ModuleRef::new(masks::MaskCircle)),
    Entry::new("mask_ellipse", MASKS, || ModuleRef::new(masks::MaskEllipse)),
    Entry::new("mask_gradient", MASKS, || {
        ModuleRef::new(masks::MaskGradient)
    }),
    Entry::new("mask_path", MASKS, || ModuleRef::new(masks::MaskPath)),
    Entry::new("mask_brush", MASKS, || ModuleRef::new(masks::MaskBrush)),
    Entry::new("mask_object", MASKS, || ModuleRef::new(masks::MaskObject)).node_only(),
];

/// Every entry, in the catalogue's order.
pub fn entries() -> &'static [Entry] {
    ENTRIES
}

/// Every module id.
pub fn ids() -> impl Iterator<Item = &'static str> {
    ENTRIES.iter().map(|e| e.id)
}

/// The module of this id.
pub fn module(id: &str) -> Option<ModuleRef> {
    ENTRIES.iter().find(|e| e.id == id).map(|e| (e.module)())
}

/// Every module the node can instantiate, by id.
pub fn registry() -> BTreeMap<&'static str, ModuleRef> {
    ENTRIES.iter().map(|e| (e.id, (e.module)())).collect()
}

/// The modules the desktop's templates may name: the raw chain (levels,
/// white balance, highlights, demosaicing, camera colours), the tools on a
/// picture and the drawn masks.
pub fn desktop() -> impl Iterator<Item = ModuleRef> {
    ENTRIES.iter().filter(|e| e.desktop).map(|e| (e.module)())
}

/// Every module's control patches.
pub fn patches() -> Vec<Patch> {
    ENTRIES
        .iter()
        .filter_map(|e| e.patches)
        .flat_map(|p| p())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    /// The crate names under `modules/`.
    fn module_crates() -> BTreeSet<String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
        let mut names = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let manifest = entry.unwrap().path().join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let name = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("name = "))
                .unwrap_or_else(|| panic!("no name in {}", manifest.display()));
            names.insert(name.trim_matches('"').to_string());
        }
        names
    }

    #[test]
    fn every_module_crate_is_in_the_catalogue() {
        let listed: BTreeSet<String> = ENTRIES.iter().map(|e| e.krate.to_string()).collect();
        let missing: Vec<_> = module_crates().difference(&listed).cloned().collect();
        assert!(
            missing.is_empty(),
            "module crates with no entry in helios-modules: {missing:?}"
        );
        let unknown: Vec<_> = listed.difference(&module_crates()).cloned().collect();
        assert!(
            unknown.is_empty(),
            "entries naming no crate under modules/: {unknown:?}"
        );
    }

    #[test]
    fn ids_are_unique_and_match_the_manifests() {
        let mut seen = BTreeSet::new();
        for e in ENTRIES {
            assert!(seen.insert(e.id), "{} listed twice", e.id);
            assert_eq!(
                (e.module)().manifest().name,
                e.id,
                "{}'s manifest name",
                e.id
            );
        }
    }

    #[test]
    fn the_views_agree_with_the_entries() {
        let all: Vec<_> = ids().collect();
        assert_eq!(
            registry().keys().copied().collect::<BTreeSet<_>>(),
            all.iter().copied().collect()
        );
        for id in &all {
            assert!(module(id).is_some(), "{id}");
        }
        for m in desktop() {
            assert!(all.contains(&m.manifest().name.as_str()));
        }
        assert!(!patches().is_empty(), "exposure declares its control");
    }
}
