//! A plugin's own catalogues.
//!
//! An external plugin translates what its manifest declares (its title
//! and description, its commands' titles) and its own messages in Fluent
//! files beside its manifest, `i18n/<locale>/*.ftl`. They are loaded
//! through egui-pop's prefix-owned catalogues (`I18n::add_owned`): every
//! message must carry the plugin's id (`plugin-<id>.title`,
//! `<id>-anything`), so a plugin never rewrites Helios's text or another
//! plugin's. The one exception is its commands' titles, whose keys are
//! the commands' own (`command-<command id>`): a catalogue may hold the
//! title of a command its manifest declares, and of no other; and its own
//! `plugin-<id>` message, which egui-pop's ownership rule leaves out.
//!
//! ```text
//! plugin-org-example-exif =
//!     .title = Métadonnées ExifTool
//!     .description = Lit les métadonnées des images avec exiftool
//! command-exiftool-version =
//!     .title = Afficher la version d’ExifTool
//! ```

use std::path::Path;

use egui_pop_i18n::I18n;
use egui_pop_i18n::catalog::owned_by;

use crate::ids;

/// One catalogue file of a plugin: its locale, its name and its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCatalogue {
    pub locale: String,
    pub name: String,
    pub source: String,
}

/// The directory of a plugin's catalogues, beside its manifest.
pub const DIR: &str = "i18n";

/// A plugin's title message: `plugin-<id>.title`, as egui-pop derives it.
pub fn title(id: &str) -> ids::MessageId {
    ids::MessageId::attr(format!("plugin-{}", ids::slug(id)), "title")
}

/// A plugin's description message: `plugin-<id>.description`.
pub fn description(id: &str) -> ids::MessageId {
    ids::MessageId::attr(format!("plugin-{}", ids::slug(id)), "description")
}

/// The catalogues under `<plugin dir>/i18n/<locale>/*.ftl`, sorted; none
/// when the plugin ships none.
pub fn read(plugin_dir: &Path) -> Result<Vec<PluginCatalogue>, String> {
    let root = plugin_dir.join(DIR);
    let Ok(locales) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut dirs: Vec<_> = locales.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for dir in dirs.into_iter().filter(|d| d.is_dir()) {
        let locale = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "ftl"))
            .collect();
        files.sort();
        for file in files {
            let source =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            out.push(PluginCatalogue {
                locale: locale.clone(),
                name: file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .into(),
                source,
            });
        }
    }
    Ok(out)
}

/// The entries of a Fluent source, each with its id (`None` for comments
/// and blank lines): an entry starts at a line's first column with a
/// letter (a message) or `-` (a term), and runs until the next one.
fn entries(source: &str) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    for line in source.lines() {
        let starts = line.starts_with(|c: char| c.is_ascii_alphabetic() || c == '-');
        if starts {
            let id = line
                .split('=')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            out.push((Some(id), String::new()));
        } else if line.starts_with('#') || out.is_empty() {
            out.push((None, String::new()));
        }
        let last = out.last_mut().expect("an entry was pushed");
        last.1.push_str(line);
        last.1.push('\n');
    }
    out
}

/// Adds a plugin's catalogues to `i18n`: its own messages through
/// `I18n::add_owned`, the titles of the commands it declares
/// (`commands`, their ids) beside them. Refused, before anything is
/// added, when a message belongs neither to the plugin nor to one of its
/// commands; returns how many messages were added.
pub fn add(
    i18n: &I18n,
    id: &str,
    commands: &[String],
    catalogues: &[PluginCatalogue],
) -> Result<usize, String> {
    // Its own title message (`plugin-<id>`), which egui-pop's ownership
    // rule (`<area>-<owner>-…`) does not cover, and its commands' titles.
    let mut declared: Vec<String> = commands.iter().map(|c| ids::command(c)).collect();
    declared.push(title(id).id);
    let mut parts = Vec::new();
    for c in catalogues {
        let (mut owned, mut titles, mut n) = (String::new(), String::new(), 0);
        for (key, text) in entries(&c.source) {
            match key {
                // Terms are the plugin's own: they only serve its messages.
                Some(k) if k.starts_with('-') || owned_by(id, &k) => {
                    owned.push_str(&text);
                    n += 1;
                }
                Some(k) if declared.contains(&k) => {
                    titles.push_str(&text);
                    n += 1;
                }
                Some(k) => {
                    return Err(format!(
                        "{}/{}: `{k}` belongs neither to `{id}` nor to one of its commands",
                        c.locale, c.name
                    ));
                }
                None => owned.push_str(&text),
            }
        }
        parts.push((c, owned, titles, n));
    }
    let mut added = 0;
    for (c, owned, titles, n) in parts {
        let at = |e: String| format!("{}/{}: {e}", c.locale, c.name);
        i18n.add_owned(id, &c.locale, &owned).map_err(at)?;
        if !titles.is_empty() {
            i18n.add(&c.locale, &titles).map_err(at)?;
        }
        added += n;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "io.github.quazardous.helios.exif-exiftool";

    fn example() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/exif-exiftool")
    }

    #[test]
    fn the_example_plugin_is_translated_by_its_own_catalogues() {
        let i = I18n::with_egui_pop("fr");
        i.add_catalogues(crate::CATALOGUES).unwrap();
        let catalogues = read(&example()).unwrap();
        assert!(catalogues.iter().any(|c| c.locale == "fr"));
        let n = add(&i, ID, &["exiftool.version".into()], &catalogues).unwrap();
        assert!(n >= 3, "{n}");
        assert_eq!(i.t(&title(ID).key(), &[]), "Métadonnées ExifTool");
        assert!(
            i.t(&description(ID).key(), &[])
                .starts_with("Lit les métadonnées")
        );
        assert_eq!(
            i.t(&ids::command_title("exiftool.version").key(), &[]),
            "Afficher la version d’ExifTool"
        );
        // English, from the same plugin.
        i.set_locale("en");
        assert_eq!(i.t(&title(ID).key(), &[]), "ExifTool metadata");
    }

    #[test]
    fn a_plugin_cannot_write_another_s_text() {
        let i = I18n::with_egui_pop("fr");
        i.add_catalogues(crate::CATALOGUES).unwrap();
        let pirate = |source: &str| {
            vec![PluginCatalogue {
                locale: "fr".into(),
                name: "plugin.ftl".into(),
                source: source.into(),
            }]
        };
        for source in [
            "panel-helios-graph =\n    .title = Pirate\n",
            "command-view-palette =\n    .title = Pirate\n",
            "other-plugin-hello = Pirate\n",
        ] {
            let e = add(&i, ID, &["exiftool.version".into()], &pirate(source)).unwrap_err();
            assert!(e.contains("belongs neither"), "{e}");
        }
        assert_eq!(i.t(&ids::panel_title("helios.graph").key(), &[]), "Graphe");
        // Its own free messages and terms are fine.
        let own = "-tool = ExifTool\nio-github-quazardous-helios-exif-exiftool-hello = Bonjour de { -tool }\n";
        assert_eq!(add(&i, ID, &[], &pirate(own)).unwrap(), 2);
        assert_eq!(
            i.t("io-github-quazardous-helios-exif-exiftool-hello", &[]),
            "Bonjour de ExifTool"
        );
    }
}
