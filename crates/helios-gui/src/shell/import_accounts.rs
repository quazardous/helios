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
//! A page for the browser (an OAuth consent, a picking) is opened the
//! moment the step asks for it, from the step's own thread, through the
//! platform's opener ([`system_opener`]) on a thread of its own: never
//! at the end of a frame, so it does not wait for the window's next
//! frame or hold the window while the opener runs. The dialog shows the
//! link, to copy, and "Open again".
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
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

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
    /// The picking running ("Pick photos…"), shown in the window's dialog.
    pub(crate) picking: Option<Picking>,
    /// How picked items are imported: `helios_library::fetch::import_remote`
    /// over the library, set by the desktop; none: "Pick photos…" is not
    /// offered.
    pub(crate) import: Option<ImportRemote>,
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
pub type ImportRemote = Arc<
    dyn Fn(
            &dyn helios_connect::Connector,
            helios_connect::SourceRef,
            Vec<helios_connect::Item>,
        ) -> Result<u64, String>
        + Send
        + Sync,
>;

/// How long a picking waits for the person at most.
const PICK_WAIT: std::time::Duration = std::time::Duration::from_mins(30);

/// Where a picking stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickStatus {
    /// Starting the picking at the service.
    Starting,
    /// Waiting for the person, in the browser opened on `url`.
    Browser {
        url: String,
    },
    /// Downloading and importing `items` photos.
    Importing {
        items: usize,
    },
    /// Done: `added` photos added.
    Done {
        added: u64,
    },
    /// Nothing picked before the deadline.
    Nothing,
    /// The picking session expired (or the service forgot it) before
    /// the person was done: a new one is needed.
    Expired,
    /// The service refused the sign-in: the account needs a reconnection.
    SignIn,
    Failed(String),
    Cancelled,
}

/// A picking running.
pub(crate) struct Picking {
    /// The account picked from.
    account: String,
    label: String,
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
            picking: None,
            import: None,
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
    /// An account's name as last read.
    pub(crate) fn label_of(&self, id: &str) -> Option<String> {
        self.listed
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.label.clone())
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

    /// Starts picking under the account `id` ("Pick photos…"): the
    /// connector's picking page opened in the system browser by the
    /// dialog, the person waited for (cancellable), what was picked
    /// imported. A sign-in the service refuses leaves the account needing
    /// attention, its Reconnect offered.
    pub(crate) fn pick(
        &mut self,
        connectors: &Connectors,
        id: &str,
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
        let set = move |s: PickStatus| {
            *st.lock().unwrap_or_else(PoisonError::into_inner) = s;
            wake();
        };
        std::thread::spawn(move || {
            use helios_connect::ConnectorError;
            let deadline = std::time::Instant::now() + PICK_WAIT;
            // Every picking is a session of its own, its page opened at
            // once: a session waits for nobody, it expires.
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
                    let done = import(connector.as_ref(), source, items);
                    let _ = connector.end_pick();
                    match done {
                        Ok(added) => PickStatus::Done { added },
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
                Err(e) => PickStatus::Failed(helios_connect::connect::said(&e)),
            };
            set(end);
        });
        self.picking = Some(Picking {
            account: account.id,
            label: account.label,
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
            Asked::SignOut(_) | Asked::Select(_) => None,
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
                let asking = (|| {
                    let plugin = helios_plugin::source::plugin_of(&broker, &connector)?;
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
                })();
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

/// What the accounts' part of the Sources column asked.
#[derive(Clone, Debug)]
pub(crate) enum Asked {
    /// The account clicked: its source selected, the Choices column
    /// showing its choices.
    Select(String),
    Add(String),
    Pick(String),
    Reconnect(String),
    SignOut(String),
}

/// The accounts under their heading, each with its state and, in its
/// menu, "Reconnect" and "Sign out". An account is added through the
/// "Add a source…" wizard ([`super::import_wizard`]). Nothing without a
/// library, or when no account exists.
pub(crate) fn accounts_ui(
    ui: &mut egui::Ui,
    accounts: &mut Accounts,
    connectors: &Connectors,
    selected: Option<&str>,
    t: &dyn Fn(Message) -> String,
) -> Option<Asked> {
    accounts.book.as_ref()?;
    let listed = accounts.listed().to_vec();
    if listed.is_empty() {
        return None;
    }
    let mut asked = None;
    ui.separator();
    ui.strong(t(Message::new("gui-import-window-accounts", "Accounts")));
    for account in &listed {
        let state = t(state_text(&account.state));
        let colour = match account.state {
            AccountState::Connected => ui.visuals().text_color(),
            AccountState::NeedsAttention { .. } => ui.visuals().warn_fg_color,
            _ => ui.visuals().weak_text_color(),
        };
        let why = match &account.state {
            AccountState::NeedsAttention { why, .. } => Some(why.clone()),
            _ => None,
        };
        let row = ui
            .horizontal(|ui| {
                if ui
                    .selectable_label(selected == Some(account.id.as_str()), &account.label)
                    .clicked()
                {
                    asked = Some(Asked::Select(account.id.clone()));
                }
                ui.label(egui::RichText::new(&state).color(colour).small());
            })
            .response;
        let row = row.interact(egui::Sense::click());
        let row = match why {
            Some(why) => row.on_hover_text(why),
            None => row,
        };
        row.context_menu(|ui| {
            if ui
                .button(t(Message::new("gui-account-reconnect", "Reconnect")))
                .clicked()
            {
                asked = Some(Asked::Reconnect(account.id.clone()));
                ui.close();
            }
            if ui
                .button(t(Message::new("gui-account-sign-out", "Sign out")))
                .clicked()
            {
                asked = Some(Asked::SignOut(account.id.clone()));
                ui.close();
            }
        });
        if matches!(account.state, AccountState::NeedsAttention { .. })
            && ui
                .small_button(t(Message::new("gui-account-reconnect", "Reconnect")))
                .clicked()
        {
            asked = Some(Asked::Reconnect(account.id.clone()));
        }
        if matches!(account.state, AccountState::Connected)
            && accounts.import.is_some()
            && connectors
                .provider(&account.connector)
                .is_some_and(|p| p.manifest().caps.picks)
            && ui
                .small_button(t(Message::new("gui-account-pick", "Pick photos…")))
                .clicked()
        {
            asked = Some(Asked::Pick(account.id.clone()));
        }
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

/// The picking's dialog, while one runs or until its end is seen: the
/// account picked from (the browser must be signed in to it), the
/// picking page (opened by the picking itself, never in a web view) to
/// open again or copy, a spinner while the person picks or the photos
/// come in, then what was imported, or why not. Cancel stops the wait;
/// "Start again" (a session expired, or a page the browser refused)
/// asks for a new picking, given back to run.
pub(crate) fn pick_dialog(
    ctx: &egui::Context,
    accounts: &mut Accounts,
    t: &dyn Fn(Message) -> String,
) -> Option<Asked> {
    let opener = accounts.opener.clone();
    let picking = accounts.picking.as_mut()?;
    let status = picking.status();
    let (mut close, mut again) = (false, false);
    egui::Modal::new(egui::Id::unique("helios.import.pick")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading(t(Message::new(
            "gui-pick-title",
            "Pick photos in { $source }",
        )
        .with("source", picking.label.clone())));
        ui.add_space(4.0);
        let working = |ui: &mut egui::Ui, text: String| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(text);
            });
        };
        let ended = |ui: &mut egui::Ui, text: String, warn: bool| {
            if warn {
                ui.colored_label(ui.visuals().warn_fg_color, text);
            } else {
                ui.label(text);
            }
        };
        match &status {
            PickStatus::Starting => working(
                ui,
                t(Message::new("gui-pick-starting", "Opening the picking…")),
            ),
            PickStatus::Browser { url } => {
                working(
                    ui,
                    t(Message::new(
                        "gui-pick-browser",
                        "Pick your photos in your browser, press Done, then come back here.",
                    )),
                );
                ui.weak(t(Message::new(
                    "gui-pick-account",
                    "Your browser must be signed in to the account connected as { $account }; if the page says it cannot open, sign in to that account and start again.",
                )
                .with("account", picking.label.clone())));
                browser_link(ui, url, &opener, t);
            }
            PickStatus::Importing { items } => working(
                ui,
                t(
                    Message::new("gui-pick-importing", "Importing { $count } photos…")
                        .with("count", *items as i64),
                ),
            ),
            PickStatus::Done { added } => ended(
                ui,
                t(Message::new("gui-pick-done", "{ $count } photos imported.")
                    .with("count", *added as i64)),
                false,
            ),
            PickStatus::Nothing => ended(
                ui,
                t(Message::new(
                    "gui-pick-nothing",
                    "Nothing was picked in time.",
                )),
                false,
            ),
            PickStatus::Expired => ended(
                ui,
                t(Message::new(
                    "gui-pick-expired",
                    "The picking session expired, start again.",
                )),
                true,
            ),
            PickStatus::SignIn => ended(
                ui,
                t(Message::new(
                    "gui-pick-sign-in",
                    "The source refused the sign-in: reconnect the account.",
                )),
                true,
            ),
            PickStatus::Failed(why) => ended(
                ui,
                t(Message::new("gui-pick-failed", "Not imported: { $why }")
                    .with("why", why.clone())),
                true,
            ),
            PickStatus::Cancelled => close = true,
        }
        let running = matches!(
            status,
            PickStatus::Starting | PickStatus::Browser { .. } | PickStatus::Importing { .. }
        );
        let restart = matches!(
            status,
            PickStatus::Browser { .. } | PickStatus::Expired | PickStatus::Nothing
        );
        ui.horizontal(|ui| {
            if restart
                && ui
                    .button(t(Message::new("gui-pick-again", "Start again")))
                    .clicked()
            {
                again = true;
            }
            if running {
                if ui
                    .button(t(Message::new("gui-connect-cancel", "Cancel")))
                    .clicked()
                {
                    picking.cancel.cancel();
                }
            } else if ui
                .button(t(Message::new("gui-connect-close", "Close")))
                .clicked()
            {
                close = true;
            }
        });
    });
    if again {
        // A new session, never the old one: the picking running gives up.
        let account = picking.account.clone();
        picking.cancel.cancel();
        accounts.picking = None;
        ctx.request_repaint();
        return Some(Asked::Pick(account));
    }
    if close {
        accounts.picking = None;
    }
    None
}
