//! A darktable edit handed over as a history rather than as a sidecar's
//! text: what an importer reads from darktable's `library.db` (the
//! `history`, `masks_history` and `module_order` rows of an image, its
//! `history_end`), in the JSON shape of the importer protocol, version 2
//! (see `docs/extensions.md`, "Importers").
//!
//! [`Sidecar::from_history`] turns it into the [`Sidecar`] the XMP path
//! reads, item by item through the same reader: params and blend params
//! are the bytes darktable stores, in the sidecar's encodings (hex, or
//! `gz` + base64 of zlib data), decoded by the same function, so an edit
//! read from the library and the same edit read from its `.xmp` map onto
//! the same Helios chain.
//!
//! ```json
//! {
//!   "end": 3,
//!   "iop_order_version": 5,
//!   "iop_order_list": "rawprepare,0,…",
//!   "version_name": "bw",
//!   "items": [{
//!     "num": 0, "operation": "exposure", "modversion": 6, "enabled": true,
//!     "params": "<hex>", "multi_name": "", "multi_priority": 0,
//!     "blendop_version": 14, "blendop_params": "<hex>"
//!   }],
//!   "masks": [{
//!     "num": 2, "id": 1700000000, "type": 2, "name": "circle #1",
//!     "version": 6, "count": 1, "points": "<hex>"
//!   }]
//! }
//! ```

use serde_json::Value;

use crate::sidecar::{Sidecar, item};

/// The sidecar format version a library edit counts as: darktable's
/// library records every module, so no implicit item is added.
const LIBRARY_XMP_VERSION: u32 = 5;

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "1" } else { "0" }.into()),
        _ => None,
    }
}

fn number(v: &Value, key: &str) -> Result<Option<u32>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("history `{key}`: {n} is not a count")),
    }
}

/// The attributes a sidecar gives a history item, from its JSON.
fn item_attributes(v: &Value) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for (key, attr) in [
        ("num", "darktable:num"),
        ("operation", "darktable:operation"),
        ("modversion", "darktable:modversion"),
        ("params", "darktable:params"),
        ("multi_name", "darktable:multi_name"),
        ("multi_priority", "darktable:multi_priority"),
        ("blendop_version", "darktable:blendop_version"),
        ("blendop_params", "darktable:blendop_params"),
    ] {
        if let Some(t) = v.get(key).and_then(text) {
            attrs.push((attr.to_string(), t));
        }
    }
    let enabled = match v.get("enabled") {
        Some(Value::Bool(b)) => *b,
        Some(n) => n.as_i64().unwrap_or(0) != 0,
        None => false,
    };
    attrs.push((
        "darktable:enabled".into(),
        if enabled { "1" } else { "0" }.into(),
    ));
    attrs
}

fn mask_attributes(v: &Value) -> Vec<(String, String)> {
    [
        ("num", "darktable:mask_num"),
        ("id", "darktable:mask_id"),
        ("type", "darktable:mask_type"),
        ("name", "darktable:mask_name"),
        ("version", "darktable:mask_version"),
        ("count", "darktable:mask_nb"),
        ("points", "darktable:mask_points"),
        ("source", "darktable:mask_src"),
    ]
    .into_iter()
    .filter_map(|(key, attr)| v.get(key).and_then(text).map(|t| (attr.to_string(), t)))
    .collect()
}

impl Sidecar {
    /// The sidecar a history stands for (see the module's doc). Only a
    /// value that is not a history fails; an item that cannot be read
    /// is kept, marked, as in a sidecar.
    pub fn from_history(history: &Value) -> Result<Sidecar, String> {
        let items = history
            .get("items")
            .and_then(Value::as_array)
            .ok_or("a history without `items`")?;
        let mut sidecar = Sidecar {
            history_end: number(history, "end")?,
            xmp_version: Some(LIBRARY_XMP_VERSION),
            iop_order_version: number(history, "iop_order_version")?,
            iop_order_list: history
                .get("iop_order_list")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from),
            version_name: history
                .get("version_name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from),
            ..Sidecar::default()
        };
        sidecar.history = items
            .iter()
            .enumerate()
            .map(|(i, v)| item(item_attributes(v), i))
            .collect();
        sidecar.history.sort_by_key(|i| i.num);
        let masks = history.get("masks").and_then(Value::as_array);
        let forms: Result<Vec<_>, String> = masks
            .into_iter()
            .flatten()
            .map(|m| crate::drawn::form_of(&mask_attributes(m)))
            .collect();
        match forms {
            Ok(forms) => sidecar.masks = forms,
            Err(e) => sidecar.masks_unreadable = Some(e),
        }
        sidecar.add_implicit_highlights();
        Ok(sidecar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_history_reads_as_the_sidecar_spelling_it() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description darktable:xmp_version="5" darktable:history_end="1" darktable:iop_order_version="5">
<darktable:history><rdf:Seq>
<rdf:li darktable:num="0" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="6"
 darktable:params="00000000000080bf" darktable:multi_name="" darktable:multi_priority="0"/>
</rdf:Seq></darktable:history></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let from_xml = Sidecar::parse(xml).unwrap();
        let from_history = Sidecar::from_history(&json!({
            "end": 1, "iop_order_version": 5,
            "items": [{"num": 0, "operation": "exposure", "enabled": true, "modversion": 6,
                       "params": "00000000000080bf", "multi_name": "", "multi_priority": 0}],
        }))
        .unwrap();
        assert_eq!(from_history.history[0].params, from_xml.history[0].params);
        assert_eq!(from_history.effective().len(), 1);
        assert_eq!(from_history.history_end, Some(1));
        assert!(from_history.history[0].unreadable.is_none());
        assert!(Sidecar::from_history(&json!({})).is_err());
    }
}
