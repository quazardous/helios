//! Profiles: export chosen parts to one file, import it back with a diff,
//! never carry secrets, keys portable across systems.

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
