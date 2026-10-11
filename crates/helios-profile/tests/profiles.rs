//! Profiles: export chosen parts to one file, import it back with a diff,
//! never carry secrets, keys portable across systems, variants per system
//! or machine resolved for this one.

use std::collections::BTreeMap;

use helios_profile::*;
use serde_json::json;

fn mine() -> Profile {
    Profile {
        name: "work".into(),
        settings: Some("[interface.graph]\nzoom = \"overview\"\n".into()),
        keymap: Some(vec![Binding {
            keys: portable_keys("ctrl+shift+e", System::Linux),
            command: "export.run".into(),
            when: String::new(),
        }]),
        theme: Some(ThemeChoice {
            name: "helios-light".into(),
            overrides: BTreeMap::from([("accent.base".into(), json!("#3080ff"))]),
        }),
        layout: None,
        plugins: Some(vec![PluginRef {
            id: "io.example.wedding".into(),
            version: "1.2.0".into(),
            registry: "https://plugins.example.org".into(),
            enabled: true,
            settings: BTreeMap::from([
                ("watermark".into(), json!("© studio")),
                ("upload_api_key".into(), json!("abc")),
            ]),
        }]),
        export_targets: None,
        ai: Some(json!({ "brief": "warm, soft skin" })),
        on: BTreeMap::new(),
    }
}

fn no_exclude(_: &str, _: &str) -> bool {
    false
}

#[test]
fn chosen_parts_export_and_import_back() {
    let exclude =
        |plugin: &str, key: &str| plugin == "io.example.wedding" && key == "upload_api_key";
    let file = mine()
        .export(
            &[Part::Settings, Part::Keymap, Part::Theme, Part::Plugins],
            &exclude,
        )
        .unwrap();
    assert!(file.contains("helios_profile = 1"), "{file}");
    assert!(file.contains("Mod+shift+e"), "{file}");
    assert!(
        !file.contains("upload_api_key"),
        "a secret setting stays home: {file}"
    );
    let back = Profile::import(&file).unwrap();
    assert_eq!(back.theme, mine().theme);
    assert_eq!(back.settings, mine().settings);
    assert!(back.ai.is_none(), "a part not chosen is not in the file");
    assert_eq!(back.plugins.unwrap()[0].settings.len(), 1);
}

#[test]
fn an_export_holding_a_secret_is_refused() {
    let mut p = mine();
    p.ai = Some(
        json!({ "endpoint": "https://api.example.com", "note": "sk-proj-AbCdEf0123456789xyz" }),
    );
    let err = p.export(&[Part::Ai], &no_exclude).unwrap_err();
    assert!(
        err.contains("`ai.note` holds what looks like a key"),
        "{err}"
    );
    // Named like a secret, whatever its value.
    let err = mine().export(&[Part::Plugins], &no_exclude).unwrap_err();
    assert!(err.contains("upload_api_key"), "{err}");
    // Ordinary text passes.
    assert!(scan("note = \"warm, soft skin, keep the dress\"\n").is_empty());
}

#[test]
fn an_import_shows_what_changes_part_by_part() {
    let current = mine();
    let mut incoming = mine().only(&[Part::Theme, Part::Plugins, Part::Settings]);
    incoming.theme.as_mut().unwrap().name = "helios-dark".into();
    incoming.settings = Some("[interface.graph]\nzoom = \"detail\"\n".into());
    incoming.plugins.as_mut().unwrap().push(PluginRef {
        id: "io.example.grain".into(),
        version: "0.3.1".into(),
        registry: "https://plugins.example.org".into(),
        enabled: true,
        settings: BTreeMap::new(),
    });
    let d = diff(&current, &incoming);
    let parts: Vec<Part> = d.iter().map(|p| p.part).collect();
    assert_eq!(parts, [Part::Settings, Part::Theme, Part::Plugins]);
    assert!(
        d[0].changes[0].contains("interface.graph.zoom"),
        "{:?}",
        d[0]
    );
    assert!(
        d[1].changes
            .iter()
            .any(|c| c.contains("helios-light → helios-dark")),
        "{:?}",
        d[1]
    );
    assert_eq!(
        d[2].changes,
        ["+ io.example.grain: 0.3.1 from https://plugins.example.org"]
    );
    // Only the new plugin needs installing.
    let installed = current.plugins.clone().unwrap();
    let new: Vec<&str> = to_install(&installed, &incoming)
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(new, ["io.example.grain"]);
}

#[test]
fn a_profile_falls_back_to_the_default_for_what_it_leaves_out() {
    let default = Profile {
        name: "default".into(),
        layout: Some(json!({ "areas": [] })),
        theme: Some(ThemeChoice {
            name: "helios-dark".into(),
            overrides: BTreeMap::new(),
        }),
        ..Profile::default()
    };
    let p = mine().over(&default);
    assert_eq!(p.theme.unwrap().name, "helios-light");
    assert_eq!(p.layout, default.layout);
}

#[test]
fn keys_move_between_systems() {
    let portable = portable_keys("ctrl+k ctrl+s", System::Linux);
    assert_eq!(portable, "Mod+k Mod+s");
    assert_eq!(local_keys(&portable, System::MacOs), "meta+k meta+s");
    assert_eq!(local_keys(&portable, System::Windows), "ctrl+k ctrl+s");
    // Other modifiers stay as they are.
    assert_eq!(portable_keys("alt+shift+f", System::Linux), "alt+shift+f");
}

#[test]
fn a_newer_format_is_refused_by_name() {
    let err = Profile::import("helios_profile = 9\nname = \"x\"\n").unwrap_err();
    assert!(err.contains("newer"), "{err}");
}

/// The plan's example: a bigger font on the laptop, another key on macOS,
/// a darker theme on a machine that is not here, a setting per system.
const VARIED: &str = r##"
helios_profile = 1
name = "work"
settings = """
[interface.graph]
zoom = "overview"
grid = true
"""

[theme]
name = "helios-dark"
overrides = { "accent.base" = "#3080ff" }

[[keymap]]
keys = "Mod+e"
command = "export.run"

[[keymap]]
keys = "Mod+shift+p"
command = "workbench.palette"

[on.macos]
settings = """
[interface.graph]
zoom = "detail"
"""

[[on.macos.keymap]]
keys = "meta+alt+e"
command = "export.run"

[on.machine.laptop.theme]
overrides = { "font.size" = "15px" }

[[on.machine.laptop.keymap]]
keys = ""
command = "workbench.palette"

[on.machine.studio.theme]
name = "helios-high-contrast"

[on.freebsd.theme]
name = "helios-light"
"##;

fn machine(system: System, name: Option<&str>) -> Machine {
    Machine {
        system,
        name: name.map(String::from),
        hidpi: false,
    }
}

fn keys(p: &Profile) -> Vec<(String, String)> {
    p.keymap
        .iter()
        .flatten()
        .map(|b| (b.command.clone(), b.keys.clone()))
        .collect()
}

#[test]
fn a_profile_resolves_for_each_system_and_machine() {
    let p = Profile::import(VARIED).unwrap();
    assert_eq!(p.on.len(), 4, "{:?}", p.on.keys());

    // Linux, unnamed: the common values.
    let linux = p.resolve(&machine(System::Linux, None));
    assert!(linux.profile.on.is_empty());
    assert_eq!(linux.profile.theme, p.theme);
    assert_eq!(
        keys(&linux.profile),
        [
            ("export.run".into(), "Mod+e".into()),
            ("workbench.palette".into(), "Mod+shift+p".into())
        ]
    );
    assert!(linux.from.is_empty(), "{:?}", linux.from);

    // macOS: its key replaces the common one, its setting merges in.
    let mac = p.resolve(&machine(System::MacOs, None));
    assert_eq!(
        keys(&mac.profile),
        [
            ("workbench.palette".into(), "Mod+shift+p".into()),
            ("export.run".into(), "meta+alt+e".into())
        ]
    );
    let settings: toml::Table = mac.profile.settings.as_deref().unwrap().parse().unwrap();
    assert_eq!(
        settings["interface"]["graph"]["zoom"].as_str(),
        Some("detail")
    );
    assert_eq!(settings["interface"]["graph"]["grid"].as_bool(), Some(true));
    let from: Vec<(Part, &str, String)> = mac
        .from
        .iter()
        .map(|f| (f.part, f.key.as_str(), f.selector.to_string()))
        .collect();
    assert_eq!(
        from,
        [
            (Part::Settings, "interface.graph.zoom", "macos".into()),
            (Part::Keymap, "export.run", "macos".into()),
        ]
    );

    // The laptop, on macOS: the system first, then the machine.
    let laptop = p.resolve(&machine(System::MacOs, Some("laptop")));
    let theme = laptop.profile.theme.clone().unwrap();
    assert_eq!(theme.name, "helios-dark");
    assert_eq!(theme.overrides["font.size"], json!("15px"));
    assert_eq!(theme.overrides["accent.base"], json!("#3080ff"));
    assert_eq!(
        keys(&laptop.profile),
        [("export.run".into(), "meta+alt+e".into())]
    );
    let palette = laptop
        .from
        .iter()
        .find(|f| f.key == "workbench.palette")
        .unwrap();
    assert!(palette.removed);
    assert_eq!(palette.selector, Selector::Machine("laptop".into()));
}

#[test]
fn a_variant_for_another_machine_is_kept_but_not_applied() {
    let p = Profile::import(VARIED).unwrap();
    let here = machine(System::Linux, Some("laptop"));
    let r = p.resolve(&here);
    assert_eq!(r.profile.theme.unwrap().name, "helios-dark");
    assert!(p.on.contains_key(&Selector::Machine("studio".into())));
    assert!(p.on.contains_key(&Selector::Other("freebsd".into())));
    assert!(!here.selects(&Selector::Machine("studio".into())));
    // The studio itself applies it.
    let studio = p.resolve(&machine(System::Linux, Some("studio")));
    assert_eq!(studio.profile.theme.unwrap().name, "helios-high-contrast");
}

#[test]
fn variants_round_trip_through_export_and_import() {
    let p = Profile::import(VARIED).unwrap();
    let file = p.export(&Part::ALL, &no_exclude).unwrap();
    assert!(
        file.contains("[on.machine.laptop.theme.overrides]"),
        "{file}"
    );
    assert!(file.contains("[[on.macos.keymap]]"), "{file}");
    assert_eq!(Profile::import(&file).unwrap(), p);

    // Only the chosen parts' variants travel.
    let themes = Profile::import(&p.export(&[Part::Theme], &no_exclude).unwrap()).unwrap();
    assert!(themes.keymap.is_none());
    assert_eq!(
        themes.on.keys().map(|s| s.to_string()).collect::<Vec<_>>(),
        ["machine.laptop", "machine.studio", "freebsd"]
    );

    // This machine's view: the variants flattened.
    let flat = p.resolve(&machine(System::MacOs, Some("laptop"))).profile;
    let file = flat.export(&Part::ALL, &no_exclude).unwrap();
    assert!(!file.contains("[on."), "{file}");
    assert!(file.contains("15px"), "{file}");
}

#[test]
fn an_import_shows_variants_per_selector() {
    let incoming = Profile::import(VARIED).unwrap();
    let current = incoming.resolve(&machine(System::Linux, None)).profile;
    let d = diff(&current, &incoming);
    let keymap = d.iter().find(|d| d.part == Part::Keymap).unwrap();
    assert_eq!(
        keymap.changes,
        [
            "on macos: + export.run: meta+alt+e",
            "on machine.laptop: + workbench.palette: "
        ]
    );
    let theme = d.iter().find(|d| d.part == Part::Theme).unwrap();
    assert!(
        theme
            .changes
            .contains(&"on machine.studio: + theme: helios-high-contrast".to_string()),
        "{:?}",
        theme.changes
    );
}

#[test]
fn a_profile_takes_the_defaults_variants_for_parts_it_leaves_out() {
    let default = Profile::import(VARIED).unwrap();
    let mine = Profile {
        name: "client".into(),
        keymap: Some(Vec::new()),
        ..Profile::default()
    };
    let p = mine.over(&default);
    let laptop = &p.on[&Selector::Machine("laptop".into())];
    assert!(laptop.theme.is_some(), "the default's theme variant");
    assert!(laptop.keymap.is_none(), "the profile has its own keymap");
}

#[test]
fn a_changed_part_is_written_back_over_the_variants() {
    let source = Profile::import(VARIED).unwrap();
    let here = machine(System::MacOs, Some("laptop"));
    let resolved = source.resolve(&here).profile;
    // As a shell holds it: the theme changed, the keys as resolved.
    let mut now = resolved.only(&[Part::Theme, Part::Keymap]);
    now.theme.as_mut().unwrap().name = "helios-light".into();
    let back = now.rebase(&source, &resolved);
    assert_eq!(back.keymap, source.keymap, "unchanged: the source's");
    assert_eq!(back.theme.as_ref().unwrap().name, "helios-light");
    assert!(back.settings.is_none(), "a part not held is left out");
    assert!(back.on[&Selector::System(System::MacOs)].keymap.is_some());
    assert!(back.on[&Selector::Machine("laptop".into())].theme.is_some());
    assert_eq!(
        back.resolve(&machine(System::Linux, Some("studio")))
            .profile
            .theme
            .unwrap()
            .name,
        "helios-high-contrast"
    );
}

#[test]
fn this_machine_is_named_in_a_local_file() {
    let dir = std::env::temp_dir().join(format!("helios-machine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("machine.toml");
    assert_eq!(machine_name(&file), None);
    std::fs::write(&file, "name = \"laptop\"\n").unwrap();
    assert_eq!(machine_name(&file).as_deref(), Some("laptop"));
    let here = Machine::here(&file);
    assert!(here.selects(&Selector::parse("machine.laptop")));
    assert!(here.selects(&Selector::System(System::current())));
    std::fs::remove_dir_all(&dir).unwrap();
}
