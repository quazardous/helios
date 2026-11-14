//! An agent's `command.batch` as one step of the library's history: the
//! library changes of the window commands it runs are one step, "Agent: 3
//! commands", by the agent; one undo takes them all back, one redo puts
//! them all again. A step failing with `stop_on_error` leaves one step
//! for what ran; a batch that changed nothing undoable leaves none; the
//! user's own action between two batches stays a step of its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::api::history::{Answer, Request};
use egui_pop::model::commands::Origin as PopOrigin;
use helios_core::jobs::Jobs;
use helios_core::{Bus, ContentHash};
use helios_gui::library_history::LIBRARY;
use helios_gui::persist::Persist;
use helios_gui::window_commands::Bridge;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_inspect::client::Client;
use helios_node_core::endpoint::VaultService;
use helios_shell::commands::Origin;
use helios_vault::rotation::{ROTATION, quarter_turns};
use helios_vault::{Image, ImageId, Location, MemoryStore, Store, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-batch-undo-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn id(n: u8) -> ImageId {
    ImageId(ContentHash([n; 32]))
}

/// A vault of three photos, upright.
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in 1..=3u8 {
        store
            .add_to_library(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
        store
            .put_location(
                &id(n),
                &Location::seen(dir.join(format!("{n}.jpg")), 10, 1, 1),
            )
            .unwrap();
    }
    path
}

/// A desktop: its window in the Library over the vault, its library's
/// history undoing through the vault, and an agent calling its node's
/// endpoint.
struct Desktop {
    h: Harness<'static, Shell>,
    persist: Persist<SqliteStore>,
    agent: Arc<Client>,
    at: f64,
}

fn desktop(root: &Path) -> Desktop {
    let path = vault_file(root);
    let config = root.join("config");
    let probe = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(&probe.template_panels(), probe.specs());
    let created = templates::create_workspace(
        &templates::find("helios.import-cull-dedupe").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run("view.library", Value::Null, Origin::User)
        .unwrap();
    h.run();
    h.run();
    let vault = persist.vault().clone();
    h.state()
        .library_history()
        .set_apply(Arc::new(move |changes, forward| {
            if forward {
                vault.replay(changes)
            } else {
                vault.revert(changes)
            }
        }));
    let bridge = Bridge::new();
    let ctx = h.ctx.clone();
    bridge.attach(h.state_mut(), &ctx);
    let bus = Arc::new(Bus::new());
    let service = VaultService::new(
        Arc::new(Vault::new(MemoryStore::default())),
        bus.clone(),
        Jobs::new(bus),
        "desktop-test",
    )
    .with_windows(Arc::new(bridge));
    let agent = Arc::new(Client::in_process(Arc::new(service)).with_origin("agent:mcp"));
    let mut d = Desktop {
        h,
        persist,
        agent,
        at: 0.0,
    };
    d.settle();
    d
}

impl Desktop {
    /// A few frames, the host's turn between them (as the desktop's loop).
    fn settle(&mut self) {
        for _ in 0..3 {
            self.at += 1.0;
            self.h.run();
            self.persist.frame_settled(self.h.state_mut(), self.at);
        }
        self.h.run();
    }

    /// The agent's batch of `steps`, its window commands run at the
    /// window's frames; the answer once the writes are recorded.
    fn batch(&mut self, steps: Value, stop_on_error: bool) -> Value {
        let agent = self.agent.clone();
        let params = json!({ "steps": steps, "stop_on_error": stop_on_error });
        let asked = std::thread::spawn(move || agent.request("command.batch", params));
        let deadline = Instant::now() + Duration::from_secs(30);
        while !asked.is_finished() {
            assert!(Instant::now() < deadline, "the window never answered");
            self.at += 0.1;
            self.h.step();
            self.persist.frame(self.h.state_mut(), self.at);
            std::thread::sleep(Duration::from_millis(5));
        }
        let answer = asked.join().unwrap().unwrap();
        self.settle();
        answer
    }

    fn turns(&self, n: u8) -> u8 {
        let rows = self
            .persist
            .vault()
            .fields_of(&[id(n)], &[ROTATION])
            .unwrap();
        quarter_turns(rows[0].get(ROTATION))
    }

    fn all_turns(&self) -> (u8, u8, u8) {
        (self.turns(1), self.turns(2), self.turns(3))
    }

    /// The library's steps, oldest first: label and origin.
    fn steps(&self) -> Vec<(String, PopOrigin)> {
        self.h
            .state()
            .library_history()
            .line()
            .undo
            .iter()
            .map(|s| (s.label.text.clone(), s.origin.clone()))
            .collect()
    }

    fn undo(&mut self, origin: &PopOrigin) {
        let undone = self
            .h
            .state()
            .library_history()
            .undo(&Request::new(LIBRARY), origin);
        assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
        self.settle();
    }

    fn redo(&mut self, origin: &PopOrigin) {
        let redone = self
            .h
            .state()
            .library_history()
            .redo(&Request::new(LIBRARY), origin);
        assert!(matches!(redone, Answer::Done(Ok(_))), "{redone:?}");
        self.settle();
    }
}

fn step(command: &str, n: u8) -> Value {
    json!({ "id": command, "params": { "images": [id(n).hex()] } })
}

fn agent() -> PopOrigin {
    PopOrigin::Agent("mcp".into())
}

#[test]
fn an_agent_batch_is_one_step_undone_and_redone_whole() {
    let root = temp("whole");
    let mut d = desktop(&root);
    assert!(d.steps().is_empty());

    // Three undoable library commands in one batch: one step, the agent's.
    let answer = d.batch(
        json!([
            step("library.rotateLeft", 1),
            step("library.rotateRight", 2),
            step("library.rotateRight", 3),
        ]),
        true,
    );
    assert_eq!(answer["ran"], 3, "{answer}");
    assert_eq!(d.all_turns(), (3, 1, 1));
    assert_eq!(d.steps(), [("Agent: 3 commands".to_string(), agent())]);

    // One undo takes the three back, one redo puts them again.
    d.undo(&agent());
    assert_eq!(d.all_turns(), (0, 0, 0));
    assert!(d.steps().is_empty());
    d.redo(&PopOrigin::User);
    assert_eq!(d.all_turns(), (3, 1, 1));
    assert_eq!(d.steps().len(), 1);

    // A batch that changes nothing undoable leaves no step.
    let answer = d.batch(
        json!([{ "id": "theme.select", "params": { "name": "helios-light" } }]),
        true,
    );
    assert_eq!(answer["ran"], 1, "{answer}");
    assert_eq!(d.steps().len(), 1);
    drop(d);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_step_leaves_one_step_for_what_ran_and_the_user_s_action_stays_apart() {
    let root = temp("failed");
    let mut d = desktop(&root);
    d.batch(
        json!([
            step("library.rotateRight", 1),
            step("library.rotateRight", 2)
        ]),
        true,
    );

    // The user turns a photo between two batches: a step of its own.
    d.h.state_mut()
        .run(
            "library.rotateLeft",
            json!({ "images": [id(3).hex()] }),
            Origin::User,
        )
        .unwrap();
    d.settle();
    assert_eq!(d.all_turns(), (1, 1, 3));

    // The middle step fails: the batch stops, its step holds what ran.
    let answer = d.batch(
        json!([
            step("library.rotateRight", 1),
            { "id": "library.rotateLeft", "params": { "images": ["nope"] } },
            step("library.rotateRight", 2),
        ]),
        true,
    );
    assert_eq!(
        (
            answer["ran"].clone(),
            answer["failed"].clone(),
            answer["skipped"].clone()
        ),
        (json!(1), json!(1), json!(1)),
        "{answer}"
    );
    assert_eq!(d.all_turns(), (2, 1, 3));
    assert_eq!(
        d.steps(),
        [
            ("Agent: 2 commands".to_string(), agent()),
            ("Rotate 1 photos left".to_string(), PopOrigin::User),
            ("Agent: 1 commands".to_string(), agent()),
        ]
    );

    // Its undo takes back only what ran; the agent does not walk the
    // user's step.
    d.undo(&agent());
    assert_eq!(d.all_turns(), (1, 1, 3));
    let refused =
        d.h.state()
            .library_history()
            .undo(&Request::new(LIBRARY), &agent());
    assert!(matches!(refused, Answer::Done(Err(_))), "{refused:?}");
    d.undo(&PopOrigin::User);
    d.undo(&PopOrigin::User);
    assert_eq!(d.all_turns(), (0, 0, 0));
    drop(d);
    let _ = std::fs::remove_dir_all(&root);
}
