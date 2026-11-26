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

/// `photo.history` through the agent socket, as an agent reads it: the
/// whole shell, a library of its own in memory reached through its node,
/// the scenario runner paired at start (the harness of
/// `agent_preset`'s socket tests, frames stepped under a deadline).
#[cfg(test)]
#[cfg(unix)]
mod socket {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::Arc;

    use egui_pop_agent::{AgentGrantsExt as _, AgentSocket, AgentSocketExt as _, PreIssueExt as _};
    use helios_core::Bus;
    use helios_vault::edit_steps::StepKind;
    use helios_vault::{EditSession, MemoryStore};
    use serde_json::{Value, json};

    use crate::agent_preset::Preset;
    use crate::agent_preset::tests::socket::{Client, finish};
    use crate::agent_preset::tests::{TOKEN, env};
    use crate::shell::{Shell, headless};
    use egui_pop_agent::TOKEN_ENV;

    /// A photo with three steps saved, the last one undone: its id, and
    /// the node's library holding it.
    fn photo() -> (String, helios_node_core::desktop::Library<MemoryStore>) {
        let library = helios_node_core::desktop::Library::on_store(
            MemoryStore::default(),
            Arc::new(Bus::new()),
        );
        let vault = library.vault();
        let dir = std::env::temp_dir().join(format!("helios-photo-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.raw");
        std::fs::write(&path, b"a photo with a history").unwrap();
        let image = vault.import_file(&path, 1).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let (mut session, _) = EditSession::open(vault, image).unwrap();
        for (n, label) in [(1, "exposure"), (2, "contrast"), (3, "crop")] {
            session
                .save(vault, json!({ "n": n }), label, "user", n)
                .unwrap()
                .expect("a step saved");
        }
        // Undone: back to the second step's edit, the crop left to redo.
        session.mark(StepKind::Undo);
        session
            .save(vault, json!({ "n": 2 }), "Undo", "user", 4)
            .unwrap()
            .expect("the undo saved");
        (image.hex(), library)
    }

    #[test]
    fn an_agent_reads_a_photo_s_steps_a_page_at_a_time_and_a_wrong_page_is_refused() {
        let (image, library) = photo();
        let dir =
            std::env::temp_dir().join(format!("helios-photo-history-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("helios.sock");
        let preset = Preset::Scenario { private: false };
        let mut builder = Shell::builder(crate::helios_panels(), None);
        builder.app = builder
            .app
            .debug_bridge(egui_pop::core::debug::BridgeMode::On)
            .agent_grants(preset.grants())
            .app_id("org.example.HeliosPhotoHistory")
            .agent_socket(AgentSocket::at(&socket))
            .pre_issued_pairing(preset.pre_issue(env(&[(TOKEN_ENV, TOKEN)])).unwrap());
        let mut shell = builder.build();
        let node = Arc::new(helios_inspect::client::Client::in_process(Arc::new(
            library.service("photo-history-test"),
        )));
        shell.set_photo_history(super::reader_of(node));
        let mut h = headless::harness(shell, egui::vec2(1024.0, 700.0));
        h.step();

        let s = socket.clone();
        let id = image.clone();
        let client = std::thread::spawn(move || {
            let mut c = Client::connect(&s);
            (
                history(&mut c, json!({ "image": id })),
                history(
                    &mut c,
                    json!({ "image": id, "from": 1, "limit": 1, "edits": true }),
                ),
                history(&mut c, json!({ "image": id, "limit": 0 })),
            )
        });
        let ((all, page), (one, one_page), (wrong, _)) = finish(&mut h, client);

        // The whole line, oldest first, the crop undone.
        assert_eq!(all["result"]["ok"], true, "{all}");
        let steps: Vec<(u64, &str, bool)> = page["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("steps: {all}"))
            .iter()
            .map(|s| {
                (
                    s["index"].as_u64().unwrap(),
                    s["label"].as_str().unwrap(),
                    s["undone"] == true,
                )
            })
            .collect();
        assert_eq!(
            steps,
            [
                (0, "exposure", false),
                (1, "contrast", false),
                (2, "crop", true)
            ],
            "{page}"
        );
        assert_eq!(
            (
                &page["image"],
                &page["from"],
                &page["total"],
                &page["undone"],
                &page["more"]
            ),
            (
                &json!(image),
                &json!(0),
                &json!(3),
                &json!(1),
                &json!(false)
            )
        );
        assert!(
            page["steps"][0].get("edit").is_none(),
            "no edit unless asked: {page}"
        );

        // One step from the second, with the edit it reached.
        assert_eq!(one["result"]["ok"], true, "{one}");
        let page = one_page;
        assert_eq!(page["steps"].as_array().map(Vec::len), Some(1), "{page}");
        assert_eq!(page["steps"][0]["label"], "contrast");
        assert_eq!(page["steps"][0]["edit"], json!({ "n": 2 }));
        assert_eq!((&page["from"], &page["more"]), (&json!(1), &json!(true)));

        // A page of no step is refused by the arguments' schema.
        assert_eq!(wrong["result"]["ok"], false, "{wrong}");
        let refused: Vec<Value> = h
            .state()
            .app()
            .handle()
            .events_since(0)
            .into_iter()
            .filter(|e| e.name == "command.refused" && e.data["by"] == "pop.command.args")
            .map(|e| e.data)
            .collect();
        assert!(
            refused.iter().any(|r| r["command"] == super::COMMAND),
            "{refused:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `commands.run` of `photo.history`, then the page its
    /// `command.done` carries, as `egui-pop-agent-mcp` reads a queued
    /// command: the run's response, and the page (null when the run was
    /// refused).
    fn history(c: &mut Client, args: Value) -> (Value, Value) {
        // The newest event now: the command's result comes after it.
        let mark = c.request(
            "wait",
            json!({ "event": "helios.test.mark", "deadline_ms": 0 }),
        );
        let mut since = mark["result"]["last"]
            .as_u64()
            .unwrap_or_else(|| panic!("the journal's last event: {mark}"));
        let run = c.run(super::COMMAND, args);
        if run["result"]["ok"] != true {
            return (run, Value::Null);
        }
        loop {
            let w = c.request(
                "wait",
                json!({
                    "any": [{ "event": "command.done" }, { "event": "command.failed" }],
                    "since": since,
                    "deadline_ms": 15_000,
                }),
            );
            let w = &w["result"];
            assert_eq!(w["met"], true, "`photo.history` queued, never done: {w}");
            let event = &w["event"];
            if event["data"]["command"] == super::COMMAND {
                assert_eq!(event["name"], "command.done", "{event}");
                return (run, event["data"]["result"].clone());
            }
            // Another command's: wait on, after it.
            since = event["seq"].as_u64().expect("an event's seq");
        }
    }
}
