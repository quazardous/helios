//! `photo.history`: any photo's undo history, read-only, for agents and
//! scripts (docs/design/undo.md, slice R6).
//!
//! The steps are those the photo's saved versions say
//! (`library.edit.history`, rebuilt from their step markers by
//! `helios_vault::edit_steps`), so a photo that is not open reads as
//! well as the one in the darkroom. One line, oldest first: the steps an
//! undo walks back, then those a redo makes again (`undone`). A long
//! line is read a page at a time (`from`, `limit`); without `from`, the
//! newest page. The edits each step reached are sent only when asked
//! (`edits`): they can be large.
//!
//! The open photo's steps not saved yet (a drag going on) are not in it:
//! its versions are written as each step ends.

use std::sync::Arc;

use serde_json::{Value, json};

use super::Shell;

/// The command.
pub const COMMAND: &str = "photo.history";

/// How many steps a page holds when `limit` is absent.
pub const PAGE: usize = 50;
/// The most steps one page holds.
pub const MAX_PAGE: usize = 500;

/// Reads a photo's history (`library.edit.history`'s answer), by its id.
pub type Reader = Arc<dyn Fn(&str) -> Result<Value, String> + Send + Sync>;

/// A [`Reader`] through the node behind `node`.
pub fn reader_of(node: Arc<helios_inspect::client::Client>) -> Reader {
    Arc::new(move |image| {
        node.request(
            helios_node_core::library::EDIT_HISTORY,
            json!({ "image": image }),
        )
        .map_err(|e| e.message)
    })
}

/// The page `args` asks of `answer` (`library.edit.history`'s).
pub fn page(answer: &Value, args: &Value) -> Result<Value, String> {
    let list = |key: &str| answer[key].as_array().cloned().unwrap_or_default();
    let (undo, redo) = (list("undo"), list("redo"));
    let total = undo.len() + redo.len();
    let limit = match args.get("limit").and_then(Value::as_u64) {
        Some(0) => return Err("`limit` is at least 1".into()),
        Some(n) => usize::try_from(n).unwrap_or(MAX_PAGE).min(MAX_PAGE),
        None => PAGE,
    };
    let from = match args.get("from").and_then(Value::as_u64) {
        Some(n) => usize::try_from(n).unwrap_or(usize::MAX).min(total),
        None => total.saturating_sub(limit),
    };
    let edits = args.get("edits").and_then(Value::as_bool).unwrap_or(false);
    let steps: Vec<Value> = undo
        .iter()
        .map(|s| (s, false))
        .chain(redo.iter().map(|s| (s, true)))
        .enumerate()
        .skip(from)
        .take(limit)
        .map(|(index, (s, undone))| {
            let mut row = json!({
                "index": index,
                "version": s["version"],
                "label": s["label"],
                "origin": s["origin"],
                "at": s["at"],
                "undone": undone,
            });
            if edits {
                row["edit"] = s["edit"].clone();
            }
            row
        })
        .collect();
    let end = from + steps.len();
    Ok(json!({
        "image": answer["image"],
        "steps": steps,
        "from": from,
        "total": total,
        "undone": redo.len(),
        "more": end < total,
    }))
}

impl Shell {
    /// Where `photo.history` reads the photos' versions.
    pub fn set_photo_history(&mut self, reader: Reader) {
        self.core.photo_history = Some(reader);
    }

    /// `photo.history`: a page of the photo `image`'s steps.
    pub fn photo_history(&self, args: &Value) -> Result<Value, String> {
        let image = args
            .get("image")
            .and_then(Value::as_str)
            .ok_or("`image` is missing: a photo's id")?;
        // An id only: a path would say where the photo is.
        helios_vault::ImageId::from_hex(image).map_err(|e| format!("`image`: {e}"))?;
        let reader = self
            .core
            .photo_history
            .clone()
            .ok_or("no library open: it keeps the photos' history")?;
        page(&reader(image)?, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(n: u32) -> Value {
        json!({ "version": format!("v{n}"), "edit": { "n": n }, "label": format!("step {n}"),
                "origin": "user", "at": f64::from(n) })
    }

    fn answer(done: u32, undone: u32) -> Value {
        json!({
            "image": "ab",
            "undo": (1..=done).map(step).collect::<Vec<_>>(),
            "redo": (done + 1..=done + undone).map(step).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn one_line_oldest_first_the_undone_steps_last_and_no_edit_unless_asked() {
        let p = page(&answer(2, 1), &json!({})).unwrap();
        let labels: Vec<_> = p["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["label"].as_str().unwrap().to_owned(), s["undone"] == true))
            .collect();
        assert_eq!(
            labels,
            [
                ("step 1".to_owned(), false),
                ("step 2".to_owned(), false),
                ("step 3".to_owned(), true)
            ]
        );
        assert_eq!(
            (p["total"].as_u64(), p["undone"].as_u64()),
            (Some(3), Some(1))
        );
        assert_eq!(p["more"], false);
        assert!(p["steps"][0].get("edit").is_none());
        let with = page(&answer(2, 1), &json!({ "edits": true })).unwrap();
        assert_eq!(with["steps"][2]["edit"], json!({ "n": 3 }));
    }

    #[test]
    fn a_long_line_reads_the_newest_page_first_then_any_page_by_from() {
        let a = answer(120, 0);
        let newest = page(&a, &json!({})).unwrap();
        assert_eq!(newest["from"], 70);
        assert_eq!(newest["steps"].as_array().unwrap().len(), PAGE);
        assert_eq!(newest["steps"][49]["index"], 119);
        assert_eq!(newest["more"], false);
        let first = page(&a, &json!({ "from": 0, "limit": 10 })).unwrap();
        assert_eq!(first["steps"][0]["label"], "step 1");
        assert_eq!(first["more"], true);
        let past = page(&a, &json!({ "from": 500 })).unwrap();
        assert!(past["steps"].as_array().unwrap().is_empty());
        assert!(page(&a, &json!({ "limit": 0 })).is_err());
        let capped = page(&answer(600, 0), &json!({ "limit": 10_000 })).unwrap();
        assert_eq!(capped["steps"].as_array().unwrap().len(), MAX_PAGE);
    }

    #[test]
    fn a_photo_never_edited_has_no_step() {
        let p = page(
            &json!({ "image": "ab", "undo": [], "redo": [] }),
            &json!({}),
        )
        .unwrap();
        assert_eq!(p["total"], 0);
        assert!(p["steps"].as_array().unwrap().is_empty());
    }
}
