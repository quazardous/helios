//! Import accounts: "Add a source…" in the import window's Sources
//! column offers the connectors that declare a connection process, runs
//! their steps in the window's dialog (here a fake connector's form and
//! test), and the account appears in the column with its state;
//! Settings > Accounts lists it with Reconnect and Sign out. An agent may
//! fill a form, never a secret.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_connect::account::{
    AccountBook, AccountState, MemoryAccounts, MemorySecrets, Secrets as _,
};
use helios_connect::fake::{FakeProvider, ID};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&mut Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}");
}

/// Whether a node with this label is shown, once or more.
fn shown(h: &Harness<'_, Shell>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

fn run(h: &mut Harness<'_, Shell>, id: &str, args: Value, origin: Origin) -> Result<Value, String> {
    let r = h.state_mut().run(id, args, origin);
    h.run_steps(2);
    r
}

fn connection(h: &mut Harness<'_, Shell>) -> Value {
    h.state_mut()
        .run("import.connection", Value::Null, Origin::User)
        .unwrap()
}

struct World {
    book: Arc<MemoryAccounts>,
    secrets: Arc<MemorySecrets>,
}

fn world() -> (World, Harness<'static, Shell>) {
    let book = Arc::new(MemoryAccounts::default());
    let secrets = Arc::new(MemorySecrets::default());
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(FakeProvider));
    shell.set_import_accounts(book.clone(), secrets.clone());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    (World { book, secrets }, h)
}

#[test]
fn add_a_source_runs_the_connectors_form_and_test_and_lists_the_account_connected() {
    let (world, mut h) = world();
    run(&mut h, "library.importWindow", Value::Null, Origin::User).unwrap();
    until(&mut h, "Add a source…", |h| shown(h, "Add a source…"));
    h.get_by_label("Add a source…").click();
    until(&mut h, "the connectors offered", |h| {
        shown(h, "Test server")
    });
    h.get_by_label("Test server").click();
    until(&mut h, "the form", |h| connection(h)["state"] == "form");
    let form = connection(&mut h);
    let fields: Vec<&str> = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["host", "user", "password"], "{form}");
    assert_eq!(form["fields"][2]["secret"], true);
    // The dialog shows the form's fields, and the account is listed as
    // connecting meanwhile.
    until(&mut h, "the dialog's form", |h| {
        shown(h, "Server") && shown(h, "Connect Test server") && shown(h, "connecting…")
    });

    // An agent fills the form but may not give the password.
    let refused = run(
        &mut h,
        "import.connectionAnswer",
        json!({ "values": { "host": "nas.local", "password": "right" } }),
        Origin::Agent("helper".into()),
    )
    .unwrap_err();
    assert!(refused.contains("never given by an agent"), "{refused}");

    run(
        &mut h,
        "import.connectionAnswer",
        json!({ "values": { "host": "nas.local", "user": "ann", "password": "right" } }),
        Origin::User,
    )
    .unwrap();
    until(&mut h, "connected", |h| {
        connection(h)["state"] == "connected"
    });
    until(&mut h, "the account in the Sources column", |h| {
        shown(h, "ann@nas.local") && shown(h, "connected")
    });
    let accounts = world.book.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].state, AccountState::Connected);
    assert_eq!(
        world
            .secrets
            .get(&accounts[0].id, "password")
            .unwrap()
            .as_deref(),
        Some("right")
    );
    h.get_by_label("Close").click();
    until(&mut h, "the dialog closed", |h| connection(h).is_null());
}

#[test]
fn settings_accounts_lists_the_accounts_and_signs_out_and_reconnects() {
    let (world, mut h) = world();
    let mut account = helios_connect::account::Account::new(ID, "Test server", &[]);
    account.label = "ann@nas.local".into();
    account.facts.insert("host".into(), "nas.local".into());
    account.secrets.push("password".into());
    account.state = AccountState::Connected;
    world.book.save(&account).unwrap();
    world.secrets.set(&account.id, "password", "right").unwrap();

    let rows = run(&mut h, "import.accounts", Value::Null, Origin::User).unwrap();
    let row = &rows["accounts"][0];
    assert_eq!(row["label"], "ann@nas.local", "{rows}");
    assert_eq!(row["connectorText"], "Test server");
    assert_eq!(row["stateText"], "connected");
    assert_eq!(row["tone"], "success");

    // The Settings app's Accounts scope shows the table.
    run(
        &mut h,
        "admin.open",
        json!({ "path": "accounts/list" }),
        Origin::User,
    )
    .unwrap();
    until(&mut h, "the accounts table", |h| shown(h, "ann@nas.local"));

    run(
        &mut h,
        "import.signOut",
        json!({ "account": account.id }),
        Origin::User,
    )
    .unwrap();
    assert_eq!(world.secrets.get(&account.id, "password").unwrap(), None);
    assert_eq!(
        world.book.accounts().unwrap()[0].state,
        AccountState::NotConnected
    );

    // Reconnect replays the whole process of a signed-out account: its
    // form first, in the import window.
    run(
        &mut h,
        "import.reconnect",
        json!({ "account": account.id }),
        Origin::User,
    )
    .unwrap();
    until(&mut h, "the form again", |h| {
        connection(h)["state"] == "form"
    });
    assert!(h.state().import_window_open());
    run(
        &mut h,
        "import.connectionAnswer",
        json!({ "values": { "host": "nas.local", "password": "wrong" } }),
        Origin::User,
    )
    .unwrap();
    until(&mut h, "refused", |h| connection(h)["state"] == "failed");
    assert_eq!(connection(&mut h)["why"], "the source refused the sign-in");
    until(&mut h, "needs attention in the Sources column", |h| {
        shown(h, "needs attention")
    });
    assert!(matches!(
        world.book.accounts().unwrap()[0].state,
        AccountState::NeedsAttention { .. }
    ));
}
