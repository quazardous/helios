//! The generic connection flow (`connect::Session`) over a fake
//! connector that declares a form and a test step: a refused answer asks
//! again, a wrong password leaves the account needing attention,
//! reconnecting replays only the failed step, a cancelled new account is
//! forgotten, and the password lands in the keyring, never in the
//! account's document.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use helios_connect::account::{
    Account, AccountBook, AccountState, MemoryAccounts, MemorySecrets, Secrets,
};
use helios_connect::connect::{Context, Session, Status};
use helios_connect::fake::{FakeProvider, ID};
use helios_connect::{ConnectorProvider as _, ConnectorServices};

struct World {
    book: Arc<MemoryAccounts>,
    secrets: Arc<MemorySecrets>,
}

impl World {
    fn new() -> Self {
        Self {
            book: Arc::default(),
            secrets: Arc::default(),
        }
    }

    fn context(&self) -> Context {
        Context {
            provider: Arc::new(FakeProvider),
            services: ConnectorServices::default(),
            book: self.book.clone(),
            secrets: self.secrets.clone(),
            web: None,
        }
    }

    /// Starts a session and returns it with a receiver of its changes.
    fn start(&self, account: Account, new: bool) -> (Session, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let session = Session::start(self.context(), account, new, move || {
            let _ = tx.send(());
        });
        (session, rx)
    }
}

/// Waits until the session's status is one `want` accepts.
fn until(
    session: &Session,
    changes: &mpsc::Receiver<()>,
    want: impl Fn(&Status) -> bool,
) -> Status {
    loop {
        let status = session.status();
        if want(&status) {
            return status;
        }
        changes
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("the session stays at {status:?}"));
    }
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn new_account(world: &World) -> Account {
    let label = FakeProvider.manifest().label.en;
    Account::new(ID, &label, &world.book.accounts().unwrap())
}

#[test]
fn a_form_then_a_test_connect_the_account_with_its_password_in_the_keyring() {
    let world = World::new();
    let (session, changes) = world.start(new_account(&world), true);
    let form = until(&session, &changes, |s| matches!(s, Status::Form { .. }));
    let Status::Form {
        step,
        fields,
        error,
    } = form
    else {
        unreachable!()
    };
    assert_eq!(step, "server");
    assert_eq!(fields.len(), 3);
    assert_eq!(error, None);
    // The account is there while it connects.
    let saved = world.book.accounts().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].state.name(), "connecting");

    // A required field left empty: asked again, said why.
    session.answer(values(&[("host", "nas.local")]));
    let again = until(&session, &changes, |s| {
        matches!(s, Status::Form { error: Some(_), .. })
    });
    assert!(matches!(again, Status::Form { error: Some(e), .. } if e == "Password is needed"));

    session.answer(values(&[
        ("host", "nas.local"),
        ("user", "ann"),
        ("password", "right"),
    ]));
    let done = until(&session, &changes, Status::ended);
    let Status::Connected(account) = done else {
        panic!("{done:?}")
    };
    assert_eq!(account.label, "ann@nas.local");
    assert_eq!(account.state, AccountState::Connected);
    assert_eq!(account.facts["host"], "nas.local");
    assert!(!account.facts.contains_key("password"));
    assert_eq!(account.secrets, ["password"]);
    assert_eq!(
        world
            .secrets
            .get(&account.id, "password")
            .unwrap()
            .as_deref(),
        Some("right")
    );
    let doc = account.to_document().to_string();
    assert!(!doc.contains("right"), "a secret in the document: {doc}");
    assert_eq!(world.book.accounts().unwrap(), [account]);
}

#[test]
fn a_refused_test_needs_attention_and_reconnecting_replays_only_that_step() {
    let world = World::new();
    let (session, changes) = world.start(new_account(&world), true);
    until(&session, &changes, |s| matches!(s, Status::Form { .. }));
    session.answer(values(&[("host", "nas.local"), ("password", "wrong")]));
    let failed = until(&session, &changes, Status::ended);
    assert_eq!(
        failed,
        Status::Failed {
            step: "probe".into(),
            why: "the source refused the sign-in".into()
        }
    );
    let account = world.book.accounts().unwrap().remove(0);
    assert!(matches!(
        &account.state,
        AccountState::NeedsAttention { step: Some(s), .. } if s == "probe"
    ));

    // The password is changed on the server's side; reconnecting runs the
    // test again, without asking the form.
    world.secrets.set(&account.id, "password", "right").unwrap();
    let (session, changes) = world.start(account, false);
    let done = until(&session, &changes, |s| {
        s.ended() || matches!(s, Status::Form { .. })
    });
    assert!(
        matches!(done, Status::Connected(ref a) if a.label == "nas.local"),
        "{done:?}"
    );
}

#[test]
fn a_new_account_cancelled_is_forgotten_and_a_known_one_keeps_its_state() {
    let world = World::new();
    let (session, changes) = world.start(new_account(&world), true);
    until(&session, &changes, |s| matches!(s, Status::Form { .. }));
    session.cancel();
    assert_eq!(until(&session, &changes, Status::ended), Status::Cancelled);
    assert!(world.book.accounts().unwrap().is_empty());

    let mut known = new_account(&world);
    known.state = AccountState::NotConnected;
    world.book.save(&known).unwrap();
    let (session, changes) = world.start(known.clone(), false);
    until(&session, &changes, |s| matches!(s, Status::Form { .. }));
    session.cancel();
    until(&session, &changes, Status::ended);
    assert_eq!(world.book.accounts().unwrap(), [known]);
}
