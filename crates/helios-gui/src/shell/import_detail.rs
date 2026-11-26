//! The import window's Detail column, the right zone
//! (`docs/design/import-window.md`): the selected source's connector in
//! detail, as Lightroom's and darktable's import dialogs keep their
//! options on the right of the grid.
//!
//! - **At the top, the choices every source shares**: duplicates, the
//!   darktable edits (when the connector honours them), the camera's
//!   default look (when the library keeps one), More.
//! - **Below, the connector's own section**, under its name: what the
//!   connector declares calls for each part ([`ImportWindow::detail`]),
//!   one contract for every connector (folders, cards, Google Photos, the
//!   plugins'):
//!   - an account connector: its account, and for a connector whose
//!     photos are chosen in its service's page, the connection and
//!     "Choose photos" ([`super::import_accounts::choose_ui`]);
//!   - copy or add where they are (a connector that cannot add in place
//!     always copies), then Into and Name for a copy; none for a
//!     connector whose source contract declares a `download` folder
//!     (Google Photos): its photos go there, never asked;
//!   - the connector's own parameters: the keys of its source contract
//!     that are not choices, drawn from the contract with the same
//!     widgets as Settings and the "Add a source…" wizard, kept on the
//!     source as they change.
//! - **The backup line**, last.
//!
//! The left zone lists the sources, the middle one shows their photos.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use egui_pop::core::ui::UiPop as _;
use helios_core::Message;
use helios_core::config::Level;

use super::import_sources::SourceKey;

/// The camera photo's width in the detail zone, in points.
const PHOTO_WIDTH: f32 = 96.0;

/// The setting turning the cameras' photos from Wikimedia Commons on or
/// off (`helios_shell::settings`): off, the drawings only.
pub const CAMERA_PHOTOS_SETTING: &str = "import.camera_photos";

/// Whether the cameras' photos are on, as the Settings app keeps the
/// setting (on by default).
pub(super) fn camera_photos_on(app: &egui_pop::api::app::AppHandle) -> bool {
    let store = app.store();
    let store = store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store
        .get(&format!("settings/{CAMERA_PHOTOS_SETTING}"))
        .and_then(|e| e.value.as_bool())
        .unwrap_or(true)
}
use super::import_window::ImportWindow;
use super::import_wizard::{contract_ui, own_params, refused_ui};

/// The source contract's key naming the folder a connector's photos are
/// downloaded into (Google Photos': `<Pictures>/Google Photos/{account}`):
/// shown pre-filled in its Parameters, never asked during the flow.
pub(super) const DOWNLOAD: &str = "download";

/// A `download` pattern made a folder: `<Pictures>` the person's
/// pictures folder (`pictures`), `{account}` the account's address or
/// name, made fit for a folder name. `None` for an empty pattern.
pub(super) fn download_folder(pattern: &str, pictures: &Path, account: &str) -> Option<PathBuf> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    let account: String = account
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let text = pattern.replace("{account}", account.trim());
    Some(match text.strip_prefix("<Pictures>") {
        Some(rest) => pictures.join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(text),
    })
}

/// The person's pictures folder, as the platform names it
/// (`helios_library::import::pictures_folder`).
pub(super) fn pictures() -> PathBuf {
    helios_library::import::pictures_folder().unwrap_or_else(|| PathBuf::from("Pictures"))
}

/// One part of a connector's section, as its declarations call for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Part {
    /// An account's source: its connection, and "Choose photos" when the
    /// connector's photos are chosen in its service's page (`picker`).
    Account { id: String, picker: bool },
    /// Copy or add where they are; Into and Name for a copy.
    Copy,
    /// The connector's own parameters (its source contract's keys that
    /// are not choices).
    Params,
}

/// The selected source's connector section: its name and its parts, in
/// the order drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Detail {
    /// The source it is for: the account clicked last, else the folder
    /// ticked last; none before any is.
    pub key: Option<SourceKey>,
    /// Its connector's id.
    pub connector: String,
    /// Its connector's name, translated: the section's heading.
    pub label: String,
    pub parts: Vec<Part>,
    /// The camera its connector declares on the source: its name and its
    /// kind's drawing head the section.
    pub camera: Option<helios_connect::camera::Camera>,
}

impl ImportWindow {
    /// The Detail column's connector section for the selected source,
    /// from what its connector declares: its browsers (a service's page
    /// to choose in), its source contract (its own parameters). Every
    /// connector goes through it; none draws the column itself.
    pub(super) fn detail(&mut self) -> Detail {
        let key = self
            .sources
            .account
            .clone()
            .map(SourceKey::Account)
            .or_else(|| self.ticked.last().cloned().map(SourceKey::Folder));
        let connector = match &key {
            Some(k) => self.connector_of(k),
            None => self
                .connector_asking(helios_connect::SetupAsks::Path)
                .unwrap_or_default(),
        };
        let provider = self.connectors.provider(&connector);
        let label = provider.as_ref().map_or_else(
            || connector.clone(),
            |p| super::import_accounts::translated(self.texts.as_ref(), &p.manifest().label),
        );
        let mut parts = Vec::new();
        if let Some(SourceKey::Account(id)) = &key {
            let picker = provider.as_ref().is_some_and(|p| {
                p.manifest()
                    .browsers
                    .contains(&helios_connect::Browser::Picker)
            });
            parts.push(Part::Account {
                id: id.clone(),
                picker,
            });
        }
        let downloads = provider
            .as_ref()
            .is_some_and(|p| p.config().source.get(DOWNLOAD).is_some());
        if !downloads {
            parts.push(Part::Copy);
        }
        if key.is_some()
            && provider
                .as_ref()
                .is_some_and(|p| !own_params(&p.config().source).is_empty())
        {
            parts.push(Part::Params);
        }
        let camera = match &key {
            Some(SourceKey::Folder(path)) => self.camera_at(path).cloned(),
            _ => None,
        };
        Detail {
            key,
            connector,
            label,
            parts,
            camera,
        }
    }

    /// The folder the photos chosen under `account` are downloaded into:
    /// its source's `download` value, else its connector's default, made
    /// a folder ([`download_folder`]). `None` when its connector declares
    /// no `download`.
    pub(super) fn download_folder(&mut self, account: &str) -> Option<PathBuf> {
        let key = SourceKey::Account(account.to_owned());
        let connector = self.connector_of(&key);
        let provider = self.connectors.provider(&connector)?;
        let contract = provider.config().source;
        let declared = contract.get(DOWNLOAD)?;
        let saved = self
            .sources
            .get(&key)
            .and_then(|s| s.params.get(DOWNLOAD))
            .and_then(Value::as_str)
            .filter(|p| !p.trim().is_empty());
        let pattern = saved
            .or_else(|| declared.default.as_ref().and_then(Value::as_str))
            .unwrap_or("<Pictures>/{account}");
        let who = self
            .accounts
            .listed()
            .iter()
            .find(|a| a.id == account)
            .map_or_else(
                || account.to_owned(),
                |a| a.email().unwrap_or(&a.label).to_owned(),
            );
        download_folder(pattern, &pictures(), &who)
    }

    /// The camera of `detail`'s source, under its heading: its credited
    /// photo from Wikimedia Commons once one is ready
    /// ([`helios_connect::camera_photo`]), else its kind's drawing, named
    /// for the camera; then its name, and the photo's credit.
    pub(super) fn detail_camera_ui(&mut self, ui: &mut egui::Ui, detail: &Detail) {
        use egui_pop::core::icon::{Icon, Size, registry};
        use egui_pop::core::tooltip::TooltipExt as _;
        let Some(camera) = &detail.camera else {
            return;
        };
        let name = camera.name();
        let photo = self
            .camera_photos
            .photo(camera)
            .and_then(|photo| Some((self.camera_texture(ui.ctx(), &photo)?, photo.credit)));
        let say = |m: Message| match self.texts.as_ref() {
            Some(t) => t(m),
            None => m.english,
        };
        let picture = say(
            Message::new("gui-import-camera-picture", "Camera picture: { $camera }")
                .with("camera", name.clone()),
        );
        let credit = photo.as_ref().map(|(_, credit)| {
            say(Message::new(
                "gui-import-camera-photo-credit",
                "Photo: { $author }, { $licence }, Wikimedia Commons",
            )
            .with("author", credit.author.clone())
            .with("licence", credit.licence.clone()))
        });
        ui.horizontal(|ui| {
            if let (Some((texture, _)), Some(credit)) = (&photo, &credit) {
                let size = texture.size_vec2();
                let size = size * (PHOTO_WIDTH / size.x.max(1.0));
                ui.add(egui::Image::new((texture.id(), size)).alt_text(&picture))
                    .tip(credit);
            } else {
                let icon = camera
                    .kind()
                    .and_then(|k| registry::lookup(&crate::camera_icons::icon(k)))
                    .or_else(|| registry::lookup("phosphor:sd-card"))
                    .unwrap_or(Icon::Folder);
                ui.add(icon.size(Size::Panel).label(&picture));
            }
            ui.strong_text(name);
        });
        if let Some(credit) = credit {
            ui.meta(credit);
        }
    }

    /// The texture of a camera's photo, made once per Commons file; `None`
    /// when its picture does not decode (the drawing stays).
    fn camera_texture(
        &mut self,
        ctx: &egui::Context,
        photo: &helios_connect::camera_photo::Photo,
    ) -> Option<egui::TextureHandle> {
        let file = &photo.credit.file;
        if let Some(texture) = self.camera_textures.get(file) {
            return Some(texture.clone());
        }
        let rgba = match image::load_from_memory(&photo.bytes) {
            Ok(picture) => picture.to_rgba8(),
            Err(e) => {
                log::warn!("camera photo {file}: {e}; its drawing stays");
                self.camera_photos.forget(file);
                return None;
            }
        };
        let (w, h) = rgba.dimensions();
        let picture = egui::ColorImage::from_rgba_unmultiplied(
            [w as usize, h as usize],
            rgba.as_flat_samples().as_slice(),
        );
        let texture = ctx.load_texture(
            format!("helios.import.camera-photo.{file}"),
            picture,
            egui::TextureOptions::LINEAR,
        );
        self.camera_textures.insert(file.clone(), texture.clone());
        Some(texture)
    }

    /// The connector's own parameters of the source of `detail`: its
    /// saved values over the contract's defaults, kept on the source as
    /// they change, once they fit the contract.
    pub(super) fn detail_params_ui(&mut self, ui: &mut egui::Ui, detail: &Detail) {
        let (Some(key), Some(texts)) = (detail.key.clone(), self.texts.clone()) else {
            return;
        };
        let Some(provider) = self.connectors.provider(&detail.connector) else {
            return;
        };
        let contract = own_params(&provider.config().source);
        let mut values: Map<String, Value> = contract.defaults();
        if let Some(saved) = self.sources.get(&key) {
            for (k, v) in &saved.params {
                if contract.get(k).is_some() {
                    values.insert(k.clone(), v.clone());
                }
            }
        }
        let before = values.clone();
        // Whose catalogue translates them: the plugin behind the
        // connector, else the connector.
        let owner = self
            .accounts
            .plugins
            .as_ref()
            .and_then(|b| helios_plugin::source::plugin_of(b, &detail.connector))
            .unwrap_or_else(|| detail.connector.clone());
        let t = |m: Message| texts(m);
        ui.strong_text(t(Message::new("gui-editor-params", "Parameters")));
        let fit = contract_ui(ui, &t, &owner, Level::Source, &contract, &mut values);
        let fits = fit.is_ok();
        refused_ui(ui, &t, fit);
        if fits && values != before {
            self.sources
                .set_params(&key, &detail.connector, values.into_iter().collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_pattern_names_the_account_s_folder_in_the_pictures() {
        let pictures = Path::new("/home/ann/Pictures");
        assert_eq!(
            download_folder(
                "<Pictures>/Google Photos/{account}",
                pictures,
                "ann@example.org"
            ),
            Some(PathBuf::from(
                "/home/ann/Pictures/Google Photos/ann@example.org"
            ))
        );
        // An account name never makes a path of its own.
        assert_eq!(
            download_folder("/photos/{account}", pictures, "a/b:c"),
            Some(PathBuf::from("/photos/a_b_c"))
        );
        assert_eq!(download_folder("  ", pictures, "ann"), None);
    }
}
