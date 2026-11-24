//! A picking's pages listed while the grid already shows the first ones
//! (`docs/design/import-slow-sources.md`, slice 5): the photos chosen in
//! a service's page join the grid page by page, and Import takes those
//! listed so far.
//!
//! The service forgets a picking once it is ended ([`Connector::end_pick`]),
//! and an import ends it when its photos are in. An import of the first
//! pages must not cut the listing of the rest, nor the import of pages
//! listed after it: [`HeldPick`] holds the end until the listing is over
//! and no page of the picking is left among the photos chosen, imported
//! or given up.

use std::sync::{Arc, Mutex, PoisonError};

use helios_connect::{
    AccountRef, Cancel, Collections, Connector, ConnectorError, ConnectorInfo, Cursor, FetchSink,
    Fetched, ItemRef, Page, Pick, Probed, SourceRef,
};

use super::import_accounts::Chosen;

/// Where a held picking stands.
#[derive(Default)]
struct Held {
    /// Its pages are still being listed.
    listing: bool,
    /// An end was asked while it could not end yet.
    wanted: bool,
    /// The service was told to forget it.
    ended: bool,
}

/// A picking's connector whose end waits for its listing and for every
/// page of it still chosen: the batches of a picking listed page by page
/// share it.
pub(crate) struct HeldPick {
    inner: Arc<dyn Connector>,
    chosen: Chosen,
    state: Mutex<Held>,
}

impl HeldPick {
    /// `inner` held while its pages are listed, the photos chosen in
    /// `chosen`.
    pub(crate) fn new(inner: Arc<dyn Connector>, chosen: Chosen) -> Self {
        Self {
            inner,
            chosen,
            state: Mutex::new(Held {
                listing: true,
                ..Held::default()
            }),
        }
    }

    /// The listing is over (complete, failed or cancelled): an end asked
    /// meanwhile runs now, unless pages of it are still chosen.
    pub(crate) fn listed(&self) {
        let end = {
            let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            s.listing = false;
            s.wanted && self.may_end(&mut s)
        };
        if end {
            self.end_now();
        }
    }

    /// Whether some page of this picking is still among the photos
    /// chosen (not imported, not given up).
    fn still_chosen(&self) -> bool {
        let me = std::ptr::from_ref(self).cast::<()>();
        self.chosen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .flatten()
            .any(|b| Arc::as_ptr(&b.connector).cast::<()>() == me)
    }

    /// Whether the picking ends now: listed, nothing of it chosen, not
    /// ended yet; marked ended when it does.
    fn may_end(&self, s: &mut Held) -> bool {
        if s.listing || s.ended || self.still_chosen() {
            return false;
        }
        s.ended = true;
        true
    }

    fn end_now(&self) {
        if let Err(e) = self.inner.end_pick() {
            log::debug!("import accounts: end of a picking: {e:?}");
        }
    }
}

impl Connector for HeldPick {
    fn describe(&self) -> ConnectorInfo {
        self.inner.describe()
    }

    fn sources(&self, account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        self.inner.sources(account)
    }

    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        self.inner.list(source, from, page, cancel)
    }

    fn changes(
        &self,
        source: &SourceRef,
        since: &Cursor,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        self.inner.changes(source, since, cancel)
    }

    fn thumbnail(&self, item: &ItemRef, max_side: u32) -> Result<Option<Vec<u8>>, ConnectorError> {
        self.inner.thumbnail(item, max_side)
    }

    fn probe(
        &self,
        account: Option<&AccountRef>,
        cancel: &Cancel,
    ) -> Result<Probed, ConnectorError> {
        self.inner.probe(account, cancel)
    }

    fn collections(
        &self,
        account: &AccountRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Collections, ConnectorError> {
        self.inner.collections(account, from, page, cancel)
    }

    fn pick(&self, account: &AccountRef, cancel: &Cancel) -> Result<Pick, ConnectorError> {
        self.inner.pick(account, cancel)
    }

    fn picked(
        &self,
        deadline: std::time::Instant,
        cancel: &Cancel,
    ) -> Result<bool, ConnectorError> {
        self.inner.picked(deadline, cancel)
    }

    /// Ends the picking once its listing is over and none of its pages
    /// is still chosen; else the end waits for the last of them.
    fn end_pick(&self) -> Result<(), ConnectorError> {
        let end = {
            let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            s.wanted = true;
            self.may_end(&mut s)
        };
        if end {
            self.end_now();
        }
        Ok(())
    }

    fn access(&self, item: &ItemRef) -> helios_connect::Access {
        self.inner.access(item)
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        self.inner.fetch(item, offset, sink, cancel)
    }
}
