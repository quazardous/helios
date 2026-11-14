//! A connector for tests that lists an account's albums
//! ([`Connector::collections`]), page by page, and each album's photos
//! ([`Connector::list`]), page by page too: its service answers at most
//! `per_page` entries whatever is asked, as a real service caps its pages.
//! Its photos are fetched, a few bytes each. The import window's tests
//! use it for the `collections` browser.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    Access, AccountRef, Cancel, Caps, Collection, Collections, ConnectStep, Connector,
    ConnectorError, ConnectorInfo, ConnectorManifest, ConnectorProvider, ConnectorServices, Cursor,
    FetchSink, Fetched, INTERFACE, Item, ItemId, ItemKind, ItemRef, Page, Probed, Settings,
    SourceRef, StepKind, Text,
};

/// Its id.
pub const ID: &str = "test-albums";

/// The provider: its albums, each a name and a number of photos, and the
/// most entries one answer holds.
#[derive(Clone)]
pub struct AlbumsProvider {
    pub albums: Arc<Vec<(String, usize)>>,
    pub per_page: usize,
    /// How many album pages were asked, for tests.
    pub album_pages: Arc<AtomicUsize>,
}

impl AlbumsProvider {
    pub fn new(albums: &[(&str, usize)], per_page: usize) -> Self {
        Self {
            albums: Arc::new(albums.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect()),
            per_page: per_page.max(1),
            album_pages: Arc::default(),
        }
    }
}

fn label() -> Text {
    Text::new("connect-test-albums", "Test albums")
}

impl ConnectorProvider for AlbumsProvider {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: ID.into(),
            version: "1.0.0".into(),
            label: label(),
            description: Text::new("connect-test-albums-about", "A service with albums"),
            caps: Caps {
                accounts: true,
                sizes: true,
                collections: true,
                originals: crate::Originals::Rendered,
                ..Caps::default()
            },
            needs: crate::Needs {
                account: true,
                network: true,
                usb: false,
            },
            settings: Settings::Null,
            platforms: Vec::new(),
            interface: INTERFACE,
            connect: vec![ConnectStep {
                id: "probe".into(),
                kind: StepKind::Test,
            }],
            browsers: Vec::new(),
            setup: None,
        }
    }

    fn create(
        &self,
        _settings: &Settings,
        _services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(Albums(self.clone())))
    }
}

/// The connector.
pub struct Albums(AlbumsProvider);

/// An album's source id: its account and its place in the list.
fn album_id(account: &str, index: usize) -> String {
    format!("{account}/album-{index}")
}

/// The place an album's source id names.
fn album_of(source: &SourceRef) -> Option<usize> {
    source
        .id
        .rsplit_once("/album-")
        .and_then(|(_, n)| n.parse().ok())
}

/// Where a page starts: its cursor's offset.
fn offset(from: Option<&Cursor>) -> Result<usize, ConnectorError> {
    from.map_or(Ok(0), |c| {
        c.0.parse()
            .map_err(|_| ConnectorError::Fatal(format!("bad cursor {}", c.0)))
    })
}

impl Albums {
    /// The entries one answer holds: what was asked, at most the
    /// service's own page.
    fn take(&self, page: usize) -> usize {
        page.clamp(1, self.0.per_page)
    }
}

impl Connector for Albums {
    fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: ID.into(),
            label: label(),
            caps: self.0.manifest().caps,
        }
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(Vec::new())
    }

    fn probe(
        &self,
        account: Option<&AccountRef>,
        _cancel: &Cancel,
    ) -> Result<Probed, ConnectorError> {
        Ok(Probed {
            label: account.map(|a| a.id.clone()),
        })
    }

    fn collections(
        &self,
        account: &AccountRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Collections, ConnectorError> {
        if cancel.is_cancelled() {
            return Err(ConnectorError::Cancelled);
        }
        self.0.album_pages.fetch_add(1, Ordering::SeqCst);
        let start = offset(from)?;
        let end = (start + self.take(page)).min(self.0.albums.len());
        let collections = self.0.albums[start.min(end)..end]
            .iter()
            .enumerate()
            .map(|(i, (name, count))| Collection {
                source: SourceRef {
                    connector: ID.into(),
                    id: album_id(&account.id, start + i),
                    label: name.clone(),
                    root: None,
                    capacity: None,
                },
                count: u64::try_from(*count).ok(),
            })
            .collect();
        Ok(Collections {
            collections,
            next: (end < self.0.albums.len()).then(|| Cursor(end.to_string())),
        })
    }

    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        if cancel.is_cancelled() {
            return Err(ConnectorError::Cancelled);
        }
        let album = album_of(source).ok_or(ConnectorError::NotFound)?;
        let count = self.0.albums.get(album).ok_or(ConnectorError::NotFound)?.1;
        let start = offset(from)?;
        let end = (start + self.take(page)).min(count);
        let items = (start.min(end)..end)
            .map(|n| Item {
                id: ItemId(format!("a{album}-p{n}")),
                path: format!("IMG_{album}{n:03}.JPG"),
                size: Some(6),
                modified: None,
                taken: Some(1_700_000_000 + i64::try_from(n).unwrap_or(0) * 60),
                kind: ItemKind::Photo,
                mime: Some("image/jpeg".into()),
                metadata: std::collections::BTreeMap::new(),
                claim: None,
                revision: None,
                group: None,
            })
            .collect();
        let next = (end < count).then(|| Cursor(end.to_string()));
        Ok(Page {
            items,
            complete: next.is_none(),
            next,
        })
    }

    fn access(&self, _item: &ItemRef) -> Access {
        Access::Fetch
    }

    fn fetch(
        &self,
        _item: &ItemRef,
        _offset: u64,
        sink: &mut dyn FetchSink,
        _cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        let bytes = b"photo!";
        sink.accept(bytes)
            .map_err(|e| ConnectorError::Transient(e.to_string()))?;
        Ok(Fetched {
            bytes: bytes.len() as u64,
            mime: Some("image/jpeg".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Browser;

    fn every<T>(mut page: impl FnMut(Option<&Cursor>) -> (Vec<T>, Option<Cursor>)) -> Vec<Vec<T>> {
        let mut pages = Vec::new();
        let mut at = None;
        loop {
            let (got, next) = page(at.as_ref());
            pages.push(got);
            match next {
                Some(n) => at = Some(n),
                None => return pages,
            }
        }
    }

    #[test]
    fn its_albums_and_their_photos_come_page_by_page() {
        let provider = AlbumsProvider::new(&[("Trip", 3), ("Family", 1), ("Empty", 0)], 2);
        let manifest = provider.manifest();
        assert!(manifest.caps.collections);
        // Declaring none, it is browsed by its albums.
        assert_eq!(manifest.browsers(), [Browser::Collections]);
        let c = provider
            .create(&Settings::Null, &ConnectorServices::default())
            .unwrap();
        let account = AccountRef {
            connector: ID.into(),
            id: "acc".into(),
        };
        let cancel = Cancel::new();
        let pages = every(|from| {
            let p = c.collections(&account, from, 50, &cancel).unwrap();
            (p.collections, p.next)
        });
        let names: Vec<Vec<String>> = pages
            .iter()
            .map(|p| p.iter().map(|a| a.source.label.clone()).collect())
            .collect();
        assert_eq!(names, [vec!["Trip", "Family"], vec!["Empty"]]);
        let trip = pages[0][0].clone();
        assert_eq!(trip.count, Some(3));
        let photos = every(|from| {
            let p = c.list(&trip.source, from, 50, &cancel).unwrap();
            assert_eq!(p.complete, p.next.is_none());
            (p.items, p.next)
        });
        let ids: Vec<Vec<String>> = photos
            .iter()
            .map(|p| p.iter().map(|i| i.id.0.clone()).collect())
            .collect();
        assert_eq!(ids, [vec!["a0-p0", "a0-p1"], vec!["a0-p2"]]);
        let mut bytes = Vec::new();
        let item = ItemRef {
            source: trip.source,
            id: ItemId("a0-p0".into()),
        };
        c.fetch(&item, 0, &mut bytes, &cancel).unwrap();
        assert_eq!(bytes, b"photo!");
    }

    #[test]
    fn a_connector_without_albums_says_so() {
        let c = crate::fake::FakeProvider
            .create(&Settings::Null, &ConnectorServices::default())
            .unwrap();
        assert!(!c.describe().caps.collections);
        let account = AccountRef {
            connector: crate::fake::ID.into(),
            id: "acc".into(),
        };
        assert_eq!(
            c.collections(&account, None, 10, &Cancel::new()),
            Err(ConnectorError::Unsupported)
        );
    }
}
