//! `helios.images`: images of the library, by id.
//!
//! A copy takes the images selected in the grid or the filmstrip (the
//! cursor's image when none is): each one's id (its file's hash), its
//! name and where its file is. Other applications get the file names,
//! one per line; the files themselves only through "Copy files"
//! (`library.copyFiles`, [`super::system`]).
//!
//! A paste into the library is an import, asked first:
//!
//! - **Images copied in Helios.** An image the library holds already (the
//!   same hash) is a reference: nothing is copied, nothing imported. The
//!   others are imported from their files, as `library.import` places
//!   them. Paste special's parts choose how: `reference` imports the
//!   files where they are, `file` copies them into the library folder;
//!   without a choice the workspace's import source decides.
//! - **Files from another application** (`text/uri-list`, or text whose
//!   lines are absolute paths or `file://` URIs): the files and folders
//!   that exist are imported the same way. A text naming no file (a file
//!   manager that gives names only) pasted by the user takes the files
//!   the system clipboard holds beside it ([`super::system`]).
//! - **An image from another application** (`image/png`, written to a
//!   file first): imported and copied into the library folder, since the
//!   file it was written to is a temporary one.
//!
//! The import is a job, as `library.import` is, not an undo step.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui_pop::api::AppHandle;
use egui_pop::api::clipboard::{
    ClipItem, ClipSource, ClipTarget, FormatId, PNG, Part, Paste, TEXT, URI_LIST,
};
use egui_pop::model::commands::Origin;
use helios_core::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use super::IMAGES;
use crate::library::Library;

/// The type's version.
pub const VERSION: u32 = 1;

/// Paste special's part that imports the files where they are.
pub const REFERENCE: &str = "reference";

/// Paste special's part that copies the files into the library folder.
pub const FILE: &str = "file";

/// The command a paste asks to run once confirmed.
pub const IMPORT: &str = "library.importFiles";

/// One image copied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    /// The vault's id: its file's hash (hex).
    pub id: String,
    /// Its file name.
    pub name: String,
    /// Where its file is, when it was on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

/// What a copy of images holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub images: Vec<Picture>,
}

impl Clip {
    /// Its text fallback: the file names, one per line.
    pub fn text(&self) -> String {
        self.images
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What it is called ("an image", "3 images").
    pub fn title(&self) -> String {
        match self.images.len() {
            1 => "an image".into(),
            n => format!("{n} images"),
        }
    }

    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or_default()
    }

    pub fn from_json(data: &Json) -> Result<Clip, String> {
        serde_json::from_value(data.clone()).map_err(|e| format!("not images: {e}"))
    }

    /// The item a copy makes.
    pub fn item(&self) -> ClipItem {
        ClipItem::new(IMAGES, VERSION, self.to_json(), &self.title())
            .fallback(TEXT, Json::String(self.text()))
    }
}

/// Copies what the library's views act on: the selection, else the
/// cursor's image.
pub fn take(library: &Library) -> Result<Clip, String> {
    let images = library
        .targets()?
        .into_iter()
        .filter_map(|id| {
            let entry = library.entry(&id)?;
            Some(Picture {
                path: library.file(&id).map(Path::to_path_buf),
                name: entry.name.clone(),
                id,
            })
        })
        .collect::<Vec<_>>();
    if images.is_empty() {
        return Err("no image selected".into());
    }
    Ok(Clip { images })
}

/// Paste special's parts: how the files the library lacks come in.
pub fn parts(_: &Json) -> Vec<Part> {
    vec![
        Part::new(REFERENCE, "Import the files where they are"),
        Part::new(FILE, "Copy the files into the library folder"),
    ]
}

/// What a paste into the library does.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Plan {
    /// The images the library holds already, by name: references.
    pub known: Vec<String>,
    /// The files to import.
    pub import: Vec<PathBuf>,
    /// What cannot come in: no file at hand, or a path that is not there.
    pub missing: Vec<String>,
    /// Copied into the library folder (`Some(true)`), referenced where
    /// they are (`Some(false)`), or as the import source says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<bool>,
}

impl Plan {
    /// Says it in a line, for paste special's preview and a dry run.
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        if !self.import.is_empty() {
            let how = match self.copy {
                Some(true) => ", copied into the library folder",
                Some(false) => ", referenced where they are",
                None => "",
            };
            out.push(format!("imports {} files{how}", self.import.len()));
        }
        if !self.known.is_empty() {
            out.push(format!("{} already in the library", self.known.len()));
        }
        if !self.missing.is_empty() {
            out.push(format!("{} without a file", self.missing.len()));
        }
        out.join("; ")
    }

    /// The arguments of the import it asks for, once confirmed.
    pub fn import_args(&self) -> Json {
        let mut args = json!({ "paths": self.import });
        if let Some(copy) = self.copy {
            args["copy"] = json!(copy);
        }
        args
    }
}

/// How paste special's parts place the files: only one asked for
/// decides, both or none leave it to the import source.
fn placement(parts: Option<&[String]>) -> Option<bool> {
    let parts = parts?;
    let has = |p: &str| parts.iter().any(|x| x == p);
    match (has(REFERENCE), has(FILE)) {
        (true, false) => Some(false),
        (false, true) => Some(true),
        _ => None,
    }
}

/// Plans a paste of copied images: those `knows` are references, the
/// others are imported from their files when `exists`.
pub fn plan(
    clip: &Clip,
    knows: impl Fn(&str) -> bool,
    exists: impl Fn(&Path) -> bool,
    parts: Option<&[String]>,
) -> Plan {
    let mut plan = Plan {
        copy: placement(parts),
        ..Plan::default()
    };
    for image in &clip.images {
        if knows(&image.id) {
            plan.known.push(image.name.clone());
        } else {
            match &image.path {
                Some(p) if exists(p) => plan.import.push(p.clone()),
                _ => plan.missing.push(image.name.clone()),
            }
        }
    }
    plan
}

/// Plans a paste of files named by another application: those that
/// exist are imported.
pub fn plan_files(paths: Vec<PathBuf>, exists: impl Fn(&Path) -> bool) -> Plan {
    let mut plan = Plan::default();
    for p in paths {
        if exists(&p) {
            plan.import.push(p);
        } else {
            plan.missing.push(p.display().to_string());
        }
    }
    plan
}

/// The files a text names: each line an absolute path or a `file://`
/// URI (`text/uri-list`'s comments and other schemes left out).
pub fn paths_in(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            if let Some(rest) = l.strip_prefix("file://") {
                // `file:///p` and `file://localhost/p`; another host's
                // file is not ours to read.
                let path = rest.strip_prefix("localhost").unwrap_or(rest);
                path.starts_with('/')
                    .then(|| PathBuf::from(percent_decoded(path)))
            } else {
                let p = PathBuf::from(l);
                p.is_absolute().then_some(p)
            }
        })
        .collect()
}

/// A URI's path with its `%XX` escapes decoded (left as they are when
/// they do not make UTF-8).
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(&h), Some(&l)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Plans a paste of an image another application copied, written to
/// `file`: imported and copied into the library folder, the file being
/// a temporary one.
pub fn plan_image(file: PathBuf, exists: impl Fn(&Path) -> bool) -> Plan {
    let mut plan = plan_files(vec![file], exists);
    plan.copy = Some(true);
    plan
}

/// The library's endpoint: the grid's and the filmstrip's.
pub struct Endpoint {
    library: Rc<RefCell<Library>>,
    /// The system clipboard's files, for a text naming none.
    system: super::system::Shared,
}

impl Endpoint {
    pub fn new(library: Rc<RefCell<Library>>, system: super::system::Shared) -> Self {
        Endpoint { library, system }
    }

    /// What a copy takes now.
    pub fn clip(&self) -> Result<Clip, String> {
        let library = self
            .library
            .try_borrow()
            .map_err(|_| "the library is busy".to_string())?;
        take(&library)
    }

    /// What a paste of `p` would do.
    fn plan(&self, p: &Paste<'_>) -> Result<Plan, String> {
        let plan = if p.repr.format == IMAGES {
            let clip = Clip::from_json(&p.repr.data)?;
            let library = self
                .library
                .try_borrow()
                .map_err(|_| "the library is busy".to_string())?;
            plan(
                &clip,
                |id| library.entry(id).is_some(),
                Path::exists,
                p.parts,
            )
        } else if p.repr.format == PNG {
            plan_image(super::system::png_file(&p.repr.data)?, Path::exists)
        } else {
            let text = p.repr.text().unwrap_or_default();
            let mut paths = paths_in(text);
            // A file manager that gives the names only (macOS's Finder):
            // the files themselves are on the system clipboard beside
            // them. The user's paste only: an agent never reads it.
            if paths.is_empty() && p.item.external && *p.origin == Origin::User {
                paths = self.system.borrow_mut().files().unwrap_or_default();
            }
            if paths.is_empty() {
                return Err("the text names no file to import".into());
            }
            plan_files(paths, Path::exists)
        };
        if plan.import.is_empty() {
            return Err(if plan.missing.is_empty() {
                "the images are in the library already".into()
            } else {
                format!("{}: no file to import", plan.describe())
            });
        }
        Ok(plan)
    }
}

/// Says the grid or the filmstrip has the focus.
pub fn focus(app: &AppHandle, widget: egui::Id) {
    egui_pop::api::clipboard::bind(app, super::LIBRARY_ENDPOINT, widget);
}

impl ClipSource for Endpoint {
    fn copy(&self, _: &AppHandle) -> Option<ClipItem> {
        self.clip().ok().map(|c| c.item())
    }

    fn can_copy(&self, _: &AppHandle) -> bool {
        self.clip().is_ok()
    }
}

impl ClipTarget for Endpoint {
    fn accepts(&self, _: &AppHandle) -> Vec<FormatId> {
        vec![IMAGES, URI_LIST, PNG, TEXT]
    }

    fn paste(&self, app: &AppHandle, p: &Paste<'_>) -> Result<Json, String> {
        let plan = self.plan(p)?;
        if p.dry_run {
            return Ok(json!(plan.describe()));
        }
        // An import writes the vault and may copy files: asked first,
        // then run as a job.
        let ctx = app.egui();
        let tr = |m: Message| crate::i18n::text(&ctx, m);
        let count = plan.import.len();
        let detail = match plan.copy {
            Some(true) if p.repr.format == PNG => Message::new(
                "gui-paste-import-image",
                "The image is saved as a PNG file and copied into the library folder.",
            ),
            Some(true) => Message::new(
                "gui-paste-import-copy",
                "The files are copied into the library folder.",
            ),
            Some(false) => Message::new(
                "gui-paste-import-reference",
                "The files stay where they are.",
            ),
            None => Message::new(
                "gui-paste-import-source",
                "The files are placed as the workspace's import source says.",
            ),
        };
        crate::dialogs::request(
            &ctx,
            crate::dialogs::Request::Confirm(crate::dialogs::Confirm {
                title: tr(Message::new(
                    "gui-paste-import-title",
                    "Import the pasted files ({ $count })?",
                )
                .with("count", count)),
                detail: tr(detail),
                verb: tr(Message::new("gui-paste-import-verb", "Import")),
                danger: false,
                command: IMPORT.into(),
                args: plan.import_args(),
            }),
        );
        ctx.request_repaint();
        Ok(serde_json::to_value(&plan).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(id: &str, name: &str, path: Option<&str>) -> Picture {
        Picture {
            id: id.into(),
            name: name.into(),
            path: path.map(PathBuf::from),
        }
    }

    fn clip() -> Clip {
        Clip {
            images: vec![
                picture("aa", "IMG_0001.CR3", Some("/photos/IMG_0001.CR3")),
                picture("bb", "IMG_0002.CR3", Some("/photos/IMG_0002.CR3")),
                picture("cc", "IMG_0003.CR3", None),
            ],
        }
    }

    #[test]
    fn a_copy_reads_as_its_file_names_and_round_trips() {
        let c = clip();
        assert_eq!(c.text(), "IMG_0001.CR3\nIMG_0002.CR3\nIMG_0003.CR3");
        assert_eq!(c.title(), "3 images");
        assert_eq!(Clip::from_json(&c.to_json()).unwrap(), c);
        let item = c.item();
        assert_eq!(item.kind, IMAGES);
        assert_eq!(item.text(), Some(c.text().as_str()));
        assert!(Clip::from_json(&json!({ "images": 3 })).is_err());
    }

    #[test]
    fn images_the_library_holds_are_references_the_others_imported() {
        let p = plan(&clip(), |id| id == "aa", |_| true, None);
        assert_eq!(p.known, ["IMG_0001.CR3"]);
        assert_eq!(p.import, [PathBuf::from("/photos/IMG_0002.CR3")]);
        // No file at hand: it cannot come in.
        assert_eq!(p.missing, ["IMG_0003.CR3"]);
        assert_eq!(p.copy, None);
        assert_eq!(
            p.describe(),
            "imports 1 files; 1 already in the library; 1 without a file"
        );
        assert_eq!(
            p.import_args(),
            json!({ "paths": ["/photos/IMG_0002.CR3"] })
        );
        // A file no longer there is missing too.
        let p = plan(&clip(), |_| false, |_| false, None);
        assert!(p.import.is_empty());
        assert_eq!(p.missing.len(), 3);
    }

    #[test]
    fn paste_special_chooses_reference_or_copy() {
        let only = |p: &[&str]| p.iter().map(ToString::to_string).collect::<Vec<_>>();
        let file = only(&[FILE]);
        let p = plan(&clip(), |_| false, |_| true, Some(&file));
        assert_eq!(p.copy, Some(true));
        assert_eq!(p.import_args()["copy"], json!(true));
        let reference = only(&[REFERENCE]);
        assert_eq!(
            plan(&clip(), |_| false, |_| true, Some(&reference)).copy,
            Some(false)
        );
        let both = only(&[REFERENCE, FILE]);
        assert_eq!(plan(&clip(), |_| false, |_| true, Some(&both)).copy, None);
        let listed: Vec<String> = parts(&Json::Null).into_iter().map(|p| p.id).collect();
        assert_eq!(listed, [REFERENCE, FILE]);
    }

    #[test]
    fn text_from_other_applications_names_files_by_path_or_uri() {
        let text = "# copied\n/photos/a b.jpg\nfile:///photos/caf%C3%A9.jpg\n\
                    file://localhost/photos/c.jpg\nfile://other/photos/d.jpg\n\
                    https://example.org/e.jpg\nrelative/f.jpg\n";
        assert_eq!(
            paths_in(text),
            [
                PathBuf::from("/photos/a b.jpg"),
                PathBuf::from("/photos/café.jpg"),
                PathBuf::from("/photos/c.jpg"),
            ]
        );
        assert!(paths_in("+0.30 EV").is_empty());
        // A broken escape is kept as written.
        assert_eq!(percent_decoded("/a%2/b%zz"), "/a%2/b%zz");
        let p = plan_files(paths_in(text), |p| p.ends_with("c.jpg"));
        assert_eq!(p.import, [PathBuf::from("/photos/c.jpg")]);
        assert_eq!(p.missing.len(), 2);
    }

    #[test]
    fn an_image_from_another_application_is_copied_into_the_library() {
        let p = plan_image(PathBuf::from("/tmp/Pasted image.png"), |_| true);
        assert_eq!(p.import, [PathBuf::from("/tmp/Pasted image.png")]);
        assert_eq!(p.copy, Some(true));
        assert_eq!(p.import_args()["copy"], json!(true));
        let gone = plan_image(PathBuf::from("/tmp/gone.png"), |_| false);
        assert!(gone.import.is_empty());
        assert_eq!(gone.missing.len(), 1);
    }
}
