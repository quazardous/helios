//! The Library's Metadata section (`docs/design/library.md`, L13): the
//! selection's metadata, edited as a group.
//!
//! - **Read-only capture details** of the photo under the cursor: camera,
//!   lens, exposure and when it was taken, read through the node's
//!   `library.fields.get` when the cursor moves.
//! - **Editable fields** of the whole selection: title, description,
//!   creator, copyright, rating, colour labels, flag and keywords. Each
//!   shows the value the selected photos share, or "‹mixed›" when they
//!   differ. A value typed (Enter, or leaving the field) is set on every
//!   selected photo; a "‹mixed›" field left untouched keeps each photo's
//!   own value. A colour label or a keyword is added or taken off alone,
//!   the others kept.
//! - **One undo step per edit**: every edit goes through the node's
//!   `library.meta.set`, which answers the step the library's history
//!   records (Ctrl+Z, Ctrl+Shift+Z); a `command.batch` groups its edits
//!   into one step.
//! - **Templates** (Photo Mechanic's stamps): a named set of fields and
//!   keywords, kept per library (a library document through the node),
//!   saved from the fields the selection shares, applied to a selection,
//!   deleted.
//!
//! The same commands serve the panel, agents and scripts:
//! [`META_SET_COMMAND`], [`META_GET_COMMAND`] and the template commands.

use std::collections::BTreeMap;

use crate::library_write::WriteAsk;
use egui::Ui;
use egui_pop::core::input::Input;
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{TextRole, UiPop as _};
use egui_pop::core::widgets;
use egui_pop::views::library::ColorLabel;
use egui_pop::views::library::marks::RatingStars;
use egui_pop::views::library::tags::TagInput;
use egui_pop::views::library::{Flag, Marks};
use helios_core::Message;
use helios_ui::{Action, Scroll};
use helios_vault::meta::{COPYRIGHT, CREATOR, DESCRIPTION, TEXT_FIELDS, TITLE};
use serde_json::{Map, Value, json};

use super::{Entry, Library};

/// Sets metadata on photos as one undoable step (`fields`, `keywords`
/// with `add` and `remove`, `labels` with `add` and `remove`; `images`:
/// ids, else the selection).
pub const META_SET_COMMAND: &str = "library.metadata.set";
/// Reads the selection's metadata as the panel shows it: each field's
/// shared value, or `mixed`.
pub const META_GET_COMMAND: &str = "library.metadata.get";
/// Saves a template (`name`), from `fields` and `keywords` when given,
/// else from what the selection shares.
pub const TEMPLATE_SAVE_COMMAND: &str = "library.metadata.template.save";
/// Applies a template (`name`) to photos (`images`, else the selection):
/// one undoable step.
pub const TEMPLATE_APPLY_COMMAND: &str = "library.metadata.template.apply";
/// Deletes a template (`name`).
pub const TEMPLATE_DELETE_COMMAND: &str = "library.metadata.template.delete";
/// Lists the templates.
pub const TEMPLATE_LIST_COMMAND: &str = "library.metadata.template.list";

/// The text fields a template keeps (a title is a photo's own).
const TEMPLATE_FIELDS: [&str; 3] = [DESCRIPTION, CREATOR, COPYRIGHT];

/// A group edit sent to the node (`library.meta.set`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaEdit {
    pub images: Vec<String>,
    /// A value per field set on every photo (null clears).
    pub fields: Map<String, Value>,
    pub keywords_add: Vec<String>,
    pub keywords_remove: Vec<String>,
    pub labels_add: Vec<String>,
    pub labels_remove: Vec<String>,
}

impl MetaEdit {
    /// The node's params.
    pub fn params(&self) -> Value {
        json!({
            "images": self.images,
            "fields": self.fields,
            "keywords": { "add": self.keywords_add, "remove": self.keywords_remove },
            "labels": { "add": self.labels_add, "remove": self.labels_remove },
        })
    }

    fn is_empty(&self) -> bool {
        self.fields.is_empty()
            && self.keywords_add.is_empty()
            && self.keywords_remove.is_empty()
            && self.labels_add.is_empty()
            && self.labels_remove.is_empty()
    }
}

/// A metadata template: fields and keywords applied together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    pub name: String,
    pub fields: Map<String, Value>,
    pub keywords: Vec<String>,
}

impl Template {
    pub fn to_json(&self) -> Value {
        json!({ "name": self.name, "fields": self.fields, "keywords": self.keywords })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            name: v["name"].as_str()?.to_string(),
            fields: v["fields"].as_object().cloned().unwrap_or_default(),
            keywords: strings(&v["keywords"]),
        })
    }

    /// The templates of a library document (none: no template yet).
    pub fn list_of(doc: Option<&Value>) -> Vec<Self> {
        doc.and_then(|d| d["templates"].as_array())
            .into_iter()
            .flatten()
            .filter_map(Self::from_json)
            .collect()
    }

    /// The library document holding `list`.
    pub fn document(list: &[Self]) -> Value {
        json!({ "templates": list.iter().map(Self::to_json).collect::<Vec<_>>() })
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

/// A field across the selection: the value they share, or mixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shared<T> {
    Same(T),
    Mixed,
}

impl<T: PartialEq + Clone> Shared<T> {
    /// What `values` share (`None` for no value at all).
    pub fn of(mut values: impl Iterator<Item = T>) -> Option<Self> {
        let first = values.next()?;
        Some(if values.all(|v| v == first) {
            Self::Same(first)
        } else {
            Self::Mixed
        })
    }

    fn json(&self, f: impl Fn(&T) -> Value) -> Value {
        match self {
            Self::Same(v) => f(v),
            Self::Mixed => json!("mixed"),
        }
    }
}

/// A text field of an entry.
pub fn text_of<'a>(entry: &'a Entry, field: &str) -> &'a str {
    match field {
        TITLE => entry.title.as_deref(),
        DESCRIPTION => entry.description.as_deref(),
        CREATOR => entry.creator.as_deref(),
        COPYRIGHT => entry.copyright.as_deref(),
        _ => None,
    }
    .unwrap_or_default()
}

fn flag_word(flag: Flag) -> &'static str {
    match flag {
        Flag::Pick => "pick",
        Flag::Reject => "reject",
        Flag::None => "none",
    }
}

/// How many photos' capture details are held: each Library view shows
/// the photo under its own cursor, and one view must not drive another's
/// out.
const EXIF_HELD: usize = 8;

/// The panel's state, kept across frames, one for every view.
#[derive(Default)]
pub struct MetaPanel {
    /// The template chosen in the list.
    template: Option<String>,
    /// The name a new template is saved under.
    new_template: String,
    /// The photos whose capture details were asked, and them (empty until
    /// they land), the latest last.
    exif: Vec<(String, Map<String, Value>)>,
    /// Photos whose capture details the host is to read, in order.
    exif_wanted: Vec<String>,
}

/// What a view types in its Metadata section: each view its own, for its
/// own selection.
#[derive(Default)]
pub(super) struct MetaDrafts {
    /// The selection the drafts were made for.
    keys: Vec<String>,
    /// The text being typed in each field, by field.
    drafts: BTreeMap<&'static str, String>,
}

impl Library {
    /// Queues a group edit for the host (`library.write`);
    /// how many photos it names. `images`: those shown, else the
    /// selection.
    ///
    /// # Errors
    /// No photo, or nothing to set.
    pub fn set_meta(
        &mut self,
        images: Option<Vec<String>>,
        mut edit: MetaEdit,
    ) -> Result<usize, String> {
        edit.images = match images {
            Some(ids) => {
                let keys: Vec<String> = ids.into_iter().filter(|i| self.holds(i)).collect();
                if keys.is_empty() {
                    return Err("none of the `images` is in the library".into());
                }
                keys
            }
            None => self.targets()?,
        };
        if edit.is_empty() {
            return Err("nothing to set: no field, label or keyword".into());
        }
        let count = edit.images.len();
        self.ask_write(WriteAsk::Meta(
            edit,
            crate::library_history::current_batch(),
        ));
        Ok(count)
    }

    /// What the selection (else the cursor's photo) shares, as
    /// [`META_GET_COMMAND`] answers: each field's value or `"mixed"`.
    ///
    /// # Errors
    /// No photo selected.
    pub fn meta_of_selection(&self) -> Result<Value, String> {
        let keys = self.targets()?;
        let entries: Vec<&Entry> = keys.iter().filter_map(|k| self.entry(k)).collect();
        let mut out = Map::new();
        for field in TEXT_FIELDS {
            let shared = Shared::of(entries.iter().map(|e| text_of(e, field).to_string()));
            out.insert(
                field.into(),
                shared.map_or(Value::Null, |s| s.json(|t| json!(t))),
            );
        }
        let rating = Shared::of(entries.iter().map(|e| e.rating));
        out.insert(
            "rating".into(),
            rating.map_or(Value::Null, |s| s.json(|r| json!(r))),
        );
        let flag = Shared::of(entries.iter().map(|e| e.flag()));
        out.insert(
            "flag".into(),
            flag.map_or(Value::Null, |s| s.json(|f| json!(flag_word(*f)))),
        );
        let (all, some) = shared_names(entries.iter().map(|e| e.labels.as_slice()));
        out.insert("labels".into(), json!({ "all": all, "some": some }));
        let (all, some) = shared_names(entries.iter().map(|e| e.tags.as_slice()));
        out.insert("keywords".into(), json!({ "all": all, "some": some }));
        out.insert("photos".into(), json!(entries.len()));
        Ok(Value::Object(out))
    }

    /// The metadata templates, once read from the library.
    pub fn templates(&self) -> Option<&[Template]> {
        self.templates.as_deref()
    }

    /// Shows the templates read from the library.
    pub fn set_templates(&mut self, list: Vec<Template>) {
        self.templates = Some(list);
    }

    /// Whether the host is to read the templates (once).
    pub fn take_templates_wanted(&mut self) -> bool {
        std::mem::replace(&mut self.templates_wanted, super::Wanted::No) == super::Wanted::Yes
    }

    fn templates_mut(&mut self) -> Result<&mut Vec<Template>, String> {
        self.templates
            .as_mut()
            .ok_or_else(|| "the templates are not read yet".to_string())
    }

    /// Saves a template under `name` (replacing one of that name): the
    /// fields and keywords given, else those the selection shares.
    ///
    /// # Errors
    /// An empty name, templates not read yet, nothing to keep.
    pub fn save_template(
        &mut self,
        name: &str,
        given: Option<(Map<String, Value>, Vec<String>)>,
    ) -> Result<usize, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a template needs a name".into());
        }
        let (fields, keywords) = if let Some(g) = given {
            g
        } else {
            {
                let now = self.meta_of_selection()?;
                let fields: Map<String, Value> = TEMPLATE_FIELDS
                    .iter()
                    .filter_map(|f| {
                        let v = &now[*f];
                        (v.is_string() && v != "mixed" && v != "")
                            .then(|| ((*f).to_string(), v.clone()))
                    })
                    .collect();
                (fields, strings(&now["keywords"]["all"]))
            }
        };
        if fields.is_empty() && keywords.is_empty() {
            return Err("nothing to keep: no field or keyword the photos share".into());
        }
        let list = self.templates_mut()?;
        list.retain(|t| t.name != name);
        list.push(Template {
            name: name.to_string(),
            fields,
            keywords,
        });
        list.sort_by(|a, b| a.name.cmp(&b.name));
        let count = list.len();
        if let Some(list) = self.templates.clone() {
            self.ask_write(WriteAsk::Templates(list));
        }
        self.meta_panel.template = Some(name.to_string());
        Ok(count)
    }

    /// Applies the template `name` to photos (`images`, else the
    /// selection): one group edit; how many photos.
    ///
    /// # Errors
    /// No such template, no photo.
    pub fn apply_template(
        &mut self,
        name: &str,
        images: Option<Vec<String>>,
    ) -> Result<usize, String> {
        let t = self
            .templates_mut()?
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .ok_or_else(|| format!("no template “{name}”"))?;
        self.set_meta(
            images,
            MetaEdit {
                fields: t.fields,
                keywords_add: t.keywords,
                ..MetaEdit::default()
            },
        )
    }

    /// Deletes the template `name`; how many are left.
    ///
    /// # Errors
    /// No such template.
    pub fn delete_template(&mut self, name: &str) -> Result<usize, String> {
        let list = self.templates_mut()?;
        let before = list.len();
        list.retain(|t| t.name != name);
        if list.len() == before {
            return Err(format!("no template “{name}”"));
        }
        let count = list.len();
        if let Some(list) = self.templates.clone() {
            self.ask_write(WriteAsk::Templates(list));
        }
        if self.meta_panel.template.as_deref() == Some(name) {
            self.meta_panel.template = None;
        }
        Ok(count)
    }

    /// A photo whose capture details the host is to read, once.
    pub fn take_exif_wanted(&mut self) -> Option<String> {
        let wanted = &mut self.meta_panel.exif_wanted;
        (!wanted.is_empty()).then(|| wanted.remove(0))
    }

    /// Shows a photo's capture details, read by the host.
    pub fn set_exif(&mut self, image: String, fields: Map<String, Value>) {
        let exif = &mut self.meta_panel.exif;
        exif.retain(|(id, _)| *id != image);
        exif.push((image, fields));
        if exif.len() > EXIF_HELD {
            exif.remove(0);
        }
    }

    /// The Metadata section: the cursor photo's capture details, then the
    /// selection's fields, keywords and templates.
    pub(super) fn metadata_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        egui_pop::core::fold::Fold::new(
            "helios.library.metadata",
            word(Message::new("gui-library-metadata", "Metadata")),
        )
        .default_open(true)
        .show(ui, |ui| {
            // Its own scroll, a share of the panel's height: the grid
            // below keeps its room, and the panel never scrolls the
            // search field away.
            helios_ui::scroll(ui, "helios.library.metadata.scroll", Scroll::Form, |ui| {
                self.metadata_body(ui);
            });
            // The versions strip below the scroll, always in view.
            if let Ok(keys) = self.targets()
                && let [one] = keys.as_slice()
            {
                self.versions_strip(ui, one);
            }
        });
    }

    fn metadata_body(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let Ok(keys) = self.targets() else {
            ui.muted(word(Message::new(
                "gui-library-no-cursor",
                "No image under the cursor",
            )));
            return;
        };
        let drafts = &mut self.view.meta;
        if keys != drafts.keys {
            drafts.keys.clone_from(&keys);
            drafts.drafts.clear();
        }
        let entries: Vec<Entry> = keys.iter().filter_map(|k| self.entry(k).cloned()).collect();
        let Some(first) = entries.first() else {
            return;
        };
        let mixed = word(Message::new("gui-library-meta-mixed", "‹mixed›"));
        if entries.len() == 1 {
            ui.text_role(TextRole::Section, &first.name);
        } else {
            ui.text_role(
                TextRole::Section,
                word(
                    Message::new("gui-library-meta-photos", "{ $count } photos selected")
                        .with("count", entries.len()),
                ),
            );
        }
        let cursor = self
            .selected()
            .and_then(|i| self.entries.get(i))
            .map_or_else(|| first.id.clone(), |e| e.id.clone());
        self.exif_rows(ui, &cursor);
        let mut edit = MetaEdit::default();
        // Rating, labels and flag: the shared value, else none drawn and
        // "‹mixed›" beside.
        let rating = Shared::of(entries.iter().map(|e| e.rating.clamp(0, 5) as u8));
        let flag = Shared::of(entries.iter().map(Entry::flag));
        let marks = Marks {
            rating: match rating {
                Some(Shared::Same(r)) => r,
                _ => 0,
            },
            labels: first
                .marks()
                .labels
                .iter()
                .filter(|l| {
                    entries
                        .iter()
                        .all(|e| e.labels.iter().any(|n| n == l.name()))
                })
                .collect(),
            flag: match flag {
                Some(Shared::Same(f)) => f,
                _ => Flag::None,
            },
        };
        let some_labels = entries.iter().any(|e| e.labels != first.labels);
        ui.horizontal_wrapped(|ui| {
            let tip = word(Message::new(
                "gui-library-meta-rating-tip",
                "Rating of the selected photos; a click sets it on all of them",
            ));
            let name = word(Message::new("gui-library-meta-rating", "Rating"));
            let out = RatingStars::new(marks.rating).label(name).show(ui);
            let _ = out.response.clone().tip(&tip);
            if let Some(r) = out.changed {
                edit.fields.insert("rating".into(), json!(r));
            }
            if matches!(rating, Some(Shared::Mixed)) {
                let _ = ui.muted(&mixed).tip(&tip);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let tip = word(Message::new(
                "gui-library-meta-labels-tip",
                "Colour labels: a click adds or removes one label on every selected photo, the others kept",
            ));
            let side = egui_pop::core::theme::Tokens::of(ui.ctx())
                .size
                .control_height;
            let out = self.color_labels(ui.ctx(), marks.labels, side).show(ui);
            if let Some(labels) = out.changed {
                for label in ColorLabel::ALL {
                    if labels.has(label) != marks.labels.has(label) {
                        let name = label.name().to_string();
                        if labels.has(label) {
                            edit.labels_add.push(name);
                        } else {
                            edit.labels_remove.push(name);
                        }
                    }
                }
            }
            if some_labels {
                let _ = ui.muted(&mixed).tip(&tip);
            }
        });
        // The flag on its own row: beside the labels it ran past a narrow
        // Library panel's edge.
        ui.horizontal_wrapped(|ui| {
            let tip = word(Message::new(
                "gui-library-meta-flag-tip",
                "Flag: pick or reject every selected photo",
            ));
            let side = egui_pop::core::theme::Tokens::of(ui.ctx())
                .size
                .control_height;
            let out = self.flag_toggle(ui.ctx(), marks.flag, side).show(ui);
            if let Some(f) = out.changed {
                edit.fields.insert("flag".into(), json!(flag_word(f)));
            }
            if matches!(flag, Some(Shared::Mixed)) {
                let _ = ui.muted(&mixed).tip(&tip);
            }
        });
        for field in TEXT_FIELDS {
            let shared = Shared::of(entries.iter().map(|e| text_of(e, field).to_string()));
            if let Some(value) = self.text_row(ui, field, shared, &mixed) {
                edit.fields.insert(field.into(), value);
            }
        }
        // Keywords: those every photo has; one added goes on all, one
        // taken off leaves all.
        let all: Vec<String> = first
            .tags
            .iter()
            .filter(|t| entries.iter().all(|e| e.tags.contains(t)))
            .cloned()
            .collect();
        let some = entries.iter().any(|e| e.tags.len() != all.len());
        let label = word(Message::new("gui-library-tags", "Keywords"));
        let out = TagInput::new("helios.library.tags")
            .label(label)
            .source(&self.known_tags)
            .show(ui, &mut self.tags, &all);
        edit.keywords_add.extend(out.added);
        edit.keywords_remove.extend(out.removed);
        if some {
            ui.muted(word(Message::new(
                "gui-library-meta-some-keywords",
                "Some photos have other keywords too: they are kept",
            )));
        }
        if !edit.is_empty() {
            run(ui.ctx(), META_SET_COMMAND, edit.params());
        }
        self.templates_row(ui);
    }

    /// A text field's row: its label and its edit, "‹mixed›" as a hint
    /// when the photos differ; the value to set once typed (Enter or
    /// leaving the field), none while unchanged.
    fn text_row(
        &mut self,
        ui: &mut Ui,
        field: &'static str,
        shared: Option<Shared<String>>,
        mixed: &str,
    ) -> Option<Value> {
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let (name, tip) = match field {
            TITLE => (
                Message::new("gui-library-meta-title", "Title"),
                Message::new(
                    "gui-library-meta-title-tip",
                    "The photos' title; Enter sets it on every selected photo",
                ),
            ),
            DESCRIPTION => (
                Message::new("gui-library-meta-description", "Description"),
                Message::new(
                    "gui-library-meta-description-tip",
                    "The photos' description (caption); leaving the field sets it on every selected photo",
                ),
            ),
            CREATOR => (
                Message::new("gui-library-meta-creator", "Creator"),
                Message::new(
                    "gui-library-meta-creator-tip",
                    "Who made the photos; Enter sets it on every selected photo",
                ),
            ),
            _ => (
                Message::new("gui-library-meta-copyright", "Copyright"),
                Message::new(
                    "gui-library-meta-copyright-tip",
                    "The photos' copyright notice; Enter sets it on every selected photo",
                ),
            ),
        };
        let (name, tip) = (word(name), word(tip));
        let shown = match &shared {
            Some(Shared::Same(t)) => t.clone(),
            _ => String::new(),
        };
        let draft = self
            .view
            .meta
            .drafts
            .entry(field)
            .or_insert_with(|| shown.clone());
        let mut out = None;
        ui.text_role(TextRole::Body, &name);
        let edit = if field == DESCRIPTION {
            Input::multiline(draft, &name)
        } else {
            Input::text(draft, &name)
        };
        let mut edit = edit.id_salt(format!("helios.library.meta.{field}"));
        if matches!(shared, Some(Shared::Mixed)) {
            edit = edit.placeholder(mixed);
        }
        let r = edit.show(ui).response.tip(&tip);
        let done = r.lost_focus();
        if done {
            let typed = draft.trim().to_string();
            // A "‹mixed›" field left empty keeps each photo's value.
            let untouched = matches!(shared, Some(Shared::Mixed)) && typed.is_empty();
            if typed != shown.trim() && !untouched {
                out = Some(if typed.is_empty() {
                    Value::Null
                } else {
                    json!(typed)
                });
            }
        }
        if !r.has_focus() && !done {
            // Not typing: show what the photos hold now.
            *draft = shown;
        }
        out
    }

    /// The capture details of `image`: asked once, shown once read.
    fn exif_rows(&mut self, ui: &mut Ui, image: &str) {
        use helios_vault::capture as c;
        let held = self.meta_panel.exif.iter().find(|(id, _)| id == image);
        let Some((_, f)) = held else {
            self.meta_panel.exif_wanted.push(image.to_string());
            // Not asked again until it lands.
            self.set_exif(image.to_string(), Map::new());
            return;
        };
        let text = |k: &str| f.get(k).and_then(Value::as_str).map(String::from);
        let num = |k: &str| f.get(k).and_then(Value::as_f64);
        let camera = [text(c::CAMERA_MAKE), text(c::CAMERA_MODEL)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let mut exposure = Vec::new();
        if let Some(a) = num(c::APERTURE) {
            exposure.push(format!("f/{a:.1}"));
        }
        if let Some(s) = num(c::SHUTTER) {
            exposure.push(if s > 0.0 && s < 1.0 {
                format!("1/{:.0} s", 1.0 / s)
            } else {
                format!("{s} s")
            });
        }
        if let Some(i) = num(c::ISO) {
            exposure.push(format!("ISO {i:.0}"));
        }
        if let Some(l) = num(c::FOCAL_LENGTH) {
            exposure.push(format!("{l:.0} mm"));
        }
        let taken = num(c::CAPTURED).and_then(|s| {
            chrono::DateTime::from_timestamp(s as i64, 0)
                .map(|d| d.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string())
        });
        let rows: Vec<(Message, String)> = [
            (
                Message::new("gui-library-meta-camera", "Camera"),
                Some(camera).filter(|c| !c.is_empty()),
            ),
            (Message::new("gui-library-meta-lens", "Lens"), text(c::LENS)),
            (
                Message::new("gui-library-meta-exposure", "Exposure"),
                Some(exposure.join(" · ")).filter(|e| !e.is_empty()),
            ),
            (Message::new("gui-library-meta-taken", "Taken"), taken),
        ]
        .into_iter()
        .filter_map(|(m, v)| v.map(|v| (m, v)))
        .collect();
        if rows.is_empty() {
            return;
        }
        let rows: Vec<(String, String)> = rows
            .into_iter()
            .map(|(m, v)| (crate::i18n::text(ui.ctx(), m), v))
            .collect();
        helios_ui::details(ui, "helios.library.meta.exif", &rows);
    }

    /// The templates: one to apply or delete, and the shared fields saved
    /// as a new one.
    fn templates_row(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let word = |m: Message| crate::i18n::text(&ctx, m);
        let names: Vec<String> = self
            .templates
            .iter()
            .flatten()
            .map(|t| t.name.clone())
            .collect();
        ui.horizontal_wrapped(|ui| {
            let tip = word(Message::new(
                "gui-library-meta-template-tip",
                "Metadata templates: named fields and keywords applied to the selection",
            ));
            let none = word(Message::new(
                "gui-library-meta-template-none",
                "No template",
            ));
            let chosen = self
                .meta_panel
                .template
                .clone()
                .filter(|t| names.contains(t));
            let combo = widgets::combo_box("helios.library.meta.template")
                .selected_text(chosen.clone().unwrap_or(none));
            let combo = ui
                .combo_box(combo, |ui| {
                    for n in &names {
                        widgets::selectable_value(
                            ui,
                            &mut self.meta_panel.template,
                            Some(n.clone()),
                            n,
                        );
                    }
                })
                .response;
            egui_pop::core::a11y::name(&combo, egui::Role::ComboBox, &tip);
            let _ = combo.tip(&tip);
            let apply = word(Message::new("gui-library-meta-template-apply", "Apply"));
            let apply_tip = word(Message::new(
                "gui-library-meta-template-apply-tip",
                "Apply the template to the selected photos (one undo step)",
            ));
            let r = ui
                .add_enabled_ui(chosen.is_some(), |ui| {
                    helios_ui::action(ui, Action::Secondary, &apply)
                })
                .inner
                .tip(&apply_tip);
            if r.clicked() {
                run(ui.ctx(), TEMPLATE_APPLY_COMMAND, json!({ "name": chosen }));
            }
            let delete = word(Message::new("gui-library-meta-template-delete", "Delete"));
            let delete_tip = word(Message::new(
                "gui-library-meta-template-delete-tip",
                "Delete the template chosen",
            ));
            let r = ui
                .add_enabled_ui(chosen.is_some(), |ui| {
                    helios_ui::action(ui, Action::Danger, &delete)
                })
                .inner
                .tip(&delete_tip);
            if r.clicked() {
                run(ui.ctx(), TEMPLATE_DELETE_COMMAND, json!({ "name": chosen }));
            }
        });
        ui.horizontal(|ui| {
            let name_tip = word(Message::new(
                "gui-library-meta-template-name-tip",
                "The new template's name",
            ));
            let hint = word(Message::new("gui-library-meta-template-name", "Template name"));
            let _ = Input::text(&mut self.meta_panel.new_template, &name_tip)
                .id_salt("helios.library.meta.template.name")
                .placeholder(&hint)
                .width(120.0)
                .show(ui)
                .response
                .tip(&name_tip);
            let save = word(Message::new("gui-library-meta-template-save", "Save as template"));
            let save_tip = word(Message::new(
                "gui-library-meta-template-save-tip",
                "Keep the description, creator, copyright and keywords the selected photos share as a template",
            ));
            let can = !self.meta_panel.new_template.trim().is_empty();
            let r = ui
                .add_enabled_ui(can, |ui| {
                    helios_ui::action(ui, Action::Secondary, &save)
                })
                .inner
                .tip(&save_tip);
            if r.clicked() {
                let name = std::mem::take(&mut self.meta_panel.new_template);
                run(ui.ctx(), TEMPLATE_SAVE_COMMAND, json!({ "name": name.trim() }));
            }
        });
    }
}

/// The names every list holds, and those only some hold.
fn shared_names<'a>(mut lists: impl Iterator<Item = &'a [String]>) -> (Vec<String>, Vec<String>) {
    let Some(first) = lists.next() else {
        return (Vec::new(), Vec::new());
    };
    let mut all: Vec<String> = first.to_vec();
    let mut some: Vec<String> = first.to_vec();
    for l in lists {
        all.retain(|n| l.contains(n));
        some.extend(l.iter().cloned());
    }
    some.sort_unstable();
    some.dedup();
    some.retain(|n| !all.contains(n));
    (all, some)
}

/// Runs a command as the user, as the panel's controls do.
fn run(ctx: &egui::Context, id: &str, args: Value) {
    crate::dialogs::request(
        ctx,
        crate::dialogs::Request::Run(id.into(), args, helios_shell::commands::Origin::User),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_values_and_names() {
        assert_eq!(Shared::of([1, 1].into_iter()), Some(Shared::Same(1)));
        assert_eq!(Shared::of([1, 2].into_iter()), Some(Shared::Mixed));
        assert_eq!(Shared::<i32>::of([].into_iter()), None);
        let a = vec!["x".to_string(), "y".to_string()];
        let b = vec!["y".to_string(), "z".to_string()];
        let (all, some) = shared_names([a.as_slice(), b.as_slice()].into_iter());
        assert_eq!(all, vec!["y"]);
        assert_eq!(some, vec!["x", "z"]);
        let t = Template {
            name: "Wedding".into(),
            fields: [("creator".to_string(), json!("Ann"))]
                .into_iter()
                .collect(),
            keywords: vec!["Wedding|2026".into()],
        };
        assert_eq!(
            Template::list_of(Some(&Template::document(std::slice::from_ref(&t)))),
            vec![t]
        );
    }
}
