//! The desktop's window commands through its node's command endpoint: the
//! CLI and agents list them beside the node's, each greyed with the reason
//! its menu entry gives in the window asked about, and run them in a
//! window under their own origin.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_gui::window_commands::Bridge;
use helios_gui::{Shell, headless, helios_panels};
use helios_inspect::client::Client;
use helios_inspect::rpc;
use helios_node_core::endpoint::VaultService;
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

/// A desktop: its shell drawn headless, its node's endpoint serving the
/// window's commands, a client calling it as an agent, the node's bus.
struct Desktop {
    h: Harness<'static, Shell>,
    client: Arc<Client>,
    bus: Arc<Bus>,
}

fn desktop() -> Desktop {
    let mut h = headless::harness(Shell::new(helios_panels(), None), egui::vec2(800.0, 500.0));
    h.run();
    let bridge = Bridge::new();
    let ctx = h.ctx.clone();
    bridge.attach(h.state_mut(), &ctx);
    let bus = Arc::new(Bus::new());
    let service = VaultService::new(
        Arc::new(Vault::new(MemoryStore::default())),
        bus.clone(),
        Jobs::new(bus.clone()),
        "desktop-test",
    )
    .with_windows(Arc::new(bridge));
    let client = Client::in_process(Arc::new(service)).with_origin("agent:test");
    Desktop {
        h,
        client: Arc::new(client),
        bus,
    }
}

impl Desktop {
    /// Calls `method` from another thread, as the endpoint's are, drawing
    /// frames until it answers.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, rpc::Error> {
        let client = self.client.clone();
        let method = method.to_string();
        let asked = std::thread::spawn(move || client.request(&method, params));
        let deadline = Instant::now() + Duration::from_secs(20);
        while !asked.is_finished() {
            assert!(Instant::now() < deadline, "the window never answered");
            self.h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        asked.join().unwrap()
    }

    fn list(&mut self, params: Value) -> Vec<Value> {
        let listed = self.call("commands.list", params).unwrap();
        assert!(listed["window_error"].is_null(), "{listed}");
        listed["commands"].as_array().unwrap().clone()
    }
}

fn find<'a>(commands: &'a [Value], id: &str) -> &'a Value {
    commands
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("{id} listed"))
}

#[test]
fn the_node_lists_the_window_commands_with_why_one_is_greyed() {
    let mut d = desktop();
    let commands = d.list(json!({}));
    // The node's own and the window's, in one list.
    assert_eq!(find(&commands, "library.stacks.list")["scope"], "node");
    let theme = find(&commands, "theme.select");
    assert_eq!(theme["scope"], "window");
    assert_eq!(theme["window"], "main");
    assert_eq!(theme["enabled"], true);
    assert!(theme["disabledReason"].is_null(), "{theme}");
    assert_eq!(theme["params"]["type"], "object");
    // Nothing to undo yet: Undo is greyed, saying why.
    let undo = find(&commands, "edit.undo");
    assert_eq!(undo["enabled"], false);
    let reason = undo["disabledReason"].as_str().unwrap();
    assert!(reason.starts_with("Does not apply here"), "{reason}");
    // Export is the main window's: in the import window it is greyed,
    // saying where it runs.
    assert_eq!(find(&commands, "library.export")["enabled"], true);
    let import = d.list(json!({ "window": "import", "filter": "library" }));
    let export = find(&import, "library.export");
    assert_eq!(export["window"], "import");
    assert_eq!(export["enabled"], false);
    let said = export["disabledReason"].as_str().unwrap();
    assert!(
        said.starts_with("Only in the ") && said.ends_with(" window"),
        "{said}"
    );
    let enabled = d.list(json!({ "window": "import", "enabled_only": true }));
    assert!(!enabled.iter().any(|c| c["id"] == "library.export"));
}

#[test]
fn a_greyed_command_is_refused_with_its_reason_and_an_enabled_one_runs_as_the_agent() {
    let mut d = desktop();
    let heard = d.bus.subscribe();
    let refused = d
        .call("command.run", json!({ "id": "edit.undo" }))
        .unwrap_err();
    assert_eq!(refused.code, rpc::code::REFUSED);
    assert!(
        refused.message.starts_with("Does not apply here"),
        "{refused:?}"
    );
    let ran = d
        .call(
            "command.run",
            json!({ "id": "theme.select", "params": { "name": "helios-light" } }),
        )
        .unwrap();
    assert_eq!(ran["origin"], "agent:test");
    assert_eq!(ran["window"], "main");
    d.h.run();
    assert_eq!(d.h.state().theme_name(), "helios-light");
    // The node's bus heard the run, by the agent; the window's history
    // keeps it as the agent's.
    let events: Vec<_> = std::iter::from_fn(|| heard.try_recv().ok()).collect();
    assert!(
        events.iter().any(|e| e.kind == "control.command"
            && e.payload["id"] == "theme.select"
            && e.payload["origin"] == "agent:test"),
        "{events:?}"
    );
    assert!(
        d.h.state()
            .history()
            .iter()
            .any(|r| r.id == "theme.select" && r.origin == Origin::Agent("test".into())),
    );
    // A window that is not open runs nothing.
    let closed = d
        .call(
            "command.run",
            json!({ "id": "theme.select", "window": "import",
                    "params": { "name": "helios-dark" } }),
        )
        .unwrap_err();
    assert!(closed.message.contains("no `import` window"), "{closed:?}");
}
