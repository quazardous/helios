//! The import window's accounts (`docs/design/import-connectors.md`,
//! sections 2.4.1 and 8.2): the Sources column lists the accounts of the
//! connectors that declare a connection process, each with its state
//! (connected, connecting, needs attention, not connected), and the "Add
//! a source…" wizard offers those connectors. Signing in there, or
//! "Reconnect" on an account, runs its declared steps through the one generic flow
//! ([`helios_connect::connect::Session`]), shown in a dialog of the
//! window: a form's fields, a spinner while a step works, the page the
//! browser was opened on, the reason a step failed.
//!
//! A page for the browser (an OAuth consent, the service's page where
//! photos are chosen) is opened the moment the step asks for it, from the
//! step's own thread, through the platform's opener ([`system_opener`])
//! on a thread of its own: never at the end of a frame, so it does not
//! wait for the window's next frame or hold the window while the opener
//! runs. The link is shown, to copy, and to open again.
//!
//! An account whose connector lets the person choose photos in the
//! service's own page (Google Photos) is shown once in the Sources column,
//! under Services, and the Detail column's connector section shows it
//! with one header line and its menu, then the one primary action of
//! where it stands
//! ([`choose_ui`]): connect, choose photos, the wait for the service's
//! page as a status line with Cancel (never a dialog), what came back, or
//! one plain sentence and one action when something went wrong. The
//! connector never asks for a folder: the Choices column's Into decides
//! where the photos are copied, as for any source.
//!
//! Settings > Accounts lists the same accounts through the commands of
//! [`super::accounts_admin`].
//!
//! A connector served by a plugin that still lacks some of the
//! capabilities it declares asks for them first, in a consent dialog
//! ([`consent_dialog`]): each capability with the reason the plugin's
//! manifest gives, Allow (granted, kept, the action run again) or Don't
//! allow (nothing runs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use egui_pop::core::icon::Icon;
use helios_connect::Text;
use helios_connect::account::{Account, AccountBook, AccountState, MemorySecrets, Secrets};
use helios_connect::connect::{Context, FieldKind, Session, Status, WebSteps};
use helios_core::Message;

use super::import_window::Texts;
use crate::import_scan::Connectors;

/// What the window keeps of the accounts.
pub(crate) struct Accounts {
    /// Where accounts are kept: the library's vault; none without a
    /// library (the accounts are not shown).
    pub(crate) book: Option<Arc<dyn AccountBook>>,
    /// Where their secrets go: the system keyring on the desktop.
    pub(crate) secrets: Arc<dyn Secrets>,
    /// How the steps that reach a service run (`oauth`).
    pub(crate) web: Option<Arc<dyn WebSteps>>,
    /// The accounts as last read.
    listed: Vec<Account>,
    /// Reads them off the interface's thread: the vault may be held by
    /// another writer, and a frame never waits for it.
    reader: Reader,
    /// Asks the window for a pass: an answer came after its frame.
    pub(crate) wake: Option<Wake>,
    /// The connection running, shown in the window's dialog.
    pub(crate) connecting: Option<Connecting>,
    /// The same session, for the commands that read and answer it off
    /// the window (`import.connection`).
    pub(crate) current: Current,
    /// The connectors' names, by id, for Settings > Accounts.
    pub(crate) labels: Labels,
    /// The connectors signing in through OAuth, with their manifest's
    /// client id, for Settings > Accounts' own clients.
    pub(crate) oauth: OauthConnectors,
    /// The connectors whose sign-in asks who the account is (the `email`
    /// scope): their accounts are shown with their address, and one signed
    /// in before has "Reconnect to see the account".
    pub(crate) identity: IdentityConnectors,
    /// The photos being chosen in the service's page ("Choose photos
    /// in Google Photos…"), shown in the Contents column.
    pub(crate) picking: Option<Picking>,
    /// How the chosen items are imported:
    /// `helios_library::fetch::import_remote` over the library, set by
    /// the desktop; none: choosing photos is not offered.
    pub(crate) import: Option<ImportRemote>,
    /// The library folder copies go to when Into names none, read off the
    /// interface's thread; none: never looked at.
    pub(crate) library: Option<Arc<dyn LibraryFolder>>,
    /// The plugins installed: a connector of theirs asks for the
    /// capabilities its plugin lacks before it runs.
    pub(crate) plugins: Option<Arc<helios_plugin::Broker>>,
    /// The consent asked, shown in the window's dialog.
    pub(crate) consent: Option<Consent>,
    /// How a page is opened in the system browser.
    pub(crate) opener: Opener,
}

/// Opens a page in the system browser; returns at once (the opening
/// runs on a thread of its own).
pub type Opener = Arc<dyn Fn(&str) + Send + Sync>;

/// The platform's own opener: `xdg-open` (the desktop portal inside a
/// sandbox) on Linux, `open` on macOS, `ShellExecute` on Windows, run on a
/// thread of its own so that the caller never waits on it.
pub(crate) fn system_opener() -> Opener {
    Arc::new(|url: &str| {
        let url = url.to_owned();
        let spawned = std::thread::Builder::new()
            .name("open in browser".into())
            .spawn(move || {
                if let Err(e) = open::that_detached(&url) {
                    log::warn!("cannot open the browser: {e}");
                }
            });
        if let Err(e) = spawned {
            log::warn!("cannot open the browser: {e}");
        }
    })
}

/// Asks the import window for a pass, from any thread.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// How long a frame waits for the accounts' read before it shows them as
/// last read (the read's answer then asks for a pass of its own).
const READ_WAIT: Duration = Duration::from_millis(30);

/// How long looking up a plugin's consent waits before the action is held
/// and the answer awaited at the next passes.
const LOOK_WAIT: Duration = Duration::from_millis(30);

/// A plugin's capabilities asked for before one of its actions runs. The
/// broker's answers (what the plugin lacks, a grant that stops the plugin
/// running) come from a thread of their own: a plugin's lock is held
/// through its handshake, and a grant waits for its stop.
pub(crate) enum Consent {
    /// What the plugin lacks is being looked up; the action is held.
    Looking {
        then: Asked,
        answer: Receiver<Option<Asking>>,
    },
    /// The dialog asks the person.
    Asking(Asking),
    /// Allowed: the capabilities are being granted (the plugin running is
    /// stopped meanwhile); the action runs again once they are.
    Granting {
        then: Asked,
        answer: Receiver<Result<(), String>>,
    },
}

/// What the consent dialog asks.
pub(crate) struct Asking {
    /// The plugin's id.
    plugin: String,
    /// Its name, as shown.
    title: String,
    /// What it lacks, each with its manifest's reason.
    capabilities: Vec<helios_plugin::manifest::CapabilityDecl>,
    /// The action to run again once allowed.
    then: Asked,
    /// The person said no: the calm word is shown until closed.
    declined: bool,
}

/// The accounts' read, on a thread of its own: each pass asks for one,
/// and waits for it at most [`READ_WAIT`].
#[derive(Default)]
struct Reader {
    shared: Arc<(Mutex<Read>, Condvar)>,
    started: bool,
}

#[derive(Default)]
struct Read {
    /// Where the accounts are read from.
    book: Option<Arc<dyn AccountBook>>,
    /// The reads asked, and the last one answered.
    asked: u64,
    done: u64,
    /// The accounts the last read found.
    list: Vec<Account>,
    /// Asks the window for a pass.
    wake: Option<Wake>,
    /// A pass stopped waiting for the read going: its answer asks for a
    /// pass of its own.
    late: bool,
    /// The window is gone: the thread ends.
    stop: bool,
}

impl Reader {
    /// The accounts in `book`, read again unless a read already runs late:
    /// as read within [`READ_WAIT`], else as last read.
    fn read(&mut self, book: &Arc<dyn AccountBook>, wake: Option<&Wake>) -> Vec<Account> {
        let (lock, ready) = &*self.shared;
        let mut read = lock.lock().unwrap_or_else(PoisonError::into_inner);
        read.book = Some(book.clone());
        read.wake = wake.cloned();
        if read.late {
            return read.list.clone();
        }
        if !self.started {
            self.started = true;
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new()
                .name("import-accounts".into())
                .spawn(move || read_accounts(&shared));
            if let Err(e) = spawned {
                log::warn!("import accounts: {e}");
                return read.list.clone();
            }
        }
        read.asked += 1;
        let asked = read.asked;
        ready.notify_all();
        let deadline = Instant::now() + READ_WAIT;
        while read.done < asked {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                read.late = true;
                break;
            }
            read = ready
                .wait_timeout(read, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        read.list.clone()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let (lock, ready) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
        ready.notify_all();
    }
}

/// The reading thread: reads the accounts each time a pass asks (the asks
/// that came during a read answered by the next one), never holding the
/// shared state while the vault is read.
fn read_accounts(shared: &(Mutex<Read>, Condvar)) {
    let (lock, ready) = shared;
    loop {
        let (book, asked) = {
            let mut read = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while read.done == read.asked && !read.stop {
                read = ready.wait(read).unwrap_or_else(PoisonError::into_inner);
            }
            if read.stop {
                return;
            }
            (read.book.clone(), read.asked)
        };
        let found = book.map(|b| b.accounts());
        let late = {
            let mut read = lock.lock().unwrap_or_else(PoisonError::into_inner);
            read.done = asked;
            match found {
                Some(Ok(list)) => read.list = list,
                Some(Err(e)) => log::warn!("import accounts: {e}"),
                None => {}
            }
            ready.notify_all();
            std::mem::take(&mut read.late)
                .then(|| read.wake.clone())
                .flatten()
        };
        if let Some(wake) = late {
            wake();
        }
    }
}

/// Imports the items a connector listed under a source: the items added.
///
/// The items are copied into the folder given, else into the library
/// folder.
pub type ImportRemote = Arc<
    dyn Fn(
            &dyn helios_connect::Connector,
            helios_connect::SourceRef,
            Vec<helios_connect::Item>,
            Option<PathBuf>,
        ) -> Result<Imported, String>
        + Send
        + Sync,
>;

/// What the import of the chosen photos did: `added` photos new to the
/// library, `known` already in it (chosen before), left as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub added: u64,
    pub known: u64,
}

/// The library folder a fetched item is copied into when the Choices
/// column's Into names none: the library's
/// (`helios_library::import::library_root`) on the desktop. With neither,
/// the service's page is not opened: Into is asked for, in the Choices
/// column, as for any copy.
pub trait LibraryFolder: Send + Sync {
    /// The library folder, if one is known.
    fn get(&self) -> Result<Option<PathBuf>, String>;
    /// Remembers `dir` as the library folder.
    fn set(&self, dir: &Path) -> Result<(), String>;
    /// Whether the library keeps a camera's default camera look: the
    /// import window's Choices then offer to apply it.
    fn has_camera_looks(&self) -> bool {
        false
    }
}

/// How long the service's page is waited for at most.
const PICK_WAIT: std::time::Duration = std::time::Duration::from_mins(30);

/// Where choosing photos stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickStatus {
    /// Opening the service's page.
    Starting,
    /// Waiting for the person, in the browser opened on `url`.
    Browser {
        url: String,
    },
    /// Downloading and importing `items` photos.
    Importing {
        items: usize,
    },
    /// Done: `added` photos added, `known` already imported.
    Done {
        added: u64,
        known: u64,
    },
    /// Nothing chosen before the deadline.
    Nothing,
    /// The service's page closed or ran out (or the service forgot it)
    /// before the person was done: it is opened again.
    Expired,
    /// The service refused the sign-in: the account needs a reconnection.
    SignIn,
    /// No folder to copy the photos into: the Choices column's Into is
    /// asked for, before the service's page opens.
    NoInto,
    /// The service could not be reached, or answered an error.
    Unreachable(String),
    /// The photos came back but their import failed.
    Failed(String),
    Cancelled,
}

impl PickStatus {
    /// Writes how a picking from a source of kind `kind` ended to the log
    /// file: `info` for an outcome, `warn` for a failure; nothing for a
    /// step.
    fn log(&self, kind: &str) {
        match self {
            PickStatus::Done { added, known } => {
                log::info!("picking from {kind}: {added} photos imported, {known} already in");
            }
            PickStatus::Nothing => log::info!("picking from {kind}: nothing picked in time"),
            PickStatus::Cancelled => log::info!("picking from {kind}: cancelled"),
            PickStatus::Expired => {
                log::warn!("picking from {kind}: the session expired before it was done");
            }
            PickStatus::SignIn => log::warn!("picking from {kind}: the source refused the sign-in"),
            PickStatus::Unreachable(why) => {
                log::warn!("picking from {kind}: the source could not be reached: {why}");
            }
            PickStatus::Failed(why) => log::warn!("picking from {kind}: not imported: {why}"),
            PickStatus::Starting
            | PickStatus::Browser { .. }
            | PickStatus::Importing { .. }
            | PickStatus::NoInto => {}
        }
    }
}

/// Photos being chosen in the service's page.
pub(crate) struct Picking {
    /// The account the photos are chosen from.
    account: String,
    status: Arc<Mutex<PickStatus>>,
    cancel: helios_connect::Cancel,
}

impl Picking {
    pub(crate) fn status(&self) -> PickStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The connection running, shared with the commands.
pub(crate) type Current = Arc<Mutex<Option<Arc<Session>>>>;
/// The connectors' names, by id.
pub(crate) type Labels = Arc<Mutex<BTreeMap<String, Text>>>;
/// The connectors with an `oauth` step, by id, with their manifest's
/// client id.
pub(crate) type OauthConnectors = Arc<Mutex<BTreeMap<String, Option<String>>>>;
/// The connectors whose sign-in asks the account's address, by id.
pub(crate) type IdentityConnectors = Arc<Mutex<std::collections::BTreeSet<String>>>;

impl Default for Accounts {
    fn default() -> Self {
        Self {
            book: None,
            secrets: Arc::new(MemorySecrets::default()),
            web: None,
            listed: Vec::new(),
            reader: Reader::default(),
            wake: None,
            connecting: None,
            current: Current::default(),
            labels: Labels::default(),
            oauth: OauthConnectors::default(),
            identity: IdentityConnectors::default(),
            picking: None,
            import: None,
            library: None,
            plugins: None,
            consent: None,
            opener: system_opener(),
        }
    }
}

/// A connection running.
pub(crate) struct Connecting {
    pub(crate) session: Arc<Session>,
    /// The connector's name, as shown.
    label: String,
    /// The form's values as typed, by field.
    values: BTreeMap<String, String>,
    /// The form step the values are for.
    form: Option<String>,
}

impl Accounts {
    /// An account's name as last read, with its address once known
    /// ("Google Photos — name@example.com").
    pub(crate) fn label_of(&self, id: &str) -> Option<String> {
        self.listed.iter().find(|a| a.id == id).map(Account::shown)
    }

    /// Whether choosing photos is offered on the account `id`: connected,
    /// its connector picks, and the photos have a library to go to.
    pub(crate) fn picks(&self, id: &str, connectors: &Connectors) -> bool {
        self.import.is_some()
            && self.listed.iter().any(|a| {
                a.id == id
                    && matches!(a.state, AccountState::Connected)
                    && connectors
                        .provider(&a.connector)
                        .is_some_and(|p| p.manifest().caps.picks)
            })
    }

    /// Whether the account signed in before its connector asked who it
    /// is: one reconnection shows its address.
    pub(crate) fn identity_missing(&self, account: &Account) -> bool {
        let asks = self
            .identity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&account.connector);
        account.identity_missing(asks)
    }

    /// The accounts, read again off the interface's thread: as the read
    /// finds them when it answers within [`READ_WAIT`], else as last read
    /// (a vault held by another writer never holds a frame; the late
    /// answer asks for a pass).
    pub(crate) fn listed(&mut self) -> &[Account] {
        let _stall = crate::stall::section("import window: the accounts read");
        if let Some(book) = &self.book {
            self.listed = self.reader.read(book, self.wake.as_ref());
        }
        &self.listed
    }

    /// Starts connecting: a new account of `connector`, or the account
    /// `account` again (from the step that failed when it needs
    /// attention).
    pub(crate) fn connect(
        &mut self,
        connectors: &Connectors,
        connector: &str,
        account: Option<&str>,
        texts: Option<&Texts>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let book = self.book.clone().ok_or("accounts need a library open")?;
        let provider = connectors
            .provider(connector)
            .ok_or_else(|| format!("no source of kind {connector}"))?;
        let manifest = provider.manifest();
        if manifest.connect.is_empty() {
            return Err(format!("{connector} needs no connection"));
        }
        let label = translated(texts, &manifest.label);
        let (account, new) = match account {
            Some(id) => (
                book.account(id)?
                    .ok_or_else(|| format!("no account {id}"))?,
                false,
            ),
            None => (Account::new(connector, &label, &book.accounts()?), true),
        };
        if let Some(c) = self.connecting.take() {
            c.session.cancel();
        }
        let context = Context {
            provider,
            services: connectors.services(),
            book,
            secrets: self.secrets.clone(),
            web: self.web.clone(),
        };
        // The browser opens the moment the session asks for it, from the
        // session's thread: the window's frame is only told to show it.
        let watched: Arc<OnceLock<Weak<Session>>> = Arc::default();
        let look = Arc::new(browser_follower(watched.clone(), self.opener.clone()));
        let then = look.clone();
        let session = Arc::new(Session::start(context, account, new, move || {
            then();
            wake();
        }));
        let _ = watched.set(Arc::downgrade(&session));
        // The page asked before the session was watched.
        look();
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(session.clone());
        self.connecting = Some(Connecting {
            session,
            label,
            values: BTreeMap::new(),
            form: None,
        });
        Ok(())
    }

    /// Signs an account out: its secrets forgotten, "not connected".
    pub(crate) fn sign_out(&mut self, id: &str) -> Result<(), String> {
        let book = self.book.clone().ok_or("accounts need a library open")?;
        match &self.web {
            Some(web) => web.sign_out(book.as_ref(), self.secrets.as_ref(), id)?,
            None => helios_connect::account::sign_out(book.as_ref(), self.secrets.as_ref(), id)?,
        };
        Ok(())
    }

    /// Starts choosing photos under the account `id` ("Choose photos in
    /// Google Photos…"): the service's page opened in the system browser,
    /// the person waited for (cancellable), what was chosen imported. A
    /// sign-in the service refuses leaves the account needing attention,
    /// its Reconnect offered.
    ///
    /// The photos are copied `into` that folder (the Choices column's
    /// Into), else into the library folder; with neither, it stops at
    /// [`PickStatus::NoInto`] before the service is asked.
    pub(crate) fn pick(
        &mut self,
        connectors: &Connectors,
        id: &str,
        into: Option<PathBuf>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let book = self.book.clone().ok_or("accounts need a library open")?;
        let import = self.import.clone().ok_or("no library to import into")?;
        let account = book
            .account(id)?
            .ok_or_else(|| format!("no account {id}"))?;
        let provider = connectors
            .provider(&account.connector)
            .ok_or_else(|| format!("no source of kind {}", account.connector))?;
        let connector = provider
            .create(&serde_json::Value::Null, &connectors.services())
            .map_err(|e| helios_connect::connect::said(&e))?;
        if let Some(p) = self.picking.take() {
            p.cancel.cancel();
        }
        let status = Arc::new(Mutex::new(PickStatus::Starting));
        let cancel = helios_connect::Cancel::new();
        let (st, stop, account_ref) = (status.clone(), cancel.clone(), account.account_ref());
        let opener = self.opener.clone();
        let folder = self.library.clone();
        // The log names the source's kind, not the account (an address).
        let kind = account.connector.clone();
        let set = move |s: PickStatus| {
            *st.lock().unwrap_or_else(PoisonError::into_inner) = s;
            wake();
        };
        std::thread::spawn(move || {
            use helios_connect::ConnectorError;
            // Where the photos go is settled before the service is asked:
            // its page waits for nobody.
            let known = match (&into, &folder) {
                (Some(_), _) | (None, None) => Ok(true),
                (None, Some(f)) => f.get().map(|d| d.is_some()),
            };
            match known {
                Ok(true) => {}
                Ok(false) => return set(PickStatus::NoInto),
                Err(e) => {
                    let end = PickStatus::Failed(e);
                    end.log(&kind);
                    return set(end);
                }
            }
            let deadline = std::time::Instant::now() + PICK_WAIT;
            // Each time a page of its own, opened at once: it waits for
            // nobody, it runs out.
            let opened = |url: &str| {
                opener(url);
                set(PickStatus::Browser {
                    url: url.to_owned(),
                });
            };
            let listed = helios_plugin::source::pick_and_list(
                connector.as_ref(),
                &account_ref,
                deadline,
                &stop,
                &opened,
            );
            let end = match listed {
                Ok(Some((source, items))) => {
                    set(PickStatus::Importing { items: items.len() });
                    let done = import(connector.as_ref(), source, items, into);
                    let _ = connector.end_pick();
                    match done {
                        Ok(Imported { added, known }) => PickStatus::Done { added, known },
                        Err(e) => PickStatus::Failed(e),
                    }
                }
                Ok(None) => PickStatus::Nothing,
                Err(ConnectorError::Expired | ConnectorError::Gone) => PickStatus::Expired,
                Err(ConnectorError::Cancelled) => PickStatus::Cancelled,
                Err(ConnectorError::AuthNeeded) => {
                    let why = "the source refused the sign-in: reconnect";
                    if let Err(e) = helios_connect::account::needs_attention(
                        book.as_ref(),
                        &account_ref.id,
                        why,
                    ) {
                        log::warn!("import accounts: {e}");
                    }
                    PickStatus::SignIn
                }
                Err(e) => PickStatus::Unreachable(helios_connect::connect::said(&e)),
            };
            end.log(&kind);
            set(end);
        });
        self.picking = Some(Picking {
            account: account.id,
            status,
            cancel,
        });
        Ok(())
    }

    /// The connector an action is for: its own, or its account's.
    fn connector_of(&mut self, asked: &Asked) -> Option<String> {
        match asked {
            Asked::Add(connector) => Some(connector.clone()),
            Asked::Reconnect(id) | Asked::Pick(id) => self
                .listed()
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.connector.clone()),
            Asked::Manage | Asked::Select(_) => None,
        }
    }

    /// Whether `asked` waits for the person's consent: its connector's
    /// plugin lacks capabilities it declares, so the consent dialog asks
    /// for them and the action is held until Allow. The plugin's id when
    /// it waits.
    ///
    /// The broker is asked on a thread of its own (a plugin's lock is held
    /// through its handshake): an answer within [`LOOK_WAIT`] is used at
    /// once; a later one holds the action until it comes, and the window's
    /// pass then asks or runs it ([`consent_dialog`]).
    pub(crate) fn ask_consent(&mut self, asked: &Asked) -> Option<String> {
        let _stall = crate::stall::section("import window: a plugin's consent looked up");
        let broker = self.plugins.clone()?;
        let connector = self.connector_of(asked)?;
        let plugin = plugin_serving(&broker, &connector)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (then, wake) = (asked.clone(), self.wake.clone());
        let spawned = std::thread::Builder::new()
            .name("plugin-consent".into())
            .spawn(move || {
                let asking = look_up(&broker, &connector, then);
                let _ = tx.send(asking);
                if let Some(wake) = wake {
                    wake();
                }
            });
        if let Err(e) = spawned {
            log::warn!("plugin consent: {e}");
            return None;
        }
        match rx.recv_timeout(LOOK_WAIT) {
            Ok(Some(asking)) => {
                let plugin = asking.plugin.clone();
                self.consent = Some(Consent::Asking(asking));
                Some(plugin)
            }
            Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.consent = Some(Consent::Looking {
                    then: asked.clone(),
                    answer: rx,
                });
                Some(plugin)
            }
        }
    }

    /// The person allowed the consent asked: its capabilities granted
    /// (and kept, by the broker's keeper) on a thread of their own, as a
    /// grant stops the plugin running and waits for it; the action held
    /// runs once they are ([`consent_dialog`]).
    fn allow(&mut self) {
        let _stall = crate::stall::section("import window: a plugin's capabilities granted");
        let Some(Consent::Asking(asking)) = self.consent.take() else {
            return;
        };
        let Some(broker) = self.plugins.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let wake = self.wake.clone();
        let Asking {
            plugin,
            capabilities,
            then,
            ..
        } = asking;
        let spawned = std::thread::Builder::new()
            .name("plugin-grant".into())
            .spawn(move || {
                let granted = capabilities
                    .iter()
                    .try_for_each(|c| broker.grant(&plugin, &c.name));
                let _ = tx.send(granted);
                if let Some(wake) = wake {
                    wake();
                }
            });
        match spawned {
            Ok(_) => {
                self.consent = Some(Consent::Granting { then, answer: rx });
            }
            Err(e) => log::warn!("plugin consent: {e}"),
        }
    }

    /// The broker's answer to the consent going, when it came: the action
    /// to run (nothing lacking, or granted), else nothing; the dialog asks
    /// when the plugin lacks something.
    fn follow_consent(&mut self) -> Option<Asked> {
        let (ran, next) = match self.consent.take()? {
            Consent::Looking { then, answer } => match answer.try_recv() {
                Ok(Some(asking)) => (None, Some(Consent::Asking(asking))),
                Ok(None) | Err(TryRecvError::Disconnected) => (Some(then), None),
                Err(TryRecvError::Empty) => (None, Some(Consent::Looking { then, answer })),
            },
            Consent::Granting { then, answer } => match answer.try_recv() {
                Ok(Ok(())) => (Some(then), None),
                Ok(Err(e)) => {
                    log::warn!("plugin consent: {e}");
                    (None, None)
                }
                Err(TryRecvError::Disconnected) => (None, None),
                Err(TryRecvError::Empty) => (None, Some(Consent::Granting { then, answer })),
            },
            asking @ Consent::Asking(_) => (None, Some(asking)),
        };
        self.consent = next;
        ran
    }

    /// The import window closed: the connection running gives up (its
    /// session's thread sees the cancel and ends on its own, never
    /// joined) and its dialog is gone; a picking goes on, detached.
    pub(crate) fn window_closed(&mut self) {
        if let Some(c) = &self.connecting {
            c.session.cancel();
        }
        self.consent = None;
        self.close();
    }

    /// The session is no longer shown.
    fn close(&mut self) {
        self.connecting = None;
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// What a session's wake also does: opens the browser on the page its
/// status asks for, once per page.
fn browser_follower(
    watched: Arc<OnceLock<Weak<Session>>>,
    opener: Opener,
) -> impl Fn() + Send + Sync {
    let opened = Mutex::new(None::<String>);
    move || {
        let Some(session) = watched.get().and_then(Weak::upgrade) else {
            return;
        };
        if let Status::Browser { url, .. } = session.status() {
            let mut last = opened.lock().unwrap_or_else(PoisonError::into_inner);
            if last.as_deref() != Some(url.as_str()) {
                *last = Some(url.clone());
                drop(last);
                opener(&url);
            }
        }
    }
}

/// The page the browser was opened on: its link, to copy, and "Open
/// again" in case the browser did not show it.
fn browser_link(ui: &mut egui::Ui, url: &str, opener: &Opener, t: &dyn Fn(Message) -> String) {
    let mut shown = url.to_owned();
    ui.add(egui::TextEdit::singleline(&mut shown).desired_width(f32::INFINITY))
        .on_hover_text(t(Message::new(
            "gui-connect-link-hint",
            "The page's address, to paste in your browser if it did not open.",
        )));
    ui.horizontal(|ui| {
        if ui
            .button(t(Message::new("gui-connect-open-again", "Open again")))
            .clicked()
        {
            opener(url);
        }
        if ui
            .button(t(Message::new("gui-connect-copy-link", "Copy link")))
            .clicked()
        {
            ui.ctx().copy_text(url.to_owned());
        }
    });
}

/// What the consent dialog asks before `then`: the capabilities the
/// plugin serving `connector` lacks; nothing when it lacks none (or is
/// not enabled). Takes the plugin's lock: never on the interface's thread.
fn look_up(broker: &helios_plugin::Broker, connector: &str, then: Asked) -> Option<Asking> {
    let plugin = helios_plugin::source::plugin_of(broker, connector)?;
    let capabilities = broker.missing(&plugin).ok()?;
    if capabilities.is_empty() {
        return None;
    }
    let title = broker
        .manifest(&plugin)
        .map_or_else(|| plugin.clone(), |m| m.title().to_owned());
    Some(Asking {
        plugin,
        title,
        capabilities,
        then,
        declined: false,
    })
}

/// The plugin serving `connector`, from the manifests alone (no plugin's
/// lock taken: whether it is enabled is the broker's answer, asked off
/// the interface's thread).
fn plugin_serving(broker: &helios_plugin::Broker, connector: &str) -> Option<String> {
    broker
        .ids()
        .find(|id| {
            id.rsplit('.').next() == Some(connector)
                && broker.manifest(id).is_some_and(|m| {
                    m.services
                        .iter()
                        .any(|s| s == helios_plugin::source::SERVICE)
                })
        })
        .map(String::from)
}

/// A connector's text in the interface's language.
pub(crate) fn translated(texts: Option<&Texts>, text: &Text) -> String {
    match texts {
        Some(t) => t(Message::new(&text.key, &text.en)),
        None => text.en.clone(),
    }
}

/// An account's state, as the Sources column and Settings > Accounts
/// say it.
pub(crate) fn state_text(state: &AccountState) -> Message {
    match state {
        AccountState::NotConnected => Message::new("gui-account-not-connected", "not connected"),
        AccountState::Connecting { .. } => Message::new("gui-account-connecting", "connecting…"),
        AccountState::Connected => Message::new("gui-account-connected", "connected"),
        AccountState::NeedsAttention { .. } => {
            Message::new("gui-account-needs-attention", "needs attention")
        }
    }
}

/// What the Contents column asked of an account whose photos are chosen
/// in the service's page.
#[derive(Clone, Debug)]
pub(crate) enum ChooseAsked {
    /// An account's action: choose photos, reconnect, another account,
    /// Settings > Accounts.
    Account(Asked),
    /// The Choices column's Into chooser: where the photos are copied.
    Into,
}

/// What the accounts' part of the Sources column, or an account's menu,
/// asked.
#[derive(Clone, Debug)]
pub(crate) enum Asked {
    /// The account clicked: its source selected, the Choices column
    /// showing its choices.
    Select(String),
    /// A new account of this connector ("Use another account…").
    Add(String),
    /// Choose photos in the service's page under this account.
    Pick(String),
    Reconnect(String),
    /// Settings > Accounts, where accounts are renamed and disconnected.
    Manage,
}

/// The name an account is shown by: its saved source's name, else its
/// own; with its address after it, once known ("Google Photos ·
/// me@example.com").
pub(crate) fn header_of(account: &Account, name: Option<&str>) -> String {
    let name = name.unwrap_or(&account.label);
    match account.email() {
        Some(email) if !name.contains(email) => format!("{name} · {email}"),
        _ => name.to_owned(),
    }
}

/// The accounts under "Services", each once: a mark (connected, or
/// needing a reconnection), its name (its saved source's, from `saved`,
/// else its own), and under it its address and, when it is not
/// connected, why in a word. No buttons: a click selects it (the
/// Contents column then shows its one action), and its menu offers
/// "Choose photos…", "Reconnect", "Manage accounts…", then what `items`
/// adds (a saved source's own items, given the account's id). An account
/// is added through the "Add a source…" wizard
/// ([`super::import_wizard`]). Nothing without a library, or when no
/// account exists.
pub(crate) fn accounts_ui(
    ui: &mut egui::Ui,
    accounts: &mut Accounts,
    connectors: &Connectors,
    selected: Option<&str>,
    saved: &BTreeMap<String, String>,
    items: &mut dyn FnMut(&mut egui::Ui, &str),
    t: &dyn Fn(Message) -> String,
) -> Option<Asked> {
    accounts.book.as_ref()?;
    let listed = accounts.listed().to_vec();
    if listed.is_empty() {
        return None;
    }
    let mut asked = None;
    ui.separator();
    ui.strong(t(Message::new("gui-import-window-services", "Services")));
    for account in &listed {
        let attention = matches!(account.state, AccountState::NeedsAttention { .. })
            || accounts.identity_missing(account);
        let (mark, colour) = match account.state {
            _ if attention => ("▲", ui.visuals().warn_fg_color),
            AccountState::Connected => ("●", ui.visuals().text_color()),
            _ => ("○", ui.visuals().weak_text_color()),
        };
        let name = saved
            .get(&account.id)
            .cloned()
            .unwrap_or_else(|| account.label.clone());
        let row = ui
            .horizontal(|ui| {
                ui.label(egui::RichText::new(mark).color(colour));
                let row = ui.selectable_label(selected == Some(account.id.as_str()), &name);
                if row.clicked() {
                    asked = Some(Asked::Select(account.id.clone()));
                }
                row
            })
            .inner;
        // Why in a word, on the row itself: never in a tooltip only.
        let word = match &account.state {
            _ if attention => Some(Message::new(
                "gui-account-needs-reconnecting",
                "needs reconnecting",
            )),
            AccountState::Connected => None,
            state => Some(state_text(state)),
        };
        let address = account.email().filter(|e| !name.contains(e));
        if address.is_some() || word.is_some() {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                if let Some(email) = address {
                    ui.weak(email);
                }
                if let Some(word) = word {
                    ui.label(egui::RichText::new(t(word)).color(colour).small());
                }
            });
        }
        let picks = accounts.picks(&account.id, connectors);
        row.context_menu(|ui| {
            if picks
                && ui
                    .button(t(Message::new("gui-account-choose", "Choose photos…")))
                    .clicked()
            {
                asked = Some(Asked::Pick(account.id.clone()));
                ui.close();
            }
            if ui
                .button(t(Message::new("gui-account-reconnect", "Reconnect")))
                .clicked()
            {
                asked = Some(Asked::Reconnect(account.id.clone()));
                ui.close();
            }
            if ui
                .button(t(Message::new("gui-account-manage", "Manage accounts…")))
                .clicked()
            {
                asked = Some(Asked::Manage);
                ui.close();
            }
            items(ui, &account.id);
        });
    }
    asked
}
/// The consent dialog, while a plugin's capabilities are asked for: the
/// plugin's name and id, each capability it lacks with the reason its
/// manifest gives, Allow and Don't allow. Allow grants them all off the
/// interface's thread; Don't allow says calmly that nothing ran and where
/// to allow them later; Escape closes it, nothing granted. The action held
/// is given back, to run without asking again, once the broker says the
/// plugin lacks nothing or the grants are done.
pub(crate) fn consent_dialog(
    ctx: &egui::Context,
    accounts: &mut Accounts,
    t: &dyn Fn(Message) -> String,
) -> Option<Asked> {
    if let Some(ran) = accounts.follow_consent() {
        ctx.request_repaint();
        return Some(ran);
    }
    let Some(Consent::Asking(consent)) = accounts.consent.as_mut() else {
        return None;
    };
    let (mut allow, mut close) = (false, false);
    let shown = egui::Modal::new(egui::Id::unique("helios.plugin.consent")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.heading(t(Message::new("gui-consent-title", "Allow { $plugin }?")
            .with("plugin", consent.title.clone())));
        ui.weak(t(Message::new("gui-consent-from", "Plugin { $id }")
            .with("id", consent.plugin.clone())));
        ui.add_space(6.0);
        if consent.declined {
            ui.label(t(Message::new(
                "gui-consent-declined",
                "Nothing was changed: { $plugin } does not run without these permissions. You can allow them later in Settings > Plugins.",
            )
            .with("plugin", consent.title.clone())));
            if ui
                .button(t(Message::new("gui-connect-close", "Close")))
                .clicked()
            {
                close = true;
            }
            return;
        }
        ui.label(t(Message::new(
            "gui-consent-asks",
            "{ $plugin } asks for your permission to:",
        )
        .with("plugin", consent.title.clone())));
        for c in &consent.capabilities {
            ui.horizontal_wrapped(|ui| {
                ui.label("•");
                ui.label(&c.reason);
            });
        }
        ui.add_space(6.0);
        ui.weak(t(Message::new(
            "gui-consent-later",
            "Helios remembers your answer; Settings > Plugins changes it.",
        )));
        ui.horizontal(|ui| {
            if ui
                .button(t(Message::new("gui-consent-allow", "Allow")))
                .clicked()
            {
                allow = true;
            }
            if ui
                .button(t(Message::new("gui-consent-deny", "Don't allow")))
                .clicked()
            {
                consent.declined = true;
                ui.ctx().request_repaint();
            }
        });
    });
    // Escape or a click beside it closes it as Don't allow would, without
    // its word: nothing granted, nothing run.
    if !allow && shown.should_close() {
        close = true;
    }
    // What the answer changed is drawn at once: the window repaints only
    // when asked.
    if close || allow {
        ctx.request_repaint();
    }
    if close {
        accounts.consent = None;
    }
    if allow {
        accounts.allow();
    }
    None
}

/// The connection's dialog, while one runs or until its end is seen:
/// the step's form, a spinner, the browser's page, why it failed, or
/// that the account is connected. Closed by Cancel or Close.
pub(crate) fn connect_dialog(
    ctx: &egui::Context,
    accounts: &mut Accounts,
    t: &dyn Fn(Message) -> String,
) {
    let Some(connecting) = accounts.connecting.as_mut() else {
        return;
    };
    let status = connecting.session.status();
    // The browser's page was opened by the session itself (never a web
    // view: services refuse them); the dialog offers it again.
    let opener = accounts.opener.clone();
    let mut close = false;
    egui::Modal::new(egui::Id::unique("helios.import.connect")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading(t(Message::new("gui-connect-title", "Connect { $source }")
            .with("source", connecting.label.clone())));
        ui.add_space(4.0);
        match &status {
            Status::Working { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t(Message::new("gui-connect-working", "Connecting…")));
                });
                if ui
                    .button(t(Message::new("gui-connect-cancel", "Cancel")))
                    .clicked()
                {
                    connecting.session.cancel();
                }
            }
            Status::Form {
                step,
                fields,
                error,
            } => {
                if connecting.form.as_deref() != Some(step.as_str()) {
                    connecting.form = Some(step.clone());
                    connecting.values = fields
                        .iter()
                        .map(|f| (f.id.clone(), f.default.clone()))
                        .collect();
                }
                egui::Grid::new("helios.import.connect.form")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for field in fields {
                            let label =
                                ui.label(t(Message::new(&field.label.key, &field.label.en)));
                            let value = connecting.values.entry(field.id.clone()).or_default();
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .password(field.kind == FieldKind::Secret),
                            )
                            .labelled_by(label.id);
                            ui.end_row();
                            if let Some(help) = &field.help {
                                ui.label("");
                                ui.weak(t(Message::new(&help.key, &help.en)));
                                ui.end_row();
                            }
                        }
                    });
                if let Some(e) = error {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(t(Message::new("gui-connect-next", "Connect")))
                        .clicked()
                    {
                        connecting.session.answer(connecting.values.clone());
                    }
                    if ui
                        .button(t(Message::new("gui-connect-cancel", "Cancel")))
                        .clicked()
                    {
                        connecting.session.cancel();
                    }
                });
            }
            Status::Browser { url, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t(Message::new(
                        "gui-connect-browser",
                        "Sign in in your browser, then come back here.",
                    )));
                });
                browser_link(ui, url, &opener, t);
                if ui
                    .button(t(Message::new("gui-connect-cancel", "Cancel")))
                    .clicked()
                {
                    connecting.session.cancel();
                }
            }
            Status::Connected(account) => {
                ui.label(t(Message::new(
                    "gui-connect-done",
                    "{ $account } is connected.",
                )
                .with("account", account.label.clone())));
                if ui
                    .button(t(Message::new("gui-connect-close", "Close")))
                    .clicked()
                {
                    close = true;
                }
            }
            Status::Failed { why, .. } => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    t(
                        Message::new("gui-connect-failed", "Not connected: { $why }")
                            .with("why", why.clone()),
                    ),
                );
                if ui
                    .button(t(Message::new("gui-connect-close", "Close")))
                    .clicked()
                {
                    close = true;
                }
            }
            Status::Cancelled => close = true,
        }
    });
    if close {
        accounts.close();
    }
}

/// The Contents column for an account whose photos are chosen in the
/// service's own page (`service`, "Google Photos"): one header line, the
/// account with its menu ("Reconnect", "Use another account…", "Manage
/// accounts…"), then where it stands, each state with its one primary
/// action:
///
/// - connected, nothing chosen: "Choose photos in Google Photos…";
/// - the service's page open in the browser: a status line, the account
///   the browser must be signed in to, the page to open again or copy,
///   and Cancel; the rest of the window stays usable;
/// - the photos imported: how many, and "Choose more photos…";
/// - nothing chosen, the page closed: said, and choose again;
/// - access refused, or an account signed in before Helios asked who it
///   is: "Reconnect";
/// - no folder to copy into: the Choices column's Into asked for;
/// - the service unreachable, or the import failed: why, "Try again".
pub(crate) fn choose_ui(
    ui: &mut egui::Ui,
    accounts: &mut Accounts,
    connectors: &Connectors,
    id: &str,
    header: &str,
    service: &str,
    t: &dyn Fn(Message) -> String,
) -> Option<ChooseAsked> {
    let mut asked = None;
    let account = accounts.listed().iter().find(|a| a.id == id).cloned();
    let Some(account) = account else {
        ui.weak(header);
        return None;
    };
    // The header: the account once, its menu at the end of the line.
    ui.horizontal(|ui| {
        ui.strong(header);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The menu's button: a drawn icon (the fonts have no "⋯"),
            // named for screen readers and tests.
            let name = t(Message::new("gui-account-menu", "Account"));
            let button = ui.add(Icon::More.label(&name).sense_click());
            egui::Popup::menu(&button).show(|ui| {
                if ui
                    .button(t(Message::new("gui-account-reconnect", "Reconnect")))
                    .clicked()
                {
                    asked = Some(Asked::Reconnect(id.to_owned()));
                    ui.close();
                }
                if ui
                    .button(t(Message::new(
                        "gui-account-another",
                        "Use another account…",
                    )))
                    .clicked()
                {
                    asked = Some(Asked::Add(account.connector.clone()));
                    ui.close();
                }
                if ui
                    .button(t(Message::new("gui-account-manage", "Manage accounts…")))
                    .clicked()
                {
                    asked = Some(Asked::Manage);
                    ui.close();
                }
            });
        });
    });
    ui.separator();
    ui.add_space(8.0);
    let who = account.email().unwrap_or(&account.label).to_owned();
    let primary = |ui: &mut egui::Ui, text: String| -> bool {
        ui.vertical_centered(|ui| {
            ui.add(egui::Button::new(egui::RichText::new(text).strong()))
                .clicked()
        })
        .inner
    };
    let choose = t(
        Message::new("gui-choose-photos", "Choose photos in { $service }…")
            .with("service", service.to_owned()),
    );
    let reconnect_who = t(
        Message::new("gui-choose-reconnect", "Reconnect { $account }").with("account", who.clone()),
    );
    let refused = |ui: &mut egui::Ui| {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            t(Message::new(
                "gui-choose-refused",
                "▲ { $service } no longer lets Helios read this account.",
            )
            .with("service", service.to_owned())),
        );
        ui.weak(t(Message::new(
            "gui-choose-refused-why",
            "This happens when access was removed in the account's settings at { $service }, or when the access Helios was given ran out.",
        )
        .with("service", service.to_owned())));
        ui.add_space(8.0);
    };
    let reconnect = Some(Asked::Reconnect(id.to_owned()));
    let pick = Some(Asked::Pick(id.to_owned()));
    // Where the account stands comes first: nothing is chosen from an
    // account Helios cannot read.
    match &account.state {
        AccountState::NeedsAttention { .. } => {
            refused(ui);
            if primary(ui, reconnect_who) {
                asked = reconnect;
            }
            return finish(asked);
        }
        AccountState::NotConnected => {
            ui.label(t(Message::new(
                "gui-choose-not-connected",
                "This account is not connected.",
            )));
            ui.add_space(8.0);
            let connect = t(Message::new("gui-choose-connect", "Connect { $service }")
                .with("service", service.to_owned()));
            if primary(ui, connect) {
                asked = reconnect;
            }
            return finish(asked);
        }
        AccountState::Connecting { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t(Message::new(
                    "gui-choose-connecting",
                    "Sign in in your browser, then come back here.",
                )));
            });
            return finish(asked);
        }
        AccountState::Connected => {}
    }
    if accounts.identity_missing(&account) {
        ui.label(t(Message::new(
            "gui-choose-identity",
            "Reconnect once to show which account this is.",
        )));
        ui.add_space(8.0);
        if primary(ui, t(Message::new("gui-account-reconnect", "Reconnect"))) {
            asked = reconnect;
        }
        return finish(asked);
    }
    if !accounts.picks(id, connectors) {
        ui.weak(t(Message::new(
            "gui-import-browse-not-listed",
            "Helios does not list this source's photos here yet.",
        )));
        return finish(asked);
    }
    let opener = accounts.opener.clone();
    let status = accounts
        .picking
        .as_ref()
        .filter(|p| p.account == id)
        .map(Picking::status);
    // The text wraps to the Detail column's width: a line wider than the
    // column would widen its content past its edge.
    let spinning = |ui: &mut egui::Ui, text: String| {
        ui.horizontal_top(|ui| {
            ui.spinner();
            ui.add(egui::Label::new(text).wrap());
        });
    };
    let warned = |ui: &mut egui::Ui, text: String| {
        ui.colored_label(ui.visuals().warn_fg_color, text);
        ui.add_space(8.0);
    };
    let mut cancel = false;
    match status {
        None | Some(PickStatus::Cancelled) => {
            if primary(ui, choose) {
                asked = pick;
            }
            ui.add_space(4.0);
            ui.vertical_centered(|ui| {
                ui.weak(t(Message::new(
                    "gui-choose-about",
                    "{ $service } opens in your browser. Choose your photos, press Done there, then come back here.",
                )
                .with("service", service.to_owned())));
            });
        }
        Some(PickStatus::Starting) => {
            spinning(
                ui,
                t(Message::new("gui-choose-opening", "Opening { $service }…")
                    .with("service", service.to_owned())),
            );
            cancel = ui
                .button(t(Message::new("gui-connect-cancel", "Cancel")))
                .clicked();
        }
        Some(PickStatus::Browser { url }) => {
            spinning(
                ui,
                t(Message::new(
                    "gui-choose-waiting",
                    "Choose your photos in the browser, press Done there, then come back here.",
                )),
            );
            ui.weak(t(Message::new(
                "gui-choose-signed-in",
                "{ $service } shows the photos of the account your browser is signed in to: { $account }.",
            )
            .with("service", service.to_owned())
            .with("account", who)));
            ui.horizontal_wrapped(|ui| {
                ui.weak(t(Message::new(
                    "gui-choose-not-open",
                    "The page did not open?",
                )));
                if ui
                    .link(t(Message::new("gui-choose-open-again", "Open it again")))
                    .clicked()
                {
                    opener(&url);
                }
                ui.weak("·");
                if ui
                    .link(t(Message::new("gui-choose-copy-link", "Copy the link")))
                    .on_hover_text(url.clone())
                    .clicked()
                {
                    ui.ctx().copy_text(url.clone());
                }
            });
            cancel = ui
                .button(t(Message::new("gui-connect-cancel", "Cancel")))
                .clicked();
        }
        Some(PickStatus::Importing { items }) => spinning(
            ui,
            t(
                Message::new("gui-choose-importing", "Importing { $count } photos…")
                    .with("count", items as i64),
            ),
        ),
        Some(PickStatus::Done { added, known }) => {
            ui.label(t(Message::new(
                "gui-choose-done",
                "{ $count } photos imported.",
            )
            .with("count", added as i64)));
            if known > 0 {
                ui.weak(t(Message::new(
                    "gui-choose-known",
                    "{ $count } were already in your Library.",
                )
                .with("count", known as i64)));
            }
            ui.add_space(8.0);
            if primary(
                ui,
                t(Message::new("gui-choose-more", "Choose more photos…")),
            ) {
                asked = pick;
            }
        }
        Some(PickStatus::Nothing) => {
            ui.label(t(Message::new(
                "gui-choose-nothing",
                "No photos were chosen.",
            )));
            ui.add_space(8.0);
            if primary(ui, choose) {
                asked = pick;
            }
        }
        Some(PickStatus::Expired) => {
            warned(
                ui,
                t(Message::new(
                    "gui-choose-closed",
                    "The { $service } page has closed. Open it again to choose your photos.",
                )
                .with("service", service.to_owned())),
            );
            if primary(ui, choose) {
                asked = pick;
            }
        }
        Some(PickStatus::SignIn) => {
            refused(ui);
            if primary(ui, reconnect_who) {
                asked = reconnect;
            }
        }
        Some(PickStatus::NoInto) => {
            ui.label(t(Message::new(
                "gui-choose-no-into",
                "Choose where the photos are copied first: Into, in the Choices column.",
            )));
            ui.add_space(8.0);
            if primary(ui, t(Message::new("gui-choose-into", "Choose the folder…"))) {
                return Some(ChooseAsked::Into);
            }
        }
        Some(PickStatus::Unreachable(why)) => {
            warned(
                ui,
                t(Message::new(
                    "gui-choose-unreachable",
                    "Could not reach { $service }: { $why }",
                )
                .with("service", service.to_owned())
                .with("why", why)),
            );
            if primary(ui, t(Message::new("gui-choose-try-again", "Try again"))) {
                asked = pick;
            }
        }
        Some(PickStatus::Failed(why)) => {
            warned(
                ui,
                t(Message::new(
                    "gui-choose-failed",
                    "The photos were not imported: { $why }",
                )
                .with("why", why)),
            );
            if primary(ui, t(Message::new("gui-choose-try-again", "Try again"))) {
                asked = pick;
            }
        }
    }
    if let Some(p) = accounts.picking.as_ref().filter(|p| p.account == id) {
        if cancel {
            p.cancel.cancel();
        }
        // Cancelled: back to the account's primary action.
        if p.status() == PickStatus::Cancelled {
            accounts.picking = None;
            ui.ctx().request_repaint();
        }
    }
    finish(asked)
}

fn finish(asked: Option<Asked>) -> Option<ChooseAsked> {
    asked.map(ChooseAsked::Account)
}

impl Accounts {
    /// Into was chosen while it was asked for: back to the account's
    /// primary action, the photos chosen with it.
    pub(crate) fn on_into_chosen(&mut self) {
        if self
            .picking
            .as_ref()
            .is_some_and(|p| p.status() == PickStatus::NoInto)
        {
            self.picking = None;
        }
    }
}
