//! The built-in templates (`templates/*.helios-template`): valid against
//! the modules and composites Helios ships, built headless, their text in
//! English and French.

use std::path::{Path, PathBuf};

use egui_pop_model::ids::PanelId;
use egui_pop_model::panels::Area;
use helios_core::ModuleRef;
use helios_template::{Bundle, Catalogue, parse_composite, schema};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The modules and composites the built-in templates use.
fn catalogue() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(m::MaskCombine))
        .module(ModuleRef::new(m::MaskFeather))
        .module(ModuleRef::new(m::MaskBlend));
    for e in std::fs::read_dir(root().join("composites")).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        c.add_composite(parse_composite(&text).unwrap());
    }
    c
}

fn portrait() -> Bundle {
    Bundle::load(&root().join("portrait-retouch.helios-template")).unwrap()
}

#[test]
fn portrait_retouch_is_valid_and_builds_with_masks_docked() {
    let bundle = portrait();
    assert_eq!(bundle.template.id, "helios.portrait-retouch");
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    assert!(report.updates.is_empty());
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    let built = bundle.build(&catalogue()).unwrap();
    let area = |p: &str| built.layout.area_of(&PanelId::new(p).unwrap());
    assert_eq!(area("helios.viewport"), Some(Area::Center));
    assert_eq!(area("helios.masks"), Some(Area::StartSide));
    assert_eq!(area("helios.params"), Some(Area::EndSide));
    // The masks opened, so the Masks panel and the viewport edit them.
    for inner in ["retouch.mask", "retouch.feather", "retouch.blend"] {
        assert!(built.nodes.contains_key(inner), "{inner}");
    }
    assert_eq!(built.view.0, built.nodes["retouch.blend"]);
    // The mask tools in the toolbox.
    let tools = &bundle.template.tools;
    assert!(tools.toolbox.iter().all(|c| c.starts_with("mask.add.")));
    assert_eq!(tools.default.as_deref(), Some("mask.add.ellipse"));
    // Each preset builds.
    for p in &bundle.template.network.presets {
        let built = bundle.build_with(
            &catalogue(),
            &helios_template::placeholder(),
            Some(p.id.as_str()),
        );
        assert!(built.is_ok(), "{}", p.id);
    }
}

#[test]
fn portrait_retouch_speaks_english_and_french() {
    let bundle = portrait();
    for locale in ["en", "fr"] {
        let text = bundle
            .texts
            .iter()
            .find(|t| t.locale == locale)
            .unwrap_or_else(|| panic!("no {locale} text"));
        for key in [bundle.name_key(), bundle.scenario_key()] {
            assert!(
                text.source.contains(&format!("{key} = ")),
                "{locale}: {key}"
            );
        }
    }
    // English as meta says it.
    let en = &bundle
        .texts
        .iter()
        .find(|t| t.locale == "en")
        .unwrap()
        .source;
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.name)));
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.scenario)));
}
