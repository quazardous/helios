//! Import accounts: "Add a source…" in the import window's Sources
//! column offers the connectors that declare a connection process, runs
//! their steps in the window's dialog (here a fake connector's form and
//! test), and the account appears in the column with its state;
//! Settings > Accounts lists it with Reconnect and Sign out. An agent may
//! fill a form, never a secret. Settings > Accounts also sets the
//! person's own OAuth client of a connector signing in through OAuth.

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

/// An account's source selected in the Sources column: the Choices column
/// shows its choices, without those its connector cannot honour (a
/// remote one always copies, and has no darktable sidecars); a folder
/// ticked shows them again.
#[test]
fn the_choices_follow_the_selected_source_and_hide_what_its_connector_cannot_honour() {
    let (world, mut h) = world();
    let mut account = helios_connect::account::Account::new(ID, "ann@nas.local", &[]);
    account.state = AccountState::Connected;
    world.book.save(&account).unwrap();
    let kept = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    let (read, write) = (kept.clone(), kept.clone());
    h.state_mut().keep_import_sources_with(
        Box::new(move || Ok(read.borrow().clone())),
        Box::new(move |text| {
            *write.borrow_mut() = Some(text.to_string());
            Ok(())
        }),
    );
    let folder =
        std::env::temp_dir().join(format!("helios-gui-accounts-src-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    run(
        &mut h,
        "library.importWindow",
        json!({ "source": folder }),
        Origin::User,
    )
    .unwrap();
    until(&mut h, "the account listed", |h| shown(h, "ann@nas.local"));
    assert!(shown(&h, "Add where they are") && shown(&h, "Read darktable edits"));
    h.get_by_label("ann@nas.local").click();
    until(&mut h, "the account's choices", |h| {
        shown(h, "ann@nas.local: the default choices")
    });
    assert!(
        !shown(&h, "Add where they are"),
        "a remote source always copies"
    );
    assert!(!shown(&h, "Read darktable edits"), "no sidecars there");
    assert!(h.state().import_copies());
    // A choice changed is the account's own.
    h.get_by_label("Keep").click();
    until(&mut h, "the account's own choices", |h| {
        shown(h, "ann@nas.local: its own choices")
    });
    let text = kept.borrow().clone().unwrap_or_default();
    assert!(
        text.contains(&account.id) && text.contains("keep"),
        "{text}"
    );
    // A folder ticked: its choices, every one of them.
    run(
        &mut h,
        "library.importWindow",
        json!({ "source": folder }),
        Origin::User,
    )
    .unwrap();
    until(&mut h, "the folder's choices", |h| {
        shown(h, "Add where they are") && shown(h, "Read darktable edits")
    });
    assert_eq!(h.state().import_choices().duplicates.as_str(), "skip");
    let _ = std::fs::remove_dir_all(&folder);
}

/// A connector signing in through OAuth: the fake's, with an `oauth`
/// step in place of its form.
struct OauthProvider;

impl helios_connect::ConnectorProvider for OauthProvider {
    fn manifest(&self) -> helios_connect::ConnectorManifest {
        let mut m = FakeProvider.manifest();
        m.id = "fake-photos".into();
        m.connect = vec![helios_connect::ConnectStep {
            id: "sign-in".into(),
            kind: helios_connect::StepKind::Oauth {
                authorize: "http://127.0.0.1:9/authorize".into(),
                token: "http://127.0.0.1:9/token".into(),
                scopes: Vec::new(),
                revoke: None,
                client_id: None,
            },
        }];
        m
    }

    fn create(
        &self,
        settings: &helios_connect::Settings,
        services: &helios_connect::ConnectorServices,
    ) -> Result<Box<dyn helios_connect::Connector>, helios_connect::ConnectorError> {
        FakeProvider.create(settings, services)
    }
}

#[test]
fn settings_accounts_sets_the_person_s_own_oauth_client_over_helios_s() {
    use helios_connect::oauth::{self, Client, OAuth};
    let book = Arc::new(MemoryAccounts::default());
    let secrets = Arc::new(MemorySecrets::default());
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(OauthProvider));
    shell.set_import_sign_in(Arc::new(
        OAuth::with_env(Arc::new(|_| None))
            .with_built_in(Arc::new(|_| {
                Client::of(Some("helios-id"), Some("helios-secret"))
            }))
            .with_own_in(secrets.clone()),
    ));
    shell.set_import_accounts(book, secrets.clone());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let row = |h: &mut Harness<'_, Shell>| {
        run(h, "import.oauthClients", Value::Null, Origin::User).unwrap()["clients"][0].clone()
    };
    let r = row(&mut h);
    assert_eq!(r["connector"], "fake-photos");
    assert_eq!(r["source"], "builtIn");
    assert_eq!(r["own"], false);
    // An agent may not give the secret; the person types it.
    let set = |id: &str, secret: &str| json!({ "connector": "fake-photos", "clientId": id, "clientSecret": secret });
    assert!(
        run(
            &mut h,
            "import.setOAuthClient",
            set("mine", "my-secret"),
            Origin::Agent("helper".into())
        )
        .is_err()
    );
    assert_eq!(oauth::own_client(secrets.as_ref(), "fake-photos"), None);
    run(
        &mut h,
        "import.setOAuthClient",
        set("mine", "my-secret"),
        Origin::User,
    )
    .unwrap();
    let r = row(&mut h);
    assert_eq!(r["source"], "own");
    assert_eq!((&r["own"], &r["clientId"]), (&json!(true), &json!("mine")));
    assert_eq!(r["clientSecret"], "", "a secret is never read back");
    assert_eq!(
        oauth::own_client(secrets.as_ref(), "fake-photos"),
        Client::of(Some("mine"), Some("my-secret"))
    );
    // "Use Helios's client" forgets it.
    run(
        &mut h,
        "import.setOAuthClient",
        json!({ "connector": "fake-photos", "own": false }),
        Origin::User,
    )
    .unwrap();
    assert_eq!(row(&mut h)["source"], "builtIn");
    assert!(
        secrets
            .get("oauth-client/fake-photos", "client_secret")
            .unwrap()
            .is_none()
    );
    // A connector without an OAuth sign-in has no client to set.
    assert!(
        run(
            &mut h,
            "import.setOAuthClient",
            json!({ "connector": ID, "clientId": "x" }),
            Origin::User
        )
        .is_err()
    );
}
