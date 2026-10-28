//! The system clipboard beyond text: files and images, read from and
//! written for other applications.
//!
//! egui-pop's clipboard writes and reads the system clipboard as text
//! only (egui's own bridge: the fallbacks a copy offers, the text the
//! paste key brings), until egui-pop has a system backend of its own
//! (ADR 0015, step 5). What Helios needs beyond text goes through this
//! seam, [`SystemClipboard`], kept by the shell:
//!
//! - **Copy files** (`library.copyFiles`): the files of the images the
//!   grid or the filmstrip acts on, as a file manager copies them
//!   (`text/uri-list` on Linux, `CF_HDROP` on Windows, file URLs on
//!   macOS). Only on this explicit command: a plain copy of images gives
//!   other applications their names.
//! - **Copy the image** (`viewport.copyImage`): the rendered image as
//!   8-bit sRGB, which other applications read as PNG.
//! - **Paste from other applications** (`library.pasteFiles`): files a
//!   file manager copied, or an image a browser or an editor copied,
//!   read here and handed to egui-pop's paste as an external item
//!   ([`external_item`]) in a slot of their own ([`SLOT`]), so they go
//!   through the same step, sanitisers and target as any paste.
//!
//! [`Arboard`] is the backend of a desktop (arboard, already in the tree
//! through egui-winit); [`Memory`] stands in for it in tests. When egui-pop
//! gets its own backend, the commands keep their names and this module
//! goes.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui_pop::api::clipboard::{ClipItem, PNG, TEXT, URI_LIST};
use image::RgbaImage;
use serde_json::{Value as Json, json};

/// The owner's slot an external item read here is pasted from: it never
/// replaces what the user copied in Helios.
pub const SLOT: &str = "system";

/// The system clipboard's files and images.
pub trait SystemClipboard {
    /// The files it holds (copied in a file manager).
    fn files(&mut self) -> Result<Vec<PathBuf>, String>;
    /// The image it holds.
    fn image(&mut self) -> Result<RgbaImage, String>;
    /// Puts files on it, replacing what it held.
    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), String>;
    /// Puts an image on it, replacing what it held.
    fn set_image(&mut self, image: &RgbaImage) -> Result<(), String>;
}

/// The backend the shell and the library's endpoint share.
pub type Shared = Rc<RefCell<Box<dyn SystemClipboard>>>;

/// The desktop's backend, over arboard: opened at its first use (a
/// headless run that never copies files never opens it), then kept, so
/// that on X11 what Helios wrote stays served.
#[derive(Default)]
pub struct Arboard {
    clipboard: Option<arboard::Clipboard>,
}

impl Arboard {
    fn open(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.clipboard.is_none() {
            let c = arboard::Clipboard::new()
                .map_err(|e| format!("the system clipboard cannot be reached: {e}"))?;
            self.clipboard = Some(c);
        }
        Ok(self.clipboard.as_mut().expect("opened above"))
    }
}

/// An arboard error, said for a toast.
fn said(e: &arboard::Error, what: &str) -> String {
    match e {
        arboard::Error::ContentNotAvailable => format!("the system clipboard holds no {what}"),
        e => format!("the system clipboard: {e}"),
    }
}

impl SystemClipboard for Arboard {
    fn files(&mut self) -> Result<Vec<PathBuf>, String> {
        self.open()?
            .get()
            .file_list()
            .map_err(|e| said(&e, "files"))
    }

    fn image(&mut self) -> Result<RgbaImage, String> {
        let data = self.open()?.get_image().map_err(|e| said(&e, "image"))?;
        let (w, h) = (
            u32::try_from(data.width).map_err(|e| e.to_string())?,
            u32::try_from(data.height).map_err(|e| e.to_string())?,
        );
        RgbaImage::from_raw(w, h, data.bytes.into_owned())
            .ok_or_else(|| "the system clipboard's image has a wrong size".into())
    }

    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), String> {
        self.open()?
            .set()
            .file_list(files)
            .map_err(|e| said(&e, "files"))
    }

    fn set_image(&mut self, image: &RgbaImage) -> Result<(), String> {
        let data = arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Borrowed(image.as_raw()),
        };
        self.open()?.set_image(data).map_err(|e| said(&e, "image"))
    }
}

/// What a system clipboard in memory holds: files or an image, one
/// replacing the other as on a real one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Held {
    pub files: Vec<PathBuf>,
    pub image: Option<RgbaImage>,
}

/// A system clipboard in memory, for tests: its clones share what it
/// holds, so a test keeps one to look at what Helios wrote.
#[derive(Clone, Debug, Default)]
pub struct Memory(Rc<RefCell<Held>>);

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    /// What it holds now.
    pub fn held(&self) -> Held {
        self.0.borrow().clone()
    }

    /// Holds files, as another application's copy would.
    pub fn hold_files(&self, files: &[PathBuf]) {
        *self.0.borrow_mut() = Held {
            files: files.to_vec(),
            image: None,
        };
    }

    /// Holds an image, as another application's copy would.
    pub fn hold_image(&self, image: RgbaImage) {
        *self.0.borrow_mut() = Held {
            files: Vec::new(),
            image: Some(image),
        };
    }
}

impl SystemClipboard for Memory {
    fn files(&mut self) -> Result<Vec<PathBuf>, String> {
        let files = self.0.borrow().files.clone();
        if files.is_empty() {
            return Err("the system clipboard holds no files".into());
        }
        Ok(files)
    }

    fn image(&mut self) -> Result<RgbaImage, String> {
        self.0
            .borrow()
            .image
            .clone()
            .ok_or_else(|| "the system clipboard holds no image".into())
    }

    fn set_files(&mut self, files: &[PathBuf]) -> Result<(), String> {
        self.hold_files(files);
        Ok(())
    }

    fn set_image(&mut self, image: &RgbaImage) -> Result<(), String> {
        self.hold_image(image.clone());
        Ok(())
    }
}

/// Files as `text/uri-list`: one `file://` URI per line, each byte
/// outside the unreserved set and `/` escaped, lines ended by CRLF
/// (RFC 2483).
pub fn uri_list(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|p| {
            let mut uri = String::from("file://");
            for b in p.to_string_lossy().bytes() {
                if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                    uri.push(b as char);
                } else {
                    let _ = write!(uri, "%{b:02X}");
                }
            }
            uri + "\r\n"
        })
        .collect()
}

/// What the system clipboard holds beyond text, as an external item for
/// egui-pop's paste: files as `text/uri-list` (their paths, one per
/// line, as its text), else an image written as a PNG file in `dir`
/// (`image/png`, whose data is `{file, width, height}`: JSON carries no
/// bytes, and an import needs a file).
pub fn external_item(backend: &mut dyn SystemClipboard, dir: &Path) -> Result<ClipItem, String> {
    let files = backend.files().unwrap_or_default();
    let mut item = if files.is_empty() {
        let image = backend
            .image()
            .map_err(|_| "the system clipboard holds no files and no image".to_string())?;
        let file = write_png(&image, dir)?;
        ClipItem::new(
            PNG,
            0,
            json!({ "file": file, "width": image.width(), "height": image.height() }),
            "an image",
        )
    } else {
        let title = match files.len() {
            1 => "a file".to_string(),
            n => format!("{n} files"),
        };
        let paths: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        ClipItem::new(URI_LIST, 0, Json::String(uri_list(&files)), &title)
            .fallback(TEXT, Json::String(paths.join("\n")))
    };
    item.external = true;
    item.origin.by = "system".into();
    Ok(item)
}

/// Writes an image pasted from another application as a PNG file in
/// `dir`, named after the time it came in.
pub fn write_png(image: &RgbaImage, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    let mut path = dir.join(format!("Pasted image {stamp}.png"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("Pasted image {stamp} {n}.png"));
    }
    image
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Where images pasted from other applications are written before they
/// are imported (and copied into the library folder).
pub fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("helios-pasted")
}

/// The file an `image/png` representation names.
pub fn png_file(data: &Json) -> Result<PathBuf, String> {
    data["file"]
        .as_str()
        .filter(|f| !f.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "the image names no file (`file`)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("helios-system-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_uri_list_escapes_what_a_uri_cannot_hold_and_reads_back() {
        let files = [
            PathBuf::from("/photos/a b.jpg"),
            PathBuf::from("/photos/café #1.jpg"),
        ];
        let list = uri_list(&files);
        assert_eq!(
            list,
            "file:///photos/a%20b.jpg\r\nfile:///photos/caf%C3%A9%20%231.jpg\r\n"
        );
        assert_eq!(super::super::images::paths_in(&list), files);
    }

    #[test]
    fn files_come_in_as_a_uri_list_with_their_paths_as_text() {
        let mut m = Memory::new();
        m.hold_files(&[
            PathBuf::from("/photos/a.jpg"),
            PathBuf::from("/photos/b.jpg"),
        ]);
        let item = external_item(&mut m, &dir("files")).unwrap();
        assert!(item.external);
        assert_eq!(item.kind, URI_LIST);
        assert_eq!(item.title, "2 files");
        assert_eq!(item.text(), Some("/photos/a.jpg\n/photos/b.jpg"));
        assert_eq!(
            item.payload,
            json!("file:///photos/a.jpg\r\nfile:///photos/b.jpg\r\n")
        );
    }

    #[test]
    fn an_image_comes_in_as_a_png_file() {
        let d = dir("image");
        let mut m = Memory::new();
        m.hold_image(RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255])));
        let item = external_item(&mut m, &d).unwrap();
        assert_eq!(item.kind, PNG);
        assert_eq!(
            (
                item.payload["width"].clone(),
                item.payload["height"].clone()
            ),
            (json!(3), json!(2))
        );
        let file = png_file(&item.payload).unwrap();
        assert!(file.starts_with(&d));
        let back = image::open(&file).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(2, 1).0, [10, 20, 30, 255]);
        // A second image in the same second gets a name of its own.
        let again = png_file(&external_item(&mut m, &d).unwrap().payload).unwrap();
        assert_ne!(again, file);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_but_text_is_said() {
        let mut m = Memory::new();
        assert_eq!(
            external_item(&mut m, &dir("none")).unwrap_err(),
            "the system clipboard holds no files and no image"
        );
        assert!(png_file(&json!({ "width": 3 })).is_err());
    }

    #[test]
    fn writing_replaces_what_it_held() {
        let m = Memory::new();
        let mut b: Box<dyn SystemClipboard> = Box::new(m.clone());
        b.set_files(&[PathBuf::from("/a.jpg")]).unwrap();
        assert_eq!(m.held().files, [PathBuf::from("/a.jpg")]);
        b.set_image(&RgbaImage::new(1, 1)).unwrap();
        assert!(m.held().files.is_empty());
        assert!(b.files().is_err());
        assert_eq!(b.image().unwrap().dimensions(), (1, 1));
    }
}
