//! A service's albums in the import window (`docs/design/import-window.md`,
//! `docs/design/import-connectors.md` 3.9): an account whose connector
//! lists its albums (`caps.collections`) shows them under its item in the
//! Sources column's Services panel, as Lightroom's source panel shows a
//! folder's subfolders under it.
//!
//! - **Listed page by page.** The first page is read when the account is
//!   selected, on a thread of its own; "More albums" reads the next.
//! - **One source at a time.** Clicking an album selects that album as
//!   the source: highlighted alone, its photos listed (every page) and
//!   shown in the middle as a card's are ("in your Library" marked), the
//!   account's Detail on the right. Clicking the account again leaves it.
//! - **Imported as chosen photos are**: the footer's Import fetches the
//!   ones ticked through the connector that listed them, with the Detail
//!   column's choices ([`super::import_chosen`]); the import history
//!   names the album (its source's label).
//!
//! A connector that declares no albums (Google Photos, whose API keeps a
//! library's albums to itself) shows none.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use helios_connect::{
    AccountRef, Cancel, Collection, Connector, ConnectorError, Cursor, PAGE, SourceRef,
};

use super::import_accounts::{Batch, KnownRemote, Wake};

/// The albums asked in one page.
const ALBUM_PAGE: usize = 50;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An account's albums as read so far.
#[derive(Clone, Debug, Default)]
pub(crate) struct Listed {
    pub(crate) albums: Vec<Collection>,
    /// Where the next page starts; `None` once every album was read.
    next: Option<Cursor>,
    /// A page is being read.
    pub(crate) reading: bool,
    /// The first page was asked.
    started: bool,
    /// Why the last page could not be read.
    pub(crate) failed: Option<String>,
}

impl Listed {
    /// More albums are there to read.
    pub(crate) fn more(&self) -> bool {
        (self.next.is_some() || self.failed.is_some()) && !self.reading
    }
}

/// One account's albums and the connector reading them.
struct AlbumList {
    connector: Arc<dyn Connector>,
    account: AccountRef,
    listed: Arc<Mutex<Listed>>,
}

/// The photos of the album selected.
#[derive(Clone)]
pub(crate) enum Photos {
    Reading,
    Listed(Arc<Batch>),
    Failed(String),
}

/// The album selected as the source, its photos being listed.
struct Opened {
    account: String,
    source: SourceRef,
    photos: Arc<Mutex<Photos>>,
    cancel: Cancel,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The albums of the accounts whose connector lists them, and the album
/// selected.
#[derive(Default)]
pub(crate) struct Albums {
    lists: BTreeMap<String, AlbumList>,
    opened: Option<Opened>,
}

/// A connector's error, in a few words.
fn said(e: &ConnectorError) -> String {
    helios_connect::connect::said(e)
}

impl Albums {
    /// The albums of `account` read so far; `None` before they are asked.
    pub(crate) fn of(&self, account: &str) -> Option<Listed> {
        self.lists.get(account).map(|l| lock(&l.listed).clone())
    }

    /// Reads the first page of `account`'s albums through `connector`,
    /// once.
    pub(crate) fn start(
        &mut self,
        account: AccountRef,
        connector: Arc<dyn Connector>,
        wake: &Wake,
    ) {
        if self.lists.contains_key(&account.id) {
            return;
        }
        let id = account.id.clone();
        self.lists.insert(
            id.clone(),
            AlbumList {
                connector,
                account,
                listed: Arc::default(),
            },
        );
        self.more(&id, wake);
    }

    /// Reads the next page of `account`'s albums, on a thread of its own,
    /// unless one is being read or every album was.
    pub(crate) fn more(&mut self, account: &str, wake: &Wake) {
        let Some(list) = self.lists.get(account) else {
            return;
        };
        let from = {
            let mut l = lock(&list.listed);
            if l.reading || (l.started && l.next.is_none() && l.failed.is_none()) {
                return;
            }
            l.reading = true;
            l.started = true;
            l.failed = None;
            l.next.clone()
        };
        let (connector, account, listed, wake) = (
            list.connector.clone(),
            list.account.clone(),
            list.listed.clone(),
            wake.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("import-albums".into())
            .spawn(move || {
                let page =
                    connector.collections(&account, from.as_ref(), ALBUM_PAGE, &Cancel::new());
                {
                    let mut l = lock(&listed);
                    l.reading = false;
                    match page {
                        Ok(p) => {
                            l.albums.extend(p.collections);
                            l.next = p.next;
                        }
                        Err(e) => {
                            // The log names the source's kind, never the
                            // account (an address).
                            log::warn!("albums of {}: {e}", account.connector);
                            l.failed = Some(said(&e));
                        }
                    }
                }
                wake();
            });
        if let Err(e) = spawned {
            log::warn!("import albums: {e}");
            lock(&list.listed).reading = false;
        }
    }

    /// The album of `account` selected as the source, if any.
    pub(crate) fn selected(&self, account: &str) -> Option<&SourceRef> {
        self.opened
            .as_ref()
            .filter(|o| o.account == account)
            .map(|o| &o.source)
    }

    /// Selects the album `source` of `account` as the source: its photos
    /// listed, every page, on a thread of their own, each marked when the
    /// Library holds it (`known`).
    pub(crate) fn open(
        &mut self,
        account: &str,
        source: SourceRef,
        known: Option<KnownRemote>,
        wake: &Wake,
    ) {
        if self.selected(account) == Some(&source) {
            return;
        }
        let Some(list) = self.lists.get(account) else {
            return;
        };
        let photos = Arc::new(Mutex::new(Photos::Reading));
        let cancel = Cancel::new();
        let (connector, src, out, stop, wake) = (
            list.connector.clone(),
            source.clone(),
            photos.clone(),
            cancel.clone(),
            wake.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("import-album".into())
            .spawn(move || {
                let mut items = Vec::new();
                let mut from = None;
                let end = loop {
                    match connector.list(&src, from.as_ref(), PAGE, &stop) {
                        Ok(page) => {
                            items.extend(page.items);
                            match page.next {
                                Some(next) if !page.complete => from = Some(next),
                                _ => break None,
                            }
                        }
                        Err(e) => break Some(e),
                    }
                };
                if stop.is_cancelled() {
                    return;
                }
                let photos = match end {
                    Some(e) => {
                        log::warn!("an album of {}: {e}", src.connector);
                        Photos::Failed(said(&e))
                    }
                    None => {
                        let known = known
                            .as_ref()
                            .map_or_else(|| vec![false; items.len()], |k| k(&src, &items));
                        Photos::Listed(Arc::new(Batch {
                            connector,
                            source: src,
                            items,
                            known,
                        }))
                    }
                };
                *lock(&out) = photos;
                wake();
            });
        if let Err(e) = spawned {
            log::warn!("import album: {e}");
            return;
        }
        self.opened = Some(Opened {
            account: account.to_owned(),
            source,
            photos,
            cancel,
        });
    }

    /// No album selected.
    pub(crate) fn close(&mut self) {
        self.opened = None;
    }

    /// The album of `account` selected, and its photos as listed so far.
    pub(crate) fn photos(&self, account: &str) -> Option<(SourceRef, Photos)> {
        self.opened
            .as_ref()
            .filter(|o| o.account == account)
            .map(|o| (o.source.clone(), lock(&o.photos).clone()))
    }
}

impl super::import_window::ImportWindow {
    /// Whether the account `id` lists its albums here: connected, and its
    /// connector declares albums (`caps.collections`).
    pub(super) fn lists_albums(&mut self, id: &str) -> bool {
        let connectors = &self.connectors;
        self.accounts.listed().iter().any(|a| {
            a.id == id
                && matches!(a.state, helios_connect::account::AccountState::Connected)
                && connectors
                    .provider(&a.connector)
                    .is_some_and(|p| p.manifest().caps.collections)
        })
    }

    /// Starts reading the albums of the account `id`, once.
    pub(super) fn start_albums(&mut self, id: &str) {
        if self.albums.of(id).is_some() {
            return;
        }
        let Some(account) = self.accounts.listed().iter().find(|a| a.id == id).cloned() else {
            return;
        };
        let Some(provider) = self.connectors.provider(&account.connector) else {
            return;
        };
        let connector = match provider.create(&serde_json::Value::Null, &self.connectors.services())
        {
            Ok(c) => Arc::<dyn Connector>::from(c),
            Err(e) => {
                log::warn!("albums of {}: {e}", account.connector);
                return;
            }
        };
        let wake: Wake = Arc::new(self.waker());
        self.albums.start(account.account_ref(), connector, &wake);
    }

    /// Reads the next page of the account's albums.
    pub(super) fn more_albums(&mut self, id: &str) {
        let wake: Wake = Arc::new(self.waker());
        self.albums.more(id, &wake);
    }

    /// Selects the album `source` of the account `id` as the source,
    /// alone: the middle zone lists its photos, the right one shows the
    /// account's detail.
    pub(super) fn select_album(&mut self, id: &str, source: SourceRef) {
        self.select(super::import_sources::SourceKey::Account(id.to_owned()));
        let wake: Wake = Arc::new(self.waker());
        let known = self.accounts.known.clone();
        self.albums.open(id, source, known, &wake);
    }

    /// The album selected as the source, with its account and its photos
    /// as listed so far.
    pub(super) fn album_shown(&self) -> Option<(String, SourceRef, Photos)> {
        let account = self.sources.account.as_ref()?;
        let (source, photos) = self.albums.photos(account)?;
        Some((account.clone(), source, photos))
    }
}
