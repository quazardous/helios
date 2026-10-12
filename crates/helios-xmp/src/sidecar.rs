//! Reading the XML of a darktable sidecar.

use std::io::Read;

use base64::Engine as _;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::Serialize;

/// One entry of the history stack.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryItem {
    pub num: u32,
    pub operation: String,
    pub enabled: bool,
    /// The version of the module's params layout.
    pub modversion: u32,
    /// The params struct as darktable wrote it, decompressed.
    #[serde(skip)]
    pub params: Vec<u8>,
    pub multi_name: String,
    pub multi_priority: u32,
    /// The item's place in the pipe, in sidecars older than `xmp_version` 4.
    pub iop_order: Option<f64>,
    /// The version of the blend params' layout, when the item has some.
    pub blendop_version: Option<u32>,
    /// The blend params as darktable wrote them, decompressed.
    #[serde(skip)]
    pub blendop_params: Vec<u8>,
    /// Every attribute of the item as the sidecar spells it, params in
    /// their own encoding: what writes the item back unchanged.
    #[serde(skip)]
    pub attributes: Vec<(String, String)>,
    /// Why the item could not be read in full. It stays in the history,
    /// in its place, and is never applied.
    pub unreadable: Option<String>,
}

impl HistoryItem {
    /// An attribute as the sidecar spells it.
    pub fn attribute(&self, key: &str) -> Option<&str> {
        get(&self.attributes, key)
    }

    /// The instance's name as people gave it: empty for the names
    /// darktable gives the instances its automatic presets add
    /// (`_builtin_auto`, `_builtin_scene-referred default`...).
    pub fn instance_name(&self) -> &str {
        if self.multi_name.starts_with("_builtin_") {
            ""
        } else {
            &self.multi_name
        }
    }

    /// The item as people name it: its operation, its instance's name or
    /// number when it is not the first, and its params version.
    pub fn label(&self) -> String {
        let instance = if !self.instance_name().is_empty() {
            format!(" {}", self.instance_name())
        } else if self.multi_priority > 0 {
            format!(" {}", self.multi_priority)
        } else {
            String::new()
        };
        format!("{}{instance} v{}", self.operation, self.modversion)
    }
}

/// What a sidecar says about an image and its edit.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sidecar {
    /// The file the edit is for.
    pub derived_from: Option<String>,
    pub rating: Option<i64>,
    /// How many history items are applied (the rest were undone).
    pub history_end: Option<u32>,
    /// The version of darktable's sidecar format.
    pub xmp_version: Option<u32>,
    /// darktable's built-in pipe order, by number (see [`crate::iop_order`]).
    pub iop_order_version: Option<u32>,
    /// A pipe order spelled out: operation and instance pairs.
    pub iop_order_list: Option<String>,
    /// The name darktable shows for this version of the image, if any.
    pub version_name: Option<String>,
    pub history: Vec<HistoryItem>,
}

/// darktable's params encoding: hex, or `gz` + two digits + base64 of zlib data.
pub fn decode_params(text: &str) -> Result<Vec<u8>, String> {
    if let Some(rest) = text.strip_prefix("gz") {
        let data = rest.get(2..).ok_or("`gz` params without data")?;
        let packed = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("params: {e}"))?;
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&packed[..])
            .read_to_end(&mut out)
            .map_err(|e| format!("params: {e}"))?;
        Ok(out)
    } else {
        if !text.len().is_multiple_of(2) {
            return Err(format!("params: odd hex length {}", text.len()));
        }
        (0..text.len())
            .step_by(2)
            .map(|i| {
                text.get(i..i + 2)
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                    .ok_or_else(|| format!("params: `{}` is not hex", &text[i..]))
            })
            .collect()
    }
}

fn attributes(e: &BytesStart) -> Result<Vec<(String, String)>, String> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(|e| e.to_string())?;
            let key = a.key.as_ref().to_string();
            let value = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| e.to_string())?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn get<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn number<T: std::str::FromStr>(
    attrs: &[(String, String)],
    key: &str,
) -> Result<Option<T>, String> {
    get(attrs, key)
        .map(|v| {
            v.parse()
                .map_err(|_| format!("`{key}`: `{v}` is not a number"))
        })
        .transpose()
}

/// Reads a history item. An item darktable could not have written (an
/// attribute missing or not a number, params that do not decode) is kept
/// with what could be read and the reason: dropping it would shift
/// `history_end` onto other items.
fn item(attrs: Vec<(String, String)>, position: usize) -> HistoryItem {
    let mut problems: Vec<String> = Vec::new();
    let mut read = |key: &str| -> Option<u32> {
        number(&attrs, key).unwrap_or_else(|e| {
            problems.push(e);
            None
        })
    };
    let num = read("darktable:num");
    let modversion = read("darktable:modversion");
    let multi_priority = read("darktable:multi_priority");
    let blendop_version = read("darktable:blendop_version");
    let iop_order = number(&attrs, "darktable:iop_order").unwrap_or_else(|e| {
        problems.push(e);
        None
    });
    let operation = get(&attrs, "darktable:operation").unwrap_or("").to_string();
    if operation.is_empty() {
        problems.push("no `darktable:operation`".into());
    }
    if get(&attrs, "darktable:modversion").is_none() {
        problems.push("no `darktable:modversion`".into());
    }
    let params = get(&attrs, "darktable:params")
        .ok_or_else(|| "no `darktable:params`".to_string())
        .and_then(decode_params)
        .unwrap_or_else(|e| {
            problems.push(e);
            Vec::new()
        });
    let blendop_params = get(&attrs, "darktable:blendop_params")
        .map(|text| {
            decode_params(text).unwrap_or_else(|e| {
                problems.push(format!("blend {e}"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    HistoryItem {
        // Without a number, the item keeps its place in the document.
        num: num.unwrap_or_else(|| u32::try_from(position).unwrap_or(u32::MAX)),
        enabled: get(&attrs, "darktable:enabled") == Some("1"),
        modversion: modversion.unwrap_or(0),
        params,
        multi_name: get(&attrs, "darktable:multi_name")
            .unwrap_or("")
            .to_string(),
        multi_priority: multi_priority.unwrap_or(0),
        iop_order,
        blendop_version,
        blendop_params,
        operation,
        unreadable: (!problems.is_empty()).then(|| problems.join("; ")),
        attributes: attrs,
    }
}

impl Sidecar {
    /// Reads a sidecar's text. Only XML that cannot be read fails: a
    /// history item that cannot be read is kept, marked (see
    /// [`HistoryItem::unreadable`]).
    pub fn parse(xml: &str) -> Result<Sidecar, String> {
        let mut reader = Reader::from_str(xml);
        let mut sidecar = Sidecar::default();
        let mut in_history = false;
        loop {
            let event = reader.read_event().map_err(|e| format!("XMP: {e}"))?;
            match event {
                Event::Start(e) | Event::Empty(e) if e.name().as_ref() == "rdf:Description" => {
                    let attrs = attributes(&e)?;
                    sidecar.derived_from = get(&attrs, "xmpMM:DerivedFrom").map(str::to_string);
                    sidecar.rating = number(&attrs, "xmp:Rating")?;
                    sidecar.history_end = number(&attrs, "darktable:history_end")?;
                    sidecar.xmp_version = number(&attrs, "darktable:xmp_version")?;
                    sidecar.iop_order_version = number(&attrs, "darktable:iop_order_version")?;
                    sidecar.iop_order_list =
                        get(&attrs, "darktable:iop_order_list").map(str::to_string);
                    sidecar.version_name =
                        get(&attrs, "darktable:version_name").map(str::to_string);
                }
                Event::Start(e) if e.name().as_ref() == "darktable:history" => in_history = true,
                Event::End(e) if e.name().as_ref() == "darktable:history" => in_history = false,
                Event::Start(e) | Event::Empty(e)
                    if in_history && e.name().as_ref() == "rdf:li" =>
                {
                    let position = sidecar.history.len();
                    sidecar.history.push(item(attributes(&e)?, position));
                }
                Event::Eof => break,
                _ => {}
            }
        }
        // A stable sort: items with the same number keep the document's order.
        sidecar.history.sort_by_key(|i| i.num);
        Ok(sidecar)
    }

    /// The items in force: those before `history_end`, the last one of each
    /// module instance (operation and instance priority), in history order.
    /// darktable runs them in pipe order: see [`Sidecar::in_pipe_order`].
    pub fn effective(&self) -> Vec<&HistoryItem> {
        let end = self.history_end.unwrap_or(u32::MAX) as usize;
        let applied: Vec<&HistoryItem> = self.history.iter().take(end).collect();
        let mut out: Vec<&HistoryItem> = Vec::new();
        for item in applied {
            out.retain(|i| {
                !(i.operation == item.operation && i.multi_priority == item.multi_priority)
            });
            out.push(item);
        }
        out
    }

    /// The items in force, in the order darktable runs them.
    pub fn in_pipe_order(&self) -> crate::iop_order::Arranged<'_> {
        crate::iop_order::arrange(self)
    }
}
