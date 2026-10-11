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
}

/// What a sidecar says about an image and its edit.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sidecar {
    /// The file the edit is for.
    pub derived_from: Option<String>,
    pub rating: Option<i64>,
    /// How many history items are applied (the rest were undone).
    pub history_end: Option<u32>,
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
                u8::from_str_radix(&text[i..i + 2], 16)
                    .map_err(|_| format!("params: `{}` is not hex", &text[i..i + 2]))
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

fn item(attrs: &[(String, String)]) -> Result<HistoryItem, String> {
    let required =
        |key: &str| get(attrs, key).ok_or_else(|| format!("history item without `{key}`"));
    let operation = required("darktable:operation")?.to_string();
    let named = |e: String| format!("history item `{operation}`: {e}");
    Ok(HistoryItem {
        num: number(attrs, "darktable:num").map_err(named)?.unwrap_or(0),
        enabled: get(attrs, "darktable:enabled") == Some("1"),
        modversion: number(attrs, "darktable:modversion")
            .map_err(named)?
            .ok_or_else(|| named("no `darktable:modversion`".into()))?,
        params: decode_params(required("darktable:params")?).map_err(named)?,
        multi_name: get(attrs, "darktable:multi_name").unwrap_or("").to_string(),
        multi_priority: number(attrs, "darktable:multi_priority")
            .map_err(named)?
            .unwrap_or(0),
        operation,
    })
}

impl Sidecar {
    /// Reads a sidecar's text.
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
                }
                Event::Start(e) if e.name().as_ref() == "darktable:history" => in_history = true,
                Event::End(e) if e.name().as_ref() == "darktable:history" => in_history = false,
                Event::Start(e) | Event::Empty(e)
                    if in_history && e.name().as_ref() == "rdf:li" =>
                {
                    sidecar.history.push(item(&attributes(&e)?)?);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        sidecar.history.sort_by_key(|i| i.num);
        Ok(sidecar)
    }

    /// The items in force: those before `history_end`, the last one of each
    /// module instance (operation and instance priority), in history order.
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
}
