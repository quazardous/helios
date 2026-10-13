//! The English catalog in the repository is the one the declarations give
//! today: an id derived from a declaration is never absent in English.

use std::path::Path;

#[test]
fn the_english_catalog_matches_the_declarations() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../i18n/en");
    let files = helios_declarations::collect()
        .english_files()
        .expect("every stable id gives its own message");
    for (name, text) in &files {
        let committed = std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        assert!(
            committed == *text,
            "i18n/en/{name} is out of date with the declarations: run `cargo xtask i18n extract`"
        );
    }
    let extra: Vec<_> = std::fs::read_dir(&dir)
        .expect("i18n/en exists")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| {
            std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e == "ftl")
                && !files.contains_key(n.as_str())
        })
        .collect();
    assert!(
        extra.is_empty(),
        "i18n/en has files no declaration gives: {extra:?}"
    );
}

#[test]
fn the_declarations_cover_the_desktop_and_the_modules() {
    let d = helios_declarations::collect();
    assert!(d.commands.iter().any(|c| c.id == "workbench.palette"));
    assert!(d.commands.iter().any(|c| c.id == "helios.graph.detach"));
    // A panel's focus and toggle commands read texts egui-pop shares.
    assert!(!d.commands.iter().any(|c| c.id == "helios.graph.toggle"));
    assert!(d.commands.iter().any(|c| c.id.starts_with("param.")));
    assert!(d.panels.iter().any(|p| p.id == "helios.graph"));
    assert!(d.modules.iter().any(|m| m.name == "exposure"));
}
