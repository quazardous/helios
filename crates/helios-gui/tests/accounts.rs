//! Import accounts: "Add a source…" in the import window's Sources
//! column offers the connectors that declare a connection process, runs
//! their steps in the window's dialog (here a fake connector's form and
//! test), and the account appears in the column with its state;
//! Settings > Accounts lists it with Reconnect and Sign out. An agent may
//! fill a form, never a secret. Settings > Accounts also sets the
//! person's own OAuth client of a connector signing in through OAuth.
//! An account is shown with its address once its sign-in read it; one
//! signed in before its connector asked who it is offers a reconnection;
//! a picking done offers "Pick more photos…", a new picking each time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_connect::ConnectorProvider as _;
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
    h.run_steps(2);
    h.get_by_label("Next").click();
    until(&mut h, "the account step", |h| {
        shown(h, "Sign in to an account…")
    });
    h.get_by_label("Sign in to an account…").click();
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

/// The sources kept in memory, as the node would keep them.
fn keep_sources(h: &mut Harness<'_, Shell>) -> std::rc::Rc<std::cell::RefCell<Option<String>>> {
    let kept = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    let (read, write) = (kept.clone(), kept.clone());
    h.state_mut().keep_import_sources_with(
        Box::new(move || Ok(read.borrow().clone())),
        Box::new(move |text| {
            *write.borrow_mut() = Some(text.to_string());
            Ok(())
        }),
    );
    kept
}

fn click(h: &mut Harness<'_, Shell>, label: &str) {
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no {label}"))
        .click();
    h.run_steps(3);
}

/// The wizard taken to the account step of the fake connector, then an
/// account signed in to there: the wizard picks it.
fn wizard_signed_in(h: &mut Harness<'_, Shell>) {
    run(h, "library.importWindow", Value::Null, Origin::User).unwrap();
    until(h, "Add a source…", |h| shown(h, "Add a source…"));
    click(h, "Add a source…");
    // The connector's line says what it does.
    until(h, "the kinds", |h| shown(h, "A server for tests"));
    click(h, "Test server");
    click(h, "Next");
    until(h, "the account step", |h| {
        shown(h, "Sign in to an account…")
    });
    assert!(shown(h, "No account yet"));
    click(h, "Sign in to an account…");
    until(h, "the form", |h| connection(h)["state"] == "form");
    run(
        h,
        "import.connectionAnswer",
        json!({ "values": { "host": "nas.local", "user": "ann", "password": "right" } }),
        Origin::User,
    )
    .unwrap();
    until(h, "connected", |h| connection(h)["state"] == "connected");
    click(h, "Close");
    until(h, "the dialog closed", |h| connection(h).is_null());
    until(h, "the account listed in the step", |h| {
        shown(h, "ann@nas.local")
    });
}

/// "Add a source…" for a connector reached through an account: its
/// account signed in to in the wizard, the choices it cannot honour
/// hidden and never kept on it, the source saved under its name and
/// selected.
#[test]
fn the_add_a_source_wizard_signs_in_to_an_account_and_hides_what_a_remote_source_cannot_honour() {
    let (world, mut h) = world();
    let kept = keep_sources(&mut h);
    // The window's choices add where they are and read darktable edits.
    let mut start = h.state().import_choices();
    start.copy_folders = false;
    start.darktable = true;
    h.state_mut().set_import_choices(start);
    wizard_signed_in(&mut h);
    click(&mut h, "Next");
    until(&mut h, "the choices", |h| shown(h, "Duplicates"));
    assert!(
        !shown(&h, "Add where they are"),
        "a remote source always copies"
    );
    assert!(!shown(&h, "Read darktable edits"), "no sidecars there");
    click(&mut h, "Keep");
    click(&mut h, "Next");
    // The name starts as the account's; the person makes it theirs.
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(egui_kittest::Node::is_focused)
        .expect("the name field focused")
        .type_text(" photos");
    h.run_steps(3);
    click(&mut h, "Add the source");
    let accounts = world.book.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    let source = &sources[0];
    assert_eq!(source.connector, ID);
    assert_eq!(source.account.as_deref(), Some(accounts[0].id.as_str()));
    assert!(source.path.is_none());
    let name = source.name.clone().unwrap_or_default();
    assert!(name.contains("photos"), "the name typed: {name}");
    let choices = source.choices.clone().expect("its own choices");
    assert!(choices.copy_folders && !choices.darktable, "{choices:?}");
    assert_eq!(choices.duplicates.as_str(), "keep");
    assert!(
        kept.borrow()
            .as_deref()
            .unwrap_or_default()
            .contains(&accounts[0].id)
    );
    // Selected: the column shows its choices, under its name.
    until(&mut h, "its choices shown", |h| {
        shown(h, &format!("{name}: its own choices"))
    });
    assert!(shown(&h, "Saved sources") && shown(&h, &name));
}

/// Cancel in the wizard leaves no source, wherever it is pressed; an
/// account signed in to on the way stays an account.
#[test]
fn cancelling_the_add_a_source_wizard_after_signing_in_keeps_the_account_only() {
    let (world, mut h) = world();
    let kept = keep_sources(&mut h);
    wizard_signed_in(&mut h);
    click(&mut h, "Cancel");
    assert!(!h.state().import_wizard_open());
    assert!(h.state().import_sources().is_empty());
    assert_eq!(world.book.accounts().unwrap().len(), 1);
    // Again, to the choices and to the name: the account is offered, picked
    // by a click.
    for steps in [1, 2] {
        click(&mut h, "Add a source…");
        click(&mut h, "Test server");
        click(&mut h, "Next");
        click(&mut h, "ann@nas.local");
        for _ in 0..steps {
            click(&mut h, "Next");
        }
        click(&mut h, "Cancel");
        assert!(!h.state().import_wizard_open(), "{steps}");
        assert!(h.state().import_sources().is_empty(), "{steps}");
    }
    assert_eq!(*kept.borrow(), None);
}

/// The browser openings asked, with when: an opener that returns at
/// once, as the platform's does.
#[derive(Clone, Default)]
struct Opened(Arc<std::sync::Mutex<Vec<(String, Instant)>>>);

impl Opened {
    fn opener(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let seen = self.0.clone();
        Arc::new(move |url: &str| {
            seen.lock().unwrap().push((url.to_owned(), Instant::now()));
        })
    }

    fn urls(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(u, _)| u.clone())
            .collect()
    }

    /// Waits for the `n`th opening, without a frame of the window: the
    /// time it took from `from`.
    fn nth_after(&self, n: usize, from: Instant) -> Duration {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some((_, at)) = self.0.lock().unwrap().get(n - 1) {
                return at.saturating_duration_since(from);
            }
            assert!(Instant::now() < deadline, "the browser was never opened");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// A sign-in whose consent page is opened, then waited on until
/// cancelled.
struct SlowSignIn;

impl helios_connect::connect::WebSteps for SlowSignIn {
    fn oauth(
        &self,
        _account: &mut helios_connect::account::Account,
        _step: &helios_connect::StepKind,
        _secrets: &dyn helios_connect::account::Secrets,
        browser: &dyn Fn(&str),
        cancel: &helios_connect::Cancel,
    ) -> Result<(), String> {
        browser("https://accounts.example.org/consent?state=1");
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err("cancelled".into())
    }
}

#[test]
fn a_sign_in_opens_the_browser_at_once_without_waiting_for_a_frame() {
    let book = Arc::new(MemoryAccounts::default());
    let secrets = Arc::new(MemorySecrets::default());
    let opened = Opened::default();
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(OauthProvider));
    shell.set_import_sign_in(Arc::new(SlowSignIn));
    shell.set_import_browser(opened.opener());
    shell.set_import_accounts(book, secrets);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "library.importWindow", Value::Null, Origin::User).unwrap();
    until(&mut h, "Add a source…", |h| shown(h, "Add a source…"));
    h.get_by_label("Add a source…").click();
    until(&mut h, "the connectors offered", |h| {
        shown(h, "Test server")
    });
    h.get_by_label("Test server").click();
    h.run_steps(2);
    h.get_by_label("Next").click();
    until(&mut h, "the account step", |h| {
        shown(h, "Sign in to an account…")
    });
    h.get_by_label("Sign in to an account…").click();
    // The frame that takes the click starts the sign-in; the browser
    // opens from the sign-in itself, no later frame or input needed.
    let clicked = Instant::now();
    h.step();
    let took = opened.nth_after(1, clicked);
    assert!(took < Duration::from_millis(200), "opened after {took:?}");
    assert_eq!(
        opened.urls(),
        ["https://accounts.example.org/consent?state=1"]
    );
    // The dialog shows the link, to copy, and opens it again on demand.
    until(&mut h, "the link and Open again", |h| {
        shown(h, "Open again") && shown(h, "Copy link")
    });
    h.run_steps(4);
    assert_eq!(opened.urls().len(), 1, "opened once per page");
    h.get_by_label("Open again").click();
    h.run_steps(2);
    assert_eq!(opened.urls().len(), 2, "and again when asked");
}

/// A connector that picks: each picking a new session (`s1`, `s2`…), the
/// first `expired` of them expiring before the person is done; with
/// `ready`, the person is done at once.
struct Picks {
    sessions: Arc<std::sync::atomic::AtomicU32>,
    expired: u32,
    ready: bool,
}

const PICKS: &str = "fake-picks";

impl helios_connect::ConnectorProvider for Picks {
    fn manifest(&self) -> helios_connect::ConnectorManifest {
        let mut m = FakeProvider.manifest();
        m.id = PICKS.into();
        m.caps.picks = true;
        // As Google Photos declares: its own picking page alone.
        m.browsers = vec![helios_connect::Browser::Picker];
        m
    }

    fn create(
        &self,
        _settings: &helios_connect::Settings,
        _services: &helios_connect::ConnectorServices,
    ) -> Result<Box<dyn helios_connect::Connector>, helios_connect::ConnectorError> {
        Ok(Box::new(PickConnector {
            sessions: self.sessions.clone(),
            expired: self.expired,
            ready: self.ready,
            current: std::sync::Mutex::new(0),
        }))
    }
}

struct PickConnector {
    sessions: Arc<std::sync::atomic::AtomicU32>,
    expired: u32,
    ready: bool,
    current: std::sync::Mutex<u32>,
}

impl helios_connect::Connector for PickConnector {
    fn describe(&self) -> helios_connect::ConnectorInfo {
        helios_connect::ConnectorInfo {
            id: PICKS.into(),
            label: FakeProvider.manifest().label,
            caps: Picks {
                sessions: Arc::default(),
                expired: 0,
                ready: false,
            }
            .manifest()
            .caps,
        }
    }

    fn sources(
        &self,
        _account: Option<&helios_connect::AccountRef>,
    ) -> Result<Vec<helios_connect::SourceRef>, helios_connect::ConnectorError> {
        Ok(Vec::new())
    }

    fn list(
        &self,
        _source: &helios_connect::SourceRef,
        _from: Option<&helios_connect::Cursor>,
        _page: usize,
        _cancel: &helios_connect::Cancel,
    ) -> Result<helios_connect::Page, helios_connect::ConnectorError> {
        Ok(helios_connect::Page {
            complete: true,
            ..helios_connect::Page::default()
        })
    }

    fn pick(
        &self,
        account: &helios_connect::AccountRef,
        _cancel: &helios_connect::Cancel,
    ) -> Result<helios_connect::Pick, helios_connect::ConnectorError> {
        let n = self
            .sessions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        *self.current.lock().unwrap() = n;
        Ok(helios_connect::Pick {
            url: format!("https://picker.example.org/pick/s{n}"),
            source: helios_connect::SourceRef {
                connector: PICKS.into(),
                id: account.id.clone(),
                label: "Picked".into(),
                root: None,
                capacity: None,
            },
        })
    }

    fn picked(
        &self,
        deadline: Instant,
        cancel: &helios_connect::Cancel,
    ) -> Result<bool, helios_connect::ConnectorError> {
        if *self.current.lock().unwrap() <= self.expired {
            std::thread::sleep(Duration::from_millis(20));
            return Err(helios_connect::ConnectorError::Expired);
        }
        if self.ready {
            return Ok(true);
        }
        while Instant::now() < deadline {
            if cancel.is_cancelled() {
                return Err(helios_connect::ConnectorError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(false)
    }

    fn access(&self, _item: &helios_connect::ItemRef) -> helios_connect::Access {
        helios_connect::Access::Fetch
    }

    fn fetch(
        &self,
        _item: &helios_connect::ItemRef,
        _offset: u64,
        _sink: &mut dyn helios_connect::FetchSink,
        _cancel: &helios_connect::Cancel,
    ) -> Result<helios_connect::Fetched, helios_connect::ConnectorError> {
        Err(helios_connect::ConnectorError::NotFound)
    }
}

#[test]
fn a_picking_opens_its_page_at_once_and_an_expired_session_is_replaced() {
    let book = Arc::new(MemoryAccounts::default());
    let secrets = Arc::new(MemorySecrets::default());
    let mut account = helios_connect::account::Account::new(PICKS, "ann@example.org", &[]);
    account.state = AccountState::Connected;
    book.save(&account).unwrap();
    let sessions = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let opened = Opened::default();
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(Picks {
        sessions: sessions.clone(),
        expired: 1,
        ready: false,
    }));
    shell.set_import_browser(opened.opener());
    shell.set_import_remote(Arc::new(|_, _, _, _| {
        Ok(helios_gui::shell::PickImported::default())
    }));
    shell.set_import_accounts(book, secrets);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "library.importWindow", Value::Null, Origin::User).unwrap();
    until(&mut h, "Pick photos…", |h| shown(h, "Pick photos…"));
    h.get_by_label("Pick photos…").click();
    let clicked = Instant::now();
    h.step();
    let took = opened.nth_after(1, clicked);
    assert!(took < Duration::from_millis(200), "opened after {took:?}");
    assert_eq!(opened.urls(), ["https://picker.example.org/pick/s1"]);
    // The first session expires: said so, not waited out, and a new
    // session replaces it.
    until(&mut h, "the session said expired", |h| {
        shown(h, "The picking session expired, start again.")
    });
    // The dialog settles where it now stands before the click.
    h.run_steps(4);
    h.get_by_label("Start again").click();
    until(&mut h, "the new session's page opened", |_| {
        opened.urls().len() == 2
    });
    assert_eq!(
        opened.urls(),
        [
            "https://picker.example.org/pick/s1",
            "https://picker.example.org/pick/s2"
        ]
    );
    assert_eq!(sessions.load(std::sync::atomic::Ordering::SeqCst), 2);
    // The dialog names the account the browser must be signed in to.
    until(&mut h, "the account named", |h| {
        h.query_all_by_label_contains("signed in to the account connected as ann@example.org")
            .next()
            .is_some()
    });
}

/// "Edit source…" on an account's source: its connector's own parameter
/// (the fake's `share`) set and kept; add where they are never offered,
/// and refused when an older document held it.
#[test]
fn edit_source_sets_an_account_sources_parameter_and_refuses_add_where_they_are() {
    use egui_kittest::kittest::By;
    let (_world, mut h) = world();
    let kept = keep_sources(&mut h);
    wizard_signed_in(&mut h);
    // The connector's own parameter, at its default, under the account.
    let share = |value: &'static str| {
        By::new()
            .role(egui::accesskit::Role::TextInput)
            .value(value)
    };
    assert!(h.query(share("photos")).is_some(), "the share's field");
    click(&mut h, "Next");
    click(&mut h, "Next");
    click(&mut h, "Add the source");
    let sources = h.state().import_sources();
    assert_eq!(sources[0].params["share"], json!("photos"));
    let name = sources[0].name.clone().unwrap_or_default();
    // A document saying add where they are (an older Helios's, say).
    let text = kept
        .borrow()
        .clone()
        .unwrap_or_default()
        .replace("copy_folders = true", "copy_folders = false");
    assert!(text.contains("copy_folders = false"), "{text}");
    *kept.borrow_mut() = Some(text);
    let (read, write) = (kept.clone(), kept.clone());
    h.state_mut().keep_import_sources_with(
        Box::new(move || Ok(read.borrow().clone())),
        Box::new(move |text| {
            *write.borrow_mut() = Some(text.to_string());
            Ok(())
        }),
    );
    h.run_steps(3);
    assert!(
        !h.state().import_sources()[0]
            .choices
            .as_ref()
            .unwrap()
            .copy_folders
    );
    // Edited: its share changed, copy kept as the only way.
    h.query_all_by_label(&name)
        .next()
        .expect("its row")
        .click_secondary();
    h.run_steps(3);
    click(&mut h, "Edit source…");
    until(&mut h, "the editor", |h| shown(h, "Edit source"));
    h.get(share("photos")).focus();
    h.run_steps(2);
    h.get(share("photos")).type_text("-2026");
    h.run_steps(3);
    click(&mut h, "Next");
    until(&mut h, "the choices", |h| shown(h, "Duplicates"));
    assert!(!shown(&h, "Add where they are"), "copy only");
    click(&mut h, "Next");
    click(&mut h, "Save the source");
    assert!(!h.state().import_wizard_open());
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    let share = sources[0].params["share"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(share.contains("2026") && share != "photos", "{share}");
    let choices = sources[0].choices.clone().unwrap();
    assert!(choices.copy_folders && !choices.darktable, "{choices:?}");
    let text = kept.borrow().clone().unwrap_or_default();
    assert!(text.contains("share = ") && text.contains("2026"), "{text}");
}

/// The library folder as a vault keeps it, in memory.
#[derive(Default)]
struct Folder(std::sync::Mutex<Option<std::path::PathBuf>>);

impl helios_gui::shell::LibraryFolder for Folder {
    fn get(&self) -> Result<Option<std::path::PathBuf>, String> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn set(&self, dir: &std::path::Path) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(dir.to_path_buf());
        Ok(())
    }
}

/// The folders the picked photos were imported into, by import.
type Imported = Arc<std::sync::Mutex<Vec<Option<std::path::PathBuf>>>>;

/// A window with a connected account of a connector whose picking is done
/// at once, the library folder and the imports the test's.
fn picking_world(
    sources: Option<String>,
) -> (
    Harness<'static, Shell>,
    Opened,
    Arc<Folder>,
    Imported,
    Arc<std::sync::atomic::AtomicU32>,
) {
    let book = Arc::new(MemoryAccounts::default());
    let mut account = helios_connect::account::Account::new(PICKS, "ann@example.org", &[]);
    account.state = AccountState::Connected;
    book.save(&account).unwrap();
    let sessions = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let (opened, folder, imported) = (
        Opened::default(),
        Arc::new(Folder::default()),
        Imported::default(),
    );
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(Picks {
        sessions: sessions.clone(),
        expired: 0,
        ready: true,
    }));
    shell.set_import_browser(opened.opener());
    let seen = imported.clone();
    shell.set_import_remote(Arc::new(move |_, _, _, into| {
        seen.lock().unwrap().push(into);
        // The second picking finds the first one's photo again.
        let known = u64::from(seen.lock().unwrap().len() > 1);
        Ok(helios_gui::shell::PickImported { added: 1, known })
    }));
    shell.set_import_library_folder(folder.clone());
    shell.set_import_accounts(book, Arc::new(MemorySecrets::default()));
    if let Some(doc) = sources {
        let doc = doc.replace("{account}", &account.id);
        shell.keep_import_sources_with(
            Box::new(move || Ok(Some(doc.clone()))),
            Box::new(|_| Ok(())),
        );
    }
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "library.importWindow", Value::Null, Origin::User).unwrap();
    until(&mut h, "Pick photos…", |h| shown(h, "Pick photos…"));
    (h, opened, folder, imported, sessions)
}

#[test]
fn with_no_library_folder_it_is_asked_before_the_picking_opens_then_the_photos_go_there() {
    let (mut h, opened, folder, imported, sessions) = picking_world(None);
    h.get_by_label("Pick photos…").click();
    until(&mut h, "the library folder asked", |h| {
        shown(h, "Choose the library folder…")
    });
    // No picking page, no session at the service: the folder comes first,
    // and the command line's wording never shows.
    h.run_steps(4);
    assert!(opened.urls().is_empty());
    assert_eq!(sessions.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        h.query_all_by_label_contains("--into").next().is_none(),
        "the command line's wording"
    );
    h.get_by_label("Choose the library folder…").click();
    h.run_steps(3);
    // The folder chooser answers.
    let dir = std::env::temp_dir().join(format!("helios-pick-library-{}", std::process::id()));
    let app = h.state().app().handle().clone();
    let files = app
        .service(&egui_pop::api::files::FILES)
        .expect("the files service");
    let ids = files.open_ids();
    assert_eq!(ids.len(), 1, "one folder chooser open");
    files.deliver(ids[0], Some(vec![dir.clone()])).unwrap();
    until(&mut h, "the picked photos imported", |_| {
        !imported.lock().unwrap().is_empty()
    });
    // Remembered as the library folder, then the picking opened and its
    // photos imported there.
    assert_eq!(folder.0.lock().unwrap().as_deref(), Some(dir.as_path()));
    assert_eq!(opened.urls(), ["https://picker.example.org/pick/s1"]);
    assert_eq!(imported.lock().unwrap()[0].as_deref(), Some(dir.as_path()));
    until(&mut h, "said imported", |h| shown(h, "1 photos imported."));
}

#[test]
fn a_source_s_own_into_folder_takes_the_picked_photos_without_asking() {
    let doc = "[[source]]\nconnector = \"fake-picks\"\naccount = \"{account}\"\n\n[source.choices]\ninto = \"/photos/picked\"\n";
    let (mut h, opened, folder, imported, _) = picking_world(Some(doc.into()));
    assert_eq!(h.state().import_sources().len(), 1, "the source read");
    h.get_by_label("Pick photos…").click();
    until(&mut h, "the picked photos imported", |_| {
        !imported.lock().unwrap().is_empty()
    });
    assert_eq!(
        imported.lock().unwrap()[0].as_deref(),
        Some(std::path::Path::new("/photos/picked"))
    );
    assert_eq!(opened.urls().len(), 1);
    // Nothing asked, nothing remembered as the library's.
    assert!(!shown(&h, "Choose the library folder…"));
    assert_eq!(*folder.0.lock().unwrap(), None);
}

/// A connector declaring three browsers: the Contents column offers
/// them for its account, the one chosen is kept on the source and comes
/// back when the sources are read again.
#[test]
fn an_account_source_offers_its_connectors_three_browsers_and_keeps_the_one_chosen() {
    use helios_connect::Browser;
    let (_world, mut h) = world();
    let kept = keep_sources(&mut h);
    wizard_signed_in(&mut h);
    click(&mut h, "Next");
    click(&mut h, "Next");
    click(&mut h, "Add the source");
    until(&mut h, "the switch", |h| shown(h, "Albums"));
    let (offered, current) = h.state().import_source_browsers();
    assert_eq!(
        offered,
        [Browser::Folder, Browser::Collections, Browser::Timeline]
    );
    assert_eq!(current, Some(Browser::Folder));
    for label in ["Browse", "All photos", "Albums", "By date"] {
        assert!(shown(&h, label), "{label}");
    }
    assert!(!shown(&h, "Folder tree") && !shown(&h, "Pick in the service"));
    // Nothing lists an account's photos yet: said, no grid of folders.
    assert!(shown(
        &h,
        "Helios does not list this source's photos here yet."
    ));
    click(&mut h, "By date");
    assert_eq!(
        h.state().import_source_browsers().1,
        Some(Browser::Timeline)
    );
    assert_eq!(
        h.state().import_sources()[0].browser,
        Some(Browser::Timeline)
    );
    let text = kept.borrow().clone().unwrap_or_default();
    assert!(text.contains("browser = \"timeline\""), "{text}");
    // Read again, as after a restart: shown by date.
    let (read, write) = (kept.clone(), kept.clone());
    h.state_mut().keep_import_sources_with(
        Box::new(move || Ok(read.borrow().clone())),
        Box::new(move |text| {
            *write.borrow_mut() = Some(text.to_string());
            Ok(())
        }),
    );
    let account = h.state().import_sources()[0].account.clone().unwrap();
    h.state_mut().select_import_account(&account);
    h.run_steps(3);
    assert_eq!(
        h.state().import_source_browsers().1,
        Some(Browser::Timeline)
    );
}

/// A connector that picks in its service's page alone (Google Photos):
/// no switch, the Contents column offers its picking.
#[test]
fn a_picking_source_shows_only_its_picker_in_the_contents_column() {
    use helios_connect::Browser;
    let (mut h, _opened, _folder, _imported, _) = picking_world(None);
    click(&mut h, "ann@example.org");
    until(&mut h, "the picker shown", |h| {
        shown(
            h,
            "Choose the photos in the service's own page: they are imported once picked.",
        )
    });
    assert_eq!(
        h.state().import_source_browsers(),
        (vec![Browser::Picker], Some(Browser::Picker))
    );
    for label in ["Browse", "All photos", "By date", "Albums", "Folder tree"] {
        assert!(!shown(&h, label), "{label}");
    }
    // Its button runs the picking, the library folder asked first.
    h.query_all_by_label("Pick photos…")
        .last()
        .expect("the Contents column's")
        .click();
    until(&mut h, "the library folder asked", |h| {
        shown(h, "Choose the library folder…")
    });
}

/// [`Picks`] signing in through OAuth asking who the account is (the
/// `email` scope), as Google Photos does.
struct PicksAsking(Picks);

impl helios_connect::ConnectorProvider for PicksAsking {
    fn manifest(&self) -> helios_connect::ConnectorManifest {
        let mut m = self.0.manifest();
        m.connect.insert(
            0,
            helios_connect::ConnectStep {
                id: "sign-in".into(),
                kind: helios_connect::StepKind::Oauth {
                    authorize: "https://accounts.example.org/auth".into(),
                    token: "https://accounts.example.org/token".into(),
                    scopes: vec!["photos".into(), "openid".into(), "email".into()],
                    revoke: None,
                    client_id: None,
                },
            },
        );
        m
    }

    fn create(
        &self,
        settings: &helios_connect::Settings,
        services: &helios_connect::ConnectorServices,
    ) -> Result<Box<dyn helios_connect::Connector>, helios_connect::ConnectorError> {
        self.0.create(settings, services)
    }
}

#[test]
fn an_account_shows_its_address_and_one_signed_in_before_offers_a_reconnection() {
    use helios_connect::account::{Account, EMAIL};
    use helios_connect::oauth::CLIENT_ID;
    let book = Arc::new(MemoryAccounts::default());
    let mut known = Account::new(PICKS, "Fake picks", &[]);
    known.state = AccountState::Connected;
    known.facts.insert(CLIENT_ID.into(), "client".into());
    known.facts.insert(EMAIL.into(), "ann@example.com".into());
    book.save(&known).unwrap();
    let mut old = Account::new(PICKS, "Fake picks", std::slice::from_ref(&known));
    old.state = AccountState::Connected;
    old.facts.insert(CLIENT_ID.into(), "client".into());
    book.save(&old).unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(Arc::new(PicksAsking(Picks {
        sessions: Arc::default(),
        expired: 0,
        ready: true,
    })));
    shell.set_import_remote(Arc::new(|_, _, _, _| {
        Ok(helios_gui::shell::PickImported::default())
    }));
    shell.set_import_accounts(book, Arc::new(MemorySecrets::default()));
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "library.importWindow", Value::Null, Origin::User).unwrap();
    until(&mut h, "the account's address", |h| {
        shown(h, "Fake picks — ann@example.com")
    });
    assert!(shown(&h, "Reconnect to see the account"));
    assert_eq!(
        h.query_all_by_label("Reconnect to see the account").count(),
        1,
        "the old account only"
    );
    // Settings > Accounts says the same.
    let rows = run(&mut h, "import.accounts", Value::Null, Origin::User).unwrap();
    let row = |id: &str| {
        rows["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
            .unwrap()
    };
    let (k, o) = (row(&known.id), row(&old.id));
    assert_eq!(k["label"], "Fake picks — ann@example.com");
    assert_eq!(k["email"], "ann@example.com");
    assert_eq!(k["tone"], "success");
    assert_eq!(o["label"], "Fake picks");
    assert_eq!(o["why"], "Reconnect to see the account");
    assert_eq!(o["tone"], "warning");
}

#[test]
fn a_picking_done_offers_to_pick_more_in_a_new_session() {
    let doc = "[[source]]\nconnector = \"fake-picks\"\naccount = \"{account}\"\n\n[source.choices]\ninto = \"/photos/picked\"\n";
    let (mut h, opened, _folder, imported, sessions) = picking_world(Some(doc.into()));
    h.get_by_label("Pick photos…").click();
    until(&mut h, "the first picking done", |h| {
        shown(h, "1 photos imported.")
    });
    h.run_steps(4);
    h.get_by_label("Pick more photos…").click();
    until(&mut h, "the second picking done", |_| {
        imported.lock().unwrap().len() == 2
    });
    // A session of its own, its photos added to the same source, the one
    // already in said so.
    assert_eq!(sessions.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        opened.urls(),
        [
            "https://picker.example.org/pick/s1",
            "https://picker.example.org/pick/s2"
        ]
    );
    until(&mut h, "the known photo said", |h| {
        shown(h, "1 already in the library, left as they are.")
    });
}
