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
        .module(ModuleRef::new(helios_module_rawprepare::RawPrepare))
        .module(ModuleRef::new(helios_module_whitebalance::WhiteBalance))
        .module(ModuleRef::new(helios_module_highlights::Highlights))
        .module(ModuleRef::new(helios_module_demosaic::Demosaic))
        .module(ModuleRef::new(helios_module_colorin::ColorIn))
        .module(ModuleRef::new(
            helios_module_channelmixerrgb::ChannelMixerRgb,
        ))
        .module(ModuleRef::new(helios_module_flip::Flip))
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
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
fn portrait_retouch_is_valid_and_builds_with_masks_docked_and_the_scopes_floating() {
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
    // The scopes float over the image, as a palette.
    let scopes = PanelId::new("helios.scopes").unwrap();
    assert!(built.layout.floating_of(&scopes).is_some());
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
            helios_template::Source::picture(&helios_template::placeholder()),
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

fn intake() -> Bundle {
    Bundle::load(&root().join("import-cull-dedupe.helios-template")).unwrap()
}

#[test]
fn import_cull_dedupe_is_valid_and_builds_a_culling_layout() {
    let bundle = intake();
    assert_eq!(bundle.template.id, "helios.import-cull-dedupe");
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    let built = bundle.build(&catalogue()).unwrap();
    let area = |p: &str| built.layout.area_of(&PanelId::new(p).unwrap());
    // The grid central, with the comparison and the loupe as its tabs;
    // the stacks to review and the duplicates beside; the filmstrip under.
    for p in ["helios.vault", "helios.compare", "helios.viewport"] {
        assert_eq!(area(p), Some(Area::Center), "{p}");
    }
    assert_eq!(area("helios.stacks"), Some(Area::EndSide));
    assert_eq!(area("helios.duplicates"), Some(Area::EndSide));
    assert_eq!(area("helios.filmstrip"), Some(Area::Bottom));
    // A light preview pipe for the loupe.
    assert_eq!(built.view.0, built.nodes["exposure"]);
    // A card copied, renamed, its duplicates asked about, stacks proposed
    // after each import; culling moves on after each mark.
    let library = bundle.template.library.as_ref().unwrap();
    assert!(library.auto_advance);
    let import = library.import.as_ref().unwrap();
    assert_eq!(
        (
            import.source.as_str(),
            import.placement.as_str(),
            import.duplicates.as_str()
        ),
        ("card", "copy", "ask")
    );
    assert_eq!(import.rename.as_deref(), Some("{date}-{seq}"));
    assert!(import.suggest_stacks);
    // The culling tools; the loupe takes the culling keys too.
    let tools = &bundle.template.tools;
    assert_eq!(tools.default.as_deref(), Some("vault.flag.pick"));
    for (keys, command) in [
        ("P", "vault.flag.pick"),
        ("X", "vault.flag.reject"),
        ("3", "vault.rate.3"),
    ] {
        assert!(
            tools
                .keymap
                .iter()
                .any(|b| b.keys == keys && b.command == command),
            "{keys}"
        );
    }
}

#[test]
fn import_cull_dedupe_speaks_english_and_french() {
    let bundle = intake();
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
    let en = &bundle
        .texts
        .iter()
        .find(|t| t.locale == "en")
        .unwrap()
        .source;
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.name)));
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.scenario)));
}

#[test]
fn an_import_source_names_known_words() {
    let mut bundle = intake();
    {
        let import = bundle
            .template
            .library
            .as_mut()
            .unwrap()
            .import
            .as_mut()
            .unwrap();
        import.source = "phone".into();
        import.placement = "reference".into();
        import.duplicates = "maybe".into();
    }
    let problems = bundle.validate(&catalogue()).problems;
    for expected in [
        "library.import.source: `phone`",
        "library.import.duplicates: `maybe`",
        "library.import.rename: only a copy is renamed",
    ] {
        assert!(
            problems.iter().any(|p| p.starts_with(expected)),
            "{expected}: {problems:?}"
        );
    }
}

#[test]
fn a_library_query_names_known_criteria_and_fits_the_schema() {
    let mut bundle = intake();
    let query = helios_template::QueryDesc {
        min_rating: Some(2),
        labels: vec!["red".into()],
        flags: vec!["pick".into()],
        tags: vec!["sea".into()],
    };
    assert_eq!(query.search_text(), "rating:2 label:red flag:pick tag:sea");
    bundle.template.library.as_mut().unwrap().query = Some(query);
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    let written = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(written["library"]["query"]["flags"][0], "pick");
    assert_eq!(
        schema::check(&schema::template(), &written),
        Vec::<String>::new()
    );
    // Words it does not know, too many stars, a tag the search field
    // cannot show, and a query asking nothing are refused.
    bundle.template.library.as_mut().unwrap().query = Some(helios_template::QueryDesc {
        min_rating: Some(6),
        labels: vec!["pink".into()],
        flags: vec!["picked".into()],
        tags: vec!["two words".into()],
    });
    let problems = bundle.validate(&catalogue()).problems;
    for expected in [
        "library.query.min_rating: 6",
        "library.query.labels: `pink`",
        "library.query.flags: `picked`",
        "library.query.tags: `two words`",
    ] {
        assert!(
            problems.iter().any(|p| p.starts_with(expected)),
            "{expected}: {problems:?}"
        );
    }
    let written = serde_json::to_value(&bundle.template).unwrap();
    assert!(!schema::check(&schema::template(), &written).is_empty());
    bundle.template.library.as_mut().unwrap().query = Some(helios_template::QueryDesc::default());
    let problems = bundle.validate(&catalogue()).problems;
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("library.query: no criterion")),
        "{problems:?}"
    );
}

fn develop() -> Bundle {
    Bundle::load(&root().join("develop-preview.helios-template")).unwrap()
}

#[test]
fn develop_preview_is_valid_and_builds_exposure_into_sigmoid_beside_the_image() {
    let bundle = develop();
    assert_eq!(bundle.template.id, "helios.develop-preview");
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    assert!(report.updates.is_empty());
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    let built = bundle.build(&catalogue()).unwrap();
    // The source through exposure into sigmoid, the sigmoid shown.
    let names: Vec<&str> = built.nodes.keys().map(String::as_str).collect();
    assert_eq!(names, ["exposure", "sigmoid", "source"]);
    assert_eq!(built.view.0, built.nodes["sigmoid"]);
    // The image large, the parameters above the scopes beside it, the
    // filmstrip under to pick the picture; the panels of other tasks away.
    let area = |p: &str| built.layout.area_of(&PanelId::new(p).unwrap());
    assert_eq!(area("helios.viewport"), Some(Area::Center));
    assert_eq!(area("helios.params"), Some(Area::EndSide));
    assert_eq!(area("helios.scopes"), Some(Area::EndSide));
    assert_eq!(area("helios.filmstrip"), Some(Area::Bottom));
    for p in ["helios.graph", "helios.masks"] {
        assert!(
            built.layout.hidden.contains(&PanelId::new(p).unwrap()),
            "{p}"
        );
    }
    // It says what it develops: the picture, or a raw's sensor data.
    assert!(bundle.template.meta.name.contains("preview"));
    assert!(bundle.template.meta.scenario.contains("sensor data"));
}

#[test]
fn develop_preview_speaks_english_and_french() {
    let bundle = develop();
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
    let en = &bundle
        .texts
        .iter()
        .find(|t| t.locale == "en")
        .unwrap()
        .source;
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.name)));
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.scenario)));
}

fn develop_raw() -> Bundle {
    Bundle::load(&root().join("develop.helios-template")).unwrap()
}

#[test]
fn develop_one_image_reads_a_raw_through_the_raw_chain_beside_the_image() {
    let bundle = develop_raw();
    assert_eq!(bundle.template.id, "helios.develop");
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    // Its image is a raw's sensor data, through the node's raw path,
    // then exposure into sigmoid, the sigmoid shown.
    let net = &bundle.template.network;
    assert_eq!(net.raw_sources, ["raw"]);
    assert!(net.reads_raw());
    let chain: Vec<&str> = net.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(
        chain,
        [
            "rawprepare",
            "whitebalance",
            "highlights",
            "demosaic",
            "colorin",
            "channelmixerrgb",
            "flip",
            "exposure",
            "sigmoid"
        ]
    );
    let mut from = "raw";
    for to in &chain {
        assert!(
            net.connections
                .iter()
                .any(|c| c.from == from && c.to == *to),
            "{from} -> {to}"
        );
        from = to;
    }
    let built = bundle.build(&catalogue()).unwrap();
    assert_eq!(built.view.0, built.nodes["sigmoid"]);
    // Laid out as the preview template: the image large, the parameters
    // and the scopes beside, the filmstrip under.
    let area = |p: &str| built.layout.area_of(&PanelId::new(p).unwrap());
    assert_eq!(area("helios.viewport"), Some(Area::Center));
    assert_eq!(area("helios.params"), Some(Area::EndSide));
    assert_eq!(area("helios.scopes"), Some(Area::EndSide));
    assert_eq!(area("helios.filmstrip"), Some(Area::Bottom));
    assert!(bundle.template.meta.scenario.contains("sensor data"));
    // Its text in English and French.
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
    let en = &bundle
        .texts
        .iter()
        .find(|t| t.locale == "en")
        .unwrap()
        .source;
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.name)));
    assert!(en.contains(&format!("= {}\n", bundle.template.meta.scenario)));
}

fn batch_export() -> Bundle {
    Bundle::load(&root().join("batch-export.helios-template")).unwrap()
}

#[test]
fn batch_export_opens_on_the_picks_with_the_export_ready() {
    let bundle = batch_export();
    assert_eq!(bundle.template.id, "helios.batch-export");
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    let built = bundle.build(&catalogue()).unwrap();
    let area = |p: &str| built.layout.area_of(&PanelId::new(p).unwrap());
    // The library central; the queue of the node exported and the log
    // beside; the jobs under.
    assert_eq!(area("helios.vault"), Some(Area::Center));
    assert_eq!(area("helios.queue"), Some(Area::EndSide));
    assert_eq!(area("helios.log"), Some(Area::EndSide));
    assert_eq!(area("egui_pop.jobs"), Some(Area::Bottom));
    assert_eq!(built.view.0, built.nodes["sigmoid"]);
    let library = bundle.template.library.as_ref().unwrap();
    assert_eq!(library.query.as_ref().unwrap().flags, ["pick"]);
    let export = library.export.as_ref().unwrap();
    assert_eq!(
        (
            export.format.as_str(),
            export.space.as_str(),
            export.pattern.as_str()
        ),
        ("jpeg", "srgb", "{stem}")
    );
    assert_eq!(export.folder.as_deref(), Some("~/Pictures/Helios export"));
    // Its text in both languages.
    for locale in ["en", "fr"] {
        let text = bundle.texts.iter().find(|t| t.locale == locale).unwrap();
        assert!(text.source.contains(&format!("{} = ", bundle.name_key())));
    }
}

fn compare() -> Bundle {
    Bundle::load(&root().join("compare.helios-template")).unwrap()
}

#[test]
fn compare_opens_in_compare_with_the_photos_central_and_the_filmstrip_under() {
    let bundle = compare();
    assert_eq!(bundle.template.id, "helios.compare");
    assert_eq!(bundle.template.meta.name, "Compare");
    let report = bundle.validate(&catalogue());
    assert!(report.is_ok(), "{:?}", report.problems);
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    let built = bundle.build(&catalogue()).unwrap();
    let area = |p: &str| built.layout.area_of(&PanelId::new(p).unwrap());
    assert_eq!(area("helios.compare"), Some(Area::Center));
    assert_eq!(area("helios.filmstrip"), Some(Area::Bottom));
    // Each photo drawn from its edit: the workspace's network develops it.
    assert_eq!(built.view.0, built.nodes["sigmoid"]);
    assert!(bundle.template.library.as_ref().unwrap().compare);
    for locale in ["en", "fr"] {
        let text = bundle.texts.iter().find(|t| t.locale == locale).unwrap();
        assert!(text.source.contains(&format!("{} = ", bundle.name_key())));
        assert!(
            text.source
                .contains(&format!("{} = ", bundle.scenario_key()))
        );
    }
}

#[test]
fn an_export_s_settings_are_checked() {
    let mut bundle = batch_export();
    let export = bundle
        .template
        .library
        .as_mut()
        .unwrap()
        .export
        .as_mut()
        .unwrap();
    export.format = "webp".into();
    export.bits = Some(16);
    export.folder = Some("relative/out".into());
    export.pattern = "a/{stem}".into();
    export.long_edge = Some(8);
    let problems = bundle.validate(&catalogue()).problems;
    for field in ["format", "bits", "folder", "pattern", "long_edge"] {
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with(&format!("library.export.{field}"))),
            "{field}: {problems:?}"
        );
    }
}

#[test]
fn a_library_s_sort_is_checked_and_follows_the_schema() {
    let mut bundle = batch_export();
    let library = bundle.template.library.as_mut().unwrap();
    library.sort = Some(helios_template::SortDesc {
        by: "rating".into(),
        descending: true,
    });
    assert!(bundle.validate(&catalogue()).is_ok());
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        json["library"]["sort"],
        serde_json::json!({ "by": "rating", "descending": true })
    );
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    bundle.template.library.as_mut().unwrap().sort = Some(helios_template::SortDesc {
        by: "colour".into(),
        descending: false,
    });
    let problems = bundle.validate(&catalogue()).problems;
    assert!(
        problems.iter().any(|p| p.starts_with("library.sort.by")),
        "{problems:?}"
    );
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert!(!schema::check(&schema::template(), &json).is_empty());
}

#[test]
fn a_library_s_folder_is_absolute_and_follows_the_schema() {
    let mut bundle = batch_export();
    let folder = if cfg!(windows) {
        "C:\\photos\\2024"
    } else {
        "/photos/2024"
    };
    bundle.template.library.as_mut().unwrap().folder = Some(folder.into());
    assert!(bundle.validate(&catalogue()).is_ok());
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(json["library"]["folder"], serde_json::json!(folder));
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
    bundle.template.library.as_mut().unwrap().folder = Some("photos/2024".into());
    let problems = bundle.validate(&catalogue()).problems;
    assert!(
        problems.iter().any(|p| p.starts_with("library.folder")),
        "{problems:?}"
    );
}

#[test]
fn a_library_s_collection_follows_the_schema() {
    let mut bundle = batch_export();
    bundle.template.library.as_mut().unwrap().collection = Some("trip-2024".into());
    assert!(bundle.validate(&catalogue()).is_ok());
    let json = serde_json::to_value(&bundle.template).unwrap();
    assert_eq!(
        json["library"]["collection"],
        serde_json::json!("trip-2024")
    );
    assert_eq!(
        schema::check(&schema::template(), &json),
        Vec::<String>::new()
    );
}

#[test]
fn the_picture_templates_and_the_blank_workspace_take_a_picture_or_a_raw() {
    // Every built-in template but "Develop one image" (a raw only), and
    // the blank workspace: a raw goes through the raw front.
    for b in helios_template::shipped::templates() {
        let net = &b.template.network;
        if b.template.id == "helios.develop" {
            assert!(net.reads_raw());
            continue;
        }
        assert_eq!(net.image_sources, net.sources, "{}", b.template.id);
        assert!(net.takes_raw() && !net.reads_raw(), "{}", b.template.id);
    }
    let blank = helios_template::workspace::blank_network();
    assert_eq!(blank.image_sources, blank.sources);
}

#[test]
fn a_raw_is_exposed_as_darktable_exposes_it_by_default() {
    use helios_core::Value;
    // darktable's scene-referred default for a raw: +0.7 EV, black
    // -0.000244, the camera's exposure bias taken back.
    let templates = helios_template::shipped::templates();
    let develop = templates
        .iter()
        .find(|b| b.template.id == "helios.develop")
        .unwrap();
    let exposure = |b: &Bundle| {
        b.template
            .network
            .nodes
            .iter()
            .find(|n| n.name == "exposure")
            .cloned()
            .unwrap()
    };
    let raw = exposure(develop).params;
    assert_eq!(raw.get("exposure"), Some(&Value::Float(0.7)));
    assert_eq!(raw.get("black"), Some(&Value::Float(-0.000244140625)));
    assert_eq!(raw.get("compensate_bias"), Some(&Value::Bool(true)));
    // The templates whose exposure is the image's own give a raw the
    // same; a picture keeps exposure's identity.
    for id in [
        "helios.develop-preview",
        "helios.batch-export",
        "helios.import-cull-dedupe",
    ] {
        let b = templates.iter().find(|b| b.template.id == id).unwrap();
        let n = exposure(b);
        assert_eq!(n.raw_params, raw, "{id}");
        assert!(n.params.iter().next().is_none(), "{id}");
    }
}
