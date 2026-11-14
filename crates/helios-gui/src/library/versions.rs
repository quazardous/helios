//! A photo's versions in the Library (L10, L-C5 B): the versions strip
//! under the cursor's marks, as darktable's duplicate manager and
//! Lightroom's virtual copies. Each head of the photo is a button with
//! its number and name, the current one (the head an edit opens)
//! highlighted; a click makes it current. "Remove Version…" removes a
//! copy after a question. Both go through the node
//! (`library.versions.switch`, `library.versions.remove`) and are steps
//! of the library's history.

use egui::Ui;
use helios_core::Message;

use super::Library;

/// Removes one of a photo's versions, a copy, with the edits only it
/// holds (`image`, else the photo under the cursor; `version`, else its
/// current one), asked first unless `confirmed`; Ctrl+Z brings it back.
pub const REMOVE_VERSION_COMMAND: &str = "library.removeVersion";

/// One head of a photo, as `library.versions.list` answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionHead {
    /// Its version id (hex).
    pub version: String,
    /// Its place among the photo's heads, oldest first, from 1.
    pub number: usize,
    pub label: Option<String>,
    /// The head an edit opens.
    pub current: bool,
}

impl VersionHead {
    /// From one entry of `library.versions.list`'s `heads`.
    pub fn from_json(row: &serde_json::Value) -> Option<Self> {
        Some(Self {
            version: row["version"].as_str()?.to_string(),
            number: usize::try_from(row["number"].as_u64()?).ok()?,
            label: row["label"].as_str().map(String::from),
            current: row["current"].as_bool().unwrap_or(false),
        })
    }
}

/// What the strip or a command asked of a photo's versions, for the
/// host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionAsk {
    /// Make `version` the photo's current one.
    Switch { image: String, version: String },
    /// Remove `version`, a copy: asked first unless `confirmed`.
    Remove {
        image: String,
        version: String,
        confirmed: bool,
    },
}

impl Library {
    /// A photo's heads as the node listed them: the strip shows them.
    pub fn set_versions(&mut self, image: String, heads: Vec<VersionHead>) {
        if self.versions_asked.as_deref() == Some(image.as_str()) {
            self.versions_asked = None;
        }
        self.versions = Some((image, heads));
    }

    /// The heads listed for `image`, when they are.
    pub fn versions_of(&self, image: &str) -> Option<&[VersionHead]> {
        self.versions
            .as_ref()
            .filter(|(id, _)| id == image)
            .map(|(_, heads)| heads.as_slice())
    }

    /// Forgets the heads listed: read again when the strip shows.
    pub fn versions_changed(&mut self) {
        self.versions = None;
        self.versions_asked = None;
    }

    /// The photo whose heads the strip wants listed, once.
    pub fn take_versions_wanted(&mut self) -> Option<String> {
        self.versions_wanted.take()
    }

    /// Asks `version` made the current one of `image`, for the host.
    ///
    /// # Errors
    /// `image` is not in the library.
    pub fn switch_version(&mut self, image: &str, version: &str) -> Result<(), String> {
        if !self.holds(image) {
            return Err(format!("{image} is not in the library"));
        }
        self.version_asks.push(VersionAsk::Switch {
            image: image.to_string(),
            version: version.to_string(),
        });
        Ok(())
    }

    /// Asks a copy removed: `image`, else the photo under the cursor;
    /// `version`, else its current one (from the heads listed). Answers
    /// the photo and the version.
    ///
    /// # Errors
    /// No photo, a photo with one version, or its heads not listed yet.
    pub fn ask_remove_version(
        &mut self,
        image: Option<&str>,
        version: Option<&str>,
        confirmed: bool,
    ) -> Result<(String, String), String> {
        let image = match image {
            Some(id) if self.holds(id) => id.to_string(),
            Some(id) => return Err(format!("{id} is not in the library")),
            None => self
                .selected
                .and_then(|i| self.entries.get(i))
                .map(|e| e.id.clone())
                .ok_or("no photo under the cursor")?,
        };
        if self.heads_of(&image).unwrap_or(1) < 2 {
            return Err("the photo has one version: there is no copy to remove".into());
        }
        let version = match version {
            Some(v) => v.to_string(),
            None => self
                .versions_of(&image)
                .and_then(|heads| heads.iter().find(|h| h.current))
                .map(|h| h.version.clone())
                .ok_or("the photo's versions are not read yet: name `version`")?,
        };
        self.version_asks.push(VersionAsk::Remove {
            image: image.clone(),
            version: version.clone(),
            confirmed,
        });
        Ok((image, version))
    }

    /// What was asked of photos' versions since.
    pub fn take_version_asks(&mut self) -> Vec<VersionAsk> {
        std::mem::take(&mut self.version_asks)
    }

    /// The versions strip of the photo under the cursor, when it has more
    /// than one: each head with its number and name, the current one
    /// highlighted; a click makes it current.
    pub(super) fn versions_strip(&mut self, ui: &mut Ui, image: &str) {
        if self.heads_of(image).unwrap_or(1) < 2 {
            return;
        }
        let Some(heads) = self.versions_of(image).map(<[VersionHead]>::to_vec) else {
            if self.versions_asked.as_deref() != Some(image) {
                self.versions_asked = Some(image.to_string());
                self.versions_wanted = Some(image.to_string());
            }
            return;
        };
        let title = crate::i18n::text(
            ui.ctx(),
            Message::new("gui-library-versions-strip", "Versions"),
        );
        let copy = crate::i18n::text(ui.ctx(), Message::new("gui-library-version-copy", "Copy"));
        let texture = self.textures.get(image).map(|t| (t.id(), t.size_vec2()));
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(title);
            for head in &heads {
                let name = format!(
                    "{} {}",
                    head.number,
                    head.label.as_deref().map_or(copy.as_str(), |l| {
                        if l == helios_vault::versions::DUPLICATE_LABEL {
                            copy.as_str()
                        } else {
                            l
                        }
                    })
                );
                let button = match texture {
                    Some((id, size)) => {
                        let scale = 32.0 / size.x.max(size.y).max(1.0);
                        egui::Button::image_and_text(
                            egui::Image::new(egui::load::SizedTexture::new(id, size * scale)),
                            name,
                        )
                    }
                    None => egui::Button::new(name),
                }
                .selected(head.current);
                if ui.add(button).clicked() && !head.current {
                    picked = Some(head.version.clone());
                }
            }
        });
        if let Some(version) = picked
            && let Err(e) = self.switch_version(image, &version)
        {
            self.notes.push(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heads() -> Vec<VersionHead> {
        vec![
            VersionHead {
                version: "v1".into(),
                number: 1,
                label: None,
                current: false,
            },
            VersionHead {
                version: "v2".into(),
                number: 2,
                label: Some("Duplicate".into()),
                current: true,
            },
        ]
    }

    #[test]
    fn the_strip_lists_heads_switches_and_asks_a_copy_removed() {
        let mut lib = Library::default();
        lib.set_entries(
            ["a", "b", "c"]
                .into_iter()
                .map(|id| crate::library::Entry {
                    id: id.into(),
                    name: format!("{id}.jpg"),
                    ..crate::library::Entry::default()
                })
                .collect(),
        );
        assert!(
            lib.ask_remove_version(Some("b"), None, false).is_err(),
            "one version"
        );
        lib.set_heads_of("b", 2);
        assert!(
            lib.ask_remove_version(Some("b"), None, false).is_err(),
            "the heads not read yet"
        );
        lib.set_versions("b".into(), heads());
        assert_eq!(lib.versions_of("b").map(<[_]>::len), Some(2));
        assert_eq!(lib.versions_of("c"), None);
        lib.switch_version("b", "v1").unwrap();
        assert!(lib.switch_version("nope", "v1").is_err());
        assert_eq!(
            lib.ask_remove_version(Some("b"), None, false),
            Ok(("b".into(), "v2".into())),
            "the current one by default"
        );
        assert_eq!(
            lib.take_version_asks(),
            [
                VersionAsk::Switch {
                    image: "b".into(),
                    version: "v1".into()
                },
                VersionAsk::Remove {
                    image: "b".into(),
                    version: "v2".into(),
                    confirmed: false
                }
            ]
        );
        lib.versions_changed();
        assert_eq!(lib.versions_of("b"), None, "read again");
        let row =
            serde_json::json!({ "version": "v9", "number": 3, "label": null, "current": true });
        assert_eq!(
            VersionHead::from_json(&row),
            Some(VersionHead {
                version: "v9".into(),
                number: 3,
                label: None,
                current: true
            })
        );
    }
}
