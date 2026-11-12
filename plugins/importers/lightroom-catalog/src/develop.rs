//! Lightroom's develop settings: read, then mapped onto Helios modules.
//!
//! # Where they are
//!
//! Lightroom Classic keeps a photo's current develop settings in the
//! catalogue, `Adobe_imageDevelopSettings (image, text)`, `text` a Lua
//! table literal (`s = { Exposure2012 = 0.35, WhiteBalance = "As Shot",
//! ToneCurvePV2012 = { 0, 0, 255, 255 }, … }`) whose keys are Camera Raw's
//! setting names. The format is not documented by Adobe; it is the one
//! read by `lrcat-extractor` (Hubert Figuière,
//! <https://github.com/hfiguiere/lrcat-extractor>, its Lua-object parser
//! `lron` and `develop` settings) and the `Lightroom-SQL-tools` scripts
//! (<https://github.com/fdenivac/Lightroom-SQL-tools>). The same settings
//! go to `.xmp` sidecars as attributes (or elements) of the Camera Raw
//! namespace `crs:` (`http://ns.adobe.com/camera-raw-settings/1.0/`),
//! listed with their types in `ExifTool`'s tag tables
//! (<https://exiftool.org/TagNames/XMP.html#crs>), the photo's orientation
//! beside them as `tiff:Orientation` (an EXIF value).
//!
//! The orientation is not a develop setting: the catalogue keeps it in
//! `Adobe_images.orientation` as two letters, the corners of the stored
//! image that land top-left and top-right (`AB` upright, `BC` turned 90
//! degrees clockwise, `CD` 180 degrees, `DA` 90 degrees counter-clockwise;
//! `lrcat-extractor`'s `Image::exif_orientation`).
//!
//! # The basic panel only, and only faithful equivalents
//!
//! Read: `Exposure2012`, `Contrast2012`, `Highlights2012`,
//! `Shadows2012`, `Whites2012`, `Blacks2012`, `Texture`, `Clarity2012`,
//! `Dehaze`, `Vibrance`, `Saturation`, `WhiteBalance` with `Temperature`
//! and `Tint` (`IncrementalTemperature`/`IncrementalTint` on a non-raw
//! file), the crop (`HasCrop`, `CropTop`, `CropLeft`, `CropBottom`,
//! `CropRight`, `CropAngle`) and the orientation.
//!
//! Mapped, where Helios has the same operation:
//!
//! - `Exposure2012` to `exposure`'s `exposure`, in EV (stops) both;
//! - the orientation to `flip`'s `orientation`, darktable's orientation
//!   bits, set by the user so it overrides the camera's;
//! - `WhiteBalance = "As Shot"` on a raw file to `whitebalance` in its
//!   `as_shot` mode, the camera's coefficients (on a non-raw file, as shot
//!   is no change at all).
//!
//! Everything else read and not at its neutral value is *unmatched*:
//! listed with its value, never approximated. Helios's tone and colour
//! modules (sigmoid, colour calibration) do not do what Lightroom's
//! sliders do, white balance by temperature and tint needs the camera's
//! spectral data, and Helios has no crop module yet.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// One setting's value. Nested tables (tone curves, masks, look tables)
/// are only skipped over: nothing here reads them yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    Number(f64),
    Text(String),
    Bool(bool),
    Table,
}

impl Setting {
    pub fn number(&self) -> Option<f64> {
        match self {
            Setting::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Setting::Text(t) => Some(t),
            _ => None,
        }
    }

    /// As written in a warning: numbers signed as Lightroom shows them.
    pub fn show(&self) -> String {
        match self {
            Setting::Number(n) if *n > 0.0 => format!("+{n}"),
            Setting::Number(n) => format!("{n}"),
            Setting::Text(t) => format!("\"{t}\""),
            Setting::Bool(b) => b.to_string(),
            Setting::Table => "{…}".into(),
        }
    }

    fn json(&self) -> Value {
        match self {
            Setting::Number(n) => json!(n),
            Setting::Text(t) => json!(t),
            Setting::Bool(b) => json!(b),
            Setting::Table => Value::Null,
        }
    }
}

/// Settings by name.
pub type Settings = BTreeMap<String, Setting>;

/// A Lua table literal as Lightroom writes develop settings: an optional
/// `name =` before the table, keys bare names or `["…"]`, values numbers,
/// quoted or long (`[[…]]`) strings, `true`, `false`, `nil` or nested
/// tables. The top table's keyed entries come back; its positional ones,
/// and anything inside nested tables, are dropped.
pub fn parse_settings(text: &str) -> Result<Settings, String> {
    let mut p = Lua {
        s: text.as_bytes(),
        i: 0,
    };
    p.space();
    // `s = {`: an optional name before the table
    let start = p.i;
    if p.name().is_some() {
        p.space();
        if p.peek() == Some(b'=') {
            p.i += 1;
        } else {
            p.i = start;
        }
    }
    p.space();
    let mut out = Settings::new();
    p.table(Some(&mut out))?;
    p.space();
    if p.i < p.s.len() {
        return Err(format!("unexpected text after the table, at byte {}", p.i));
    }
    Ok(out)
}

struct Lua<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lua<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn error(&self, what: &str) -> String {
        format!("{what}, at byte {}", self.i)
    }

    /// Spaces and `--` comments.
    fn space(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn name(&mut self) -> Option<String> {
        let start = self.i;
        if !self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        {
            return None;
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.i += 1;
        }
        Some(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    /// A `{ … }`, its keyed entries put in `into` when given.
    fn table(&mut self, mut into: Option<&mut Settings>) -> Result<(), String> {
        if self.peek() != Some(b'{') {
            return Err(self.error("a table was expected"));
        }
        self.i += 1;
        loop {
            self.space();
            match self.peek() {
                None => return Err(self.error("the table is not closed")),
                Some(b'}') => {
                    self.i += 1;
                    return Ok(());
                }
                _ => {}
            }
            let key = self.key()?;
            let value = self.value()?;
            if let (Some(out), Some(key), Some(value)) = (into.as_deref_mut(), key, value) {
                out.insert(key, value);
            }
            self.space();
            match self.peek() {
                Some(b',' | b';') => self.i += 1,
                Some(b'}') => {}
                _ => return Err(self.error("`,` or `}` was expected")),
            }
        }
    }

    /// `name =` or `["name"] =` before a value, none for a positional one.
    fn key(&mut self) -> Result<Option<String>, String> {
        let start = self.i;
        if self.peek() == Some(b'[') && self.s.get(self.i + 1) == Some(&b'"') {
            self.i += 1;
            let key = self.quoted()?;
            self.space();
            if self.peek() != Some(b']') {
                return Err(self.error("`]` was expected"));
            }
            self.i += 1;
            self.space();
            if self.peek() != Some(b'=') {
                return Err(self.error("`=` was expected"));
            }
            self.i += 1;
            self.space();
            return Ok(Some(key));
        }
        if let Some(name) = self.name() {
            self.space();
            if self.peek() == Some(b'=') && self.s.get(self.i + 1) != Some(&b'=') {
                self.i += 1;
                self.space();
                return Ok(Some(name));
            }
        }
        self.i = start;
        Ok(None)
    }

    /// A value; none for `nil`.
    fn value(&mut self) -> Result<Option<Setting>, String> {
        match self.peek() {
            Some(b'{') => {
                self.table(None)?;
                Ok(Some(Setting::Table))
            }
            Some(b'"' | b'\'') => Ok(Some(Setting::Text(self.quoted()?))),
            Some(b'[') => Ok(Some(Setting::Text(self.long()?))),
            Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.peek().is_some_and(|c| {
                    c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'+'
                }) {
                    self.i += 1;
                }
                let raw = String::from_utf8_lossy(&self.s[start..self.i]);
                raw.parse::<f64>()
                    .map(|n| Some(Setting::Number(n)))
                    .map_err(|_| self.error(&format!("`{raw}` is not a number")))
            }
            _ => match self.name().as_deref() {
                Some("true") => Ok(Some(Setting::Bool(true))),
                Some("false") => Ok(Some(Setting::Bool(false))),
                Some("nil") => Ok(None),
                _ => Err(self.error("a value was expected")),
            },
        }
    }

    /// A `"…"` or `'…'` string, its escapes read.
    fn quoted(&mut self) -> Result<String, String> {
        let quote = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.error("the string is not closed")),
                Some(c) if c == quote => {
                    self.i += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or_else(|| self.error("a lone `\\`"))?;
                    out.push(match c {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        other => other,
                    });
                    self.i += 1;
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// A long string, `[[…]]` or `[==[…]==]`.
    fn long(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut level = 0;
        while self.peek() == Some(b'=') {
            level += 1;
            self.i += 1;
        }
        if self.peek() != Some(b'[') {
            return Err(self.error("a long string was expected"));
        }
        self.i += 1;
        let close = format!("]{}]", "=".repeat(level));
        let rest = &self.s[self.i..];
        let end = rest
            .windows(close.len())
            .position(|w| w == close.as_bytes())
            .ok_or_else(|| self.error("the long string is not closed"))?;
        let mut body = &rest[..end];
        // Lua drops a first newline
        if body.first() == Some(&b'\n') {
            body = &body[1..];
        }
        self.i += end + close.len();
        Ok(String::from_utf8_lossy(body).into_owned())
    }
}

/// The `crs:` settings of an `.xmp` sidecar, attributes
/// (`crs:Exposure2012="+0.35"`) or simple elements
/// (`<crs:Exposure2012>+0.35</crs:Exposure2012>`), and its
/// `tiff:Orientation` under that name. Numbers lose their `+`; `True` and
/// `False` are booleans; structures (`<crs:ToneCurvePV2012>` holding a
/// sequence) are tables.
pub fn parse_xmp(xml: &str) -> Settings {
    let mut out = Settings::new();
    for prefix in ["crs:", "tiff:"] {
        let mut rest = xml;
        while let Some(at) = rest.find(prefix) {
            let before = rest[..at].chars().last();
            rest = &rest[at + prefix.len()..];
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let name = &rest[..len];
            let after = &rest[len..];
            if name.is_empty() || (prefix == "tiff:" && name != "Orientation") {
                continue;
            }
            let value = if before == Some('<') {
                // an element: its text up to the closing tag, or a structure
                let Some(open_end) = after.find('>') else {
                    continue;
                };
                if after[..open_end].ends_with('/') {
                    continue;
                }
                let body = &after[open_end + 1..];
                let Some(close) = body.find(&format!("</{prefix}{name}>")) else {
                    continue;
                };
                let body = &body[..close];
                if body.contains('<') {
                    Setting::Table
                } else {
                    xmp_value(body)
                }
            } else if let Some(v) = after.trim_start().strip_prefix('=') {
                let v = v.trim_start();
                let Some(quote) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                    continue;
                };
                let Some(end) = v[1..].find(quote) else {
                    continue;
                };
                xmp_value(&v[1..=end])
            } else {
                continue;
            };
            let key = if prefix == "tiff:" {
                ORIENTATION_EXIF
            } else {
                name
            };
            out.entry(key.to_string()).or_insert(value);
        }
    }
    out
}

fn xmp_value(raw: &str) -> Setting {
    let text = raw
        .trim()
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    match text.as_str() {
        "True" | "true" => return Setting::Bool(true),
        "False" | "false" => return Setting::Bool(false),
        _ => {}
    }
    match text.strip_prefix('+').unwrap_or(&text).parse::<f64>() {
        Ok(n) => Setting::Number(n),
        Err(_) => Setting::Text(text),
    }
}

/// The name a sidecar's `tiff:Orientation` (an EXIF value) is kept under.
pub const ORIENTATION_EXIF: &str = "tiff:Orientation";

/// Basic-panel sliders Helios has no faithful equivalent for, neutral at 0.
const UNMATCHED_SLIDERS: &[&str] = &[
    "Contrast2012",
    "Highlights2012",
    "Shadows2012",
    "Whites2012",
    "Blacks2012",
    "Texture",
    "Clarity2012",
    "Dehaze",
    "Vibrance",
    "Saturation",
];

const CROP: &[&str] = &[
    "CropTop",
    "CropLeft",
    "CropBottom",
    "CropRight",
    "CropAngle",
];

/// darktable's orientation (`flip`'s `orientation`) for Lightroom's
/// two-letter one; none for a mirrored or unknown value. `flip`'s bits:
/// flip vertically 1, horizontally 2, swap x and y 4.
pub fn flip_from_letters(code: &str) -> Option<i64> {
    match code.trim() {
        "AB" => Some(0),
        "BC" => Some(5),
        "CD" => Some(3),
        "DA" => Some(6),
        _ => None,
    }
}

/// darktable's orientation for an EXIF orientation tag, as `flip`'s
/// `from_exif` (darktable's `dt_image_orientation_to_flip_bits`).
pub fn flip_from_exif(tag: f64) -> Option<i64> {
    match tag as i64 {
        1 => Some(0),
        2 => Some(2),
        3 => Some(3),
        4 => Some(1),
        5 => Some(4),
        6 => Some(5),
        7 => Some(7),
        8 => Some(6),
        _ => None,
    }
}

/// A photo's develop settings as Helios modules, and what was not mapped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Develop {
    /// `{"module": …, "params": {…}}`, in pipe order.
    pub modules: Vec<Value>,
    /// The settings read with no faithful Helios equivalent, by name, with
    /// their values.
    pub unmatched: Vec<(String, Setting)>,
}

impl Develop {
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty() && self.unmatched.is_empty()
    }

    /// As an image's `develop` key.
    pub fn json(&self, from: &str) -> Value {
        let unmatched: serde_json::Map<String, Value> = self
            .unmatched
            .iter()
            .map(|(k, v)| (k.clone(), v.json()))
            .collect();
        json!({ "from": from, "modules": self.modules, "unmatched": unmatched })
    }

    /// The unmatched settings, `Name = value` joined, for a warning.
    pub fn unmatched_text(&self) -> String {
        self.unmatched
            .iter()
            .map(|(k, v)| format!("{k} = {}", v.show()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn unmatched(d: &mut Develop, name: &str, value: Setting) {
    d.unmatched.push((name.to_string(), value));
}

/// Maps a photo's settings: `orientation` the catalogue's two letters
/// (else a sidecar's `tiff:Orientation` in `settings` is used), `raw`
/// whether the file is a raw one (white balance is set on raw data only).
pub fn map(settings: &Settings, orientation: Option<&str>, raw: bool) -> Develop {
    let mut d = Develop::default();

    // white balance, on the mosaic: first in the pipe
    let wb = settings.get("WhiteBalance");
    match wb.and_then(Setting::text) {
        Some("As Shot") if raw => d
            .modules
            .push(json!({ "module": "whitebalance", "params": { "mode": "as_shot" } })),
        Some("As Shot") => {
            // a non-raw file's shift from its as-shot colours
            for name in ["IncrementalTemperature", "IncrementalTint"] {
                if let Some(v) = settings.get(name).filter(|v| v.number() != Some(0.0)) {
                    unmatched(&mut d, name, v.clone());
                }
            }
        }
        _ if wb.is_some() => {
            unmatched(
                &mut d,
                "WhiteBalance",
                wb.cloned().unwrap_or(Setting::Table),
            );
            for name in ["Temperature", "Tint"] {
                if let Some(v) = settings.get(name) {
                    unmatched(&mut d, name, v.clone());
                }
            }
            for name in ["IncrementalTemperature", "IncrementalTint"] {
                if let Some(v) = settings.get(name).filter(|v| v.number() != Some(0.0)) {
                    unmatched(&mut d, name, v.clone());
                }
            }
        }
        _ => {}
    }

    if let Some(ev) = settings
        .get("Exposure2012")
        .and_then(Setting::number)
        .filter(|ev| *ev != 0.0)
    {
        d.modules
            .push(json!({ "module": "exposure", "params": { "exposure": ev } }));
    }

    for name in UNMATCHED_SLIDERS {
        if let Some(v) = settings.get(*name).filter(|v| v.number() != Some(0.0)) {
            unmatched(&mut d, name, v.clone());
        }
    }

    // the crop: `HasCrop`, else any edge off the frame or an angle
    let edges_off = CROP.iter().any(|n| {
        let neutral = if matches!(*n, "CropBottom" | "CropRight") {
            1.0
        } else {
            0.0
        };
        settings
            .get(*n)
            .and_then(Setting::number)
            .is_some_and(|v| v != neutral)
    });
    let has_crop = match settings.get("HasCrop") {
        Some(Setting::Bool(b)) => *b,
        _ => edges_off,
    };
    if has_crop {
        for name in CROP {
            if let Some(v) = settings.get(*name) {
                unmatched(&mut d, name, v.clone());
            }
        }
    }

    // the orientation, geometry: last
    let letters = orientation.map(str::trim).filter(|o| !o.is_empty());
    let flip = match letters {
        Some(code) => flip_from_letters(code)
            .ok_or_else(|| ("Orientation".to_string(), Setting::Text(code.to_string()))),
        None => match settings.get(ORIENTATION_EXIF) {
            Some(v) => v
                .number()
                .and_then(flip_from_exif)
                .ok_or_else(|| (ORIENTATION_EXIF.to_string(), v.clone())),
            None => Err((String::new(), Setting::Table)),
        },
    };
    match flip {
        Ok(o) => d
            .modules
            .push(json!({ "module": "flip", "params": { "orientation": o } })),
        Err((name, value)) if !name.is_empty() => unmatched(&mut d, &name, value),
        Err(_) => {}
    }
    d
}

/// Whether a file extension names a raw file: anything but the rendered
/// formats Lightroom imports.
pub fn is_raw(extension: &str) -> bool {
    !matches!(
        extension.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "psd" | "psb" | "heic" | "heif" | "avif" | "webp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A develop text as Lightroom Classic writes it (process version 2012
    /// and later), trimmed: keys sorted, nested tables for curves and
    /// masks, a long string, a comment.
    const SAMPLE: &str = r#"s = { AutoLateralCA = 0,
	Blacks2012 = -12,
	CameraProfile = "Adobe Standard",
	Clarity2012 = 15,
	Contrast2012 = 20,
	ConvertToGrayscale = false,
	CropAngle = 1.25,
	CropBottom = 0.95,
	CropLeft = 0.04,
	CropRight = 1,
	CropTop = 0.02,
	Dehaze = 0,
	Exposure2012 = 0.35,
	HasCrop = true,
	Highlights2012 = -40,
	IncrementalTemperature = 0,
	IncrementalTint = 0,
	MaskGroupBasedCorrections = { { CorrectionActive = true,
		CorrectionMasks = { { MaskID = "B5C1", What = "Mask/Image" } } } },
	ProcessVersion = "11.0",
	Saturation = 0,
	Shadows2012 = 25,
	Temperature = 5600,
	Texture = 0,
	Tint = 8,
	ToneCurveName2012 = "Linear",
	ToneCurvePV2012 = { 0, 0, 255, 255 },
	Vibrance = 10,
	WhiteBalance = "Custom",
	Whites2012 = 0,
	["Look"] = { Name = "Adobe Color", Parameters = { Version = 15 } },
	-- a comment
	Description = [[two
lines]],
}
"#;

    #[test]
    fn a_develop_text_is_read_top_level_keys_only() {
        let s = parse_settings(SAMPLE).unwrap();
        assert_eq!(s["Exposure2012"], Setting::Number(0.35));
        assert_eq!(s["Blacks2012"], Setting::Number(-12.0));
        assert_eq!(s["WhiteBalance"], Setting::Text("Custom".into()));
        assert_eq!(s["HasCrop"], Setting::Bool(true));
        assert_eq!(s["ToneCurvePV2012"], Setting::Table);
        assert_eq!(s["MaskGroupBasedCorrections"], Setting::Table);
        assert_eq!(s["Look"], Setting::Table);
        assert_eq!(s["Description"], Setting::Text("two\nlines".into()));
        // nested keys stay inside their table
        assert!(!s.contains_key("CorrectionActive"));
        assert!(!s.contains_key("MaskID"));
    }

    #[test]
    fn escapes_bare_tables_and_errors() {
        let s = parse_settings(r#"{ A = "say \"hi\"", B = -1e-3, C = nil, 4, }"#).unwrap();
        assert_eq!(s["A"], Setting::Text("say \"hi\"".into()));
        assert_eq!(s["B"], Setting::Number(-0.001));
        assert!(!s.contains_key("C"));
        assert_eq!(s.len(), 2);
        assert!(parse_settings("s = { A = 1").is_err());
        assert!(parse_settings("s = { A = 1 } x").is_err());
        assert!(parse_settings("s = { A = 1x2 }").is_err());
    }

    #[test]
    fn the_sample_maps_exposure_and_lists_the_rest_with_values() {
        let s = parse_settings(SAMPLE).unwrap();
        let d = map(&s, Some("BC"), true);
        assert_eq!(
            d.modules,
            vec![
                json!({ "module": "exposure", "params": { "exposure": 0.35 } }),
                json!({ "module": "flip", "params": { "orientation": 5 } }),
            ]
        );
        let names: Vec<&str> = d.unmatched.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "WhiteBalance",
                "Temperature",
                "Tint",
                "Contrast2012",
                "Highlights2012",
                "Shadows2012",
                "Blacks2012",
                "Clarity2012",
                "Vibrance",
                "CropTop",
                "CropLeft",
                "CropBottom",
                "CropRight",
                "CropAngle",
            ]
        );
        let text = d.unmatched_text();
        assert!(text.starts_with("WhiteBalance = \"Custom\", Temperature = +5600, Tint = +8"));
        assert!(text.contains("Highlights2012 = -40"));
        assert!(text.ends_with("CropAngle = +1.25"));
        let j = d.json("catalogue");
        assert_eq!(j["unmatched"]["Contrast2012"], json!(20.0));
        assert_eq!(j["from"], "catalogue");
    }

    #[test]
    fn as_shot_maps_on_raw_files_only_and_neutral_values_are_quiet() {
        let s = parse_settings(
            r#"s = { WhiteBalance = "As Shot", Exposure2012 = 0, Contrast2012 = 0,
                     HasCrop = false, CropTop = 0, CropBottom = 1,
                     IncrementalTemperature = 0, IncrementalTint = 0 }"#,
        )
        .unwrap();
        let raw = map(&s, Some("AB"), true);
        assert_eq!(
            raw.modules,
            vec![
                json!({ "module": "whitebalance", "params": { "mode": "as_shot" } }),
                json!({ "module": "flip", "params": { "orientation": 0 } }),
            ]
        );
        assert!(raw.unmatched.is_empty());
        let jpeg = map(&s, None, false);
        assert!(jpeg.is_empty());
        let mut shifted = s.clone();
        shifted.insert("IncrementalTint".into(), Setting::Number(-5.0));
        assert_eq!(
            map(&shifted, None, false).unmatched_text(),
            "IncrementalTint = -5"
        );
    }

    #[test]
    fn a_crop_without_has_crop_is_seen_from_its_edges() {
        let s = parse_settings("s = { CropLeft = 0.1, CropRight = 1, CropAngle = 0 }").unwrap();
        let names: Vec<String> = map(&s, None, true)
            .unmatched
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(names, ["CropLeft", "CropRight", "CropAngle"]);
    }

    #[test]
    fn orientations_map_to_darktable_bits_mirrors_are_unmatched() {
        assert_eq!(flip_from_letters("AB"), Some(0));
        assert_eq!(flip_from_letters("BC"), Some(5));
        assert_eq!(flip_from_letters("CD"), Some(3));
        assert_eq!(flip_from_letters("DA"), Some(6));
        assert_eq!(flip_from_letters("BA"), None);
        assert_eq!(flip_from_exif(6.0), Some(5));
        assert_eq!(flip_from_exif(8.0), Some(6));
        assert_eq!(flip_from_exif(0.0), None);
        let d = map(&Settings::new(), Some("BA"), true);
        assert_eq!(d.unmatched_text(), "Orientation = \"BA\"");
    }

    #[test]
    fn a_sidecar_gives_the_same_settings_and_its_orientation() {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   tiff:Orientation="8"
   crs:Version="15.0"
   crs:WhiteBalance="As Shot"
   crs:Exposure2012="+0.35"
   crs:Contrast2012="-10"
   crs:HasCrop="False"
   crs:CameraProfile="Adobe Standard &amp; more">
   <crs:Clarity2012>+15</crs:Clarity2012>
   <crs:ToneCurvePV2012>
    <rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = parse_xmp(xmp);
        assert_eq!(s["Exposure2012"], Setting::Number(0.35));
        assert_eq!(s["Contrast2012"], Setting::Number(-10.0));
        assert_eq!(s["HasCrop"], Setting::Bool(false));
        assert_eq!(s["Clarity2012"], Setting::Number(15.0));
        assert_eq!(s["ToneCurvePV2012"], Setting::Table);
        assert_eq!(
            s["CameraProfile"],
            Setting::Text("Adobe Standard & more".into())
        );
        assert_eq!(s[ORIENTATION_EXIF], Setting::Number(8.0));
        let d = map(&s, None, true);
        assert_eq!(
            d.modules,
            vec![
                json!({ "module": "whitebalance", "params": { "mode": "as_shot" } }),
                json!({ "module": "exposure", "params": { "exposure": 0.35 } }),
                json!({ "module": "flip", "params": { "orientation": 6 } }),
            ]
        );
        assert_eq!(d.unmatched_text(), "Contrast2012 = -10, Clarity2012 = +15");
    }
}
