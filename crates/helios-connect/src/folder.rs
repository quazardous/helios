//! The local folder connector: a folder with its subfolders (from `DCIM`
//! when it holds one), read in place. Its sources are the folders the host
//! gives it (the import window's recent folders and favourites); it
//! detects nothing and declares no connection process.

use std::path::Path;

use crate::local::{Local, source_at};
use crate::{
    Access, AccountRef, Cancel, Caps, Connector, ConnectorError, ConnectorInfo, ConnectorManifest,
    ConnectorProvider, ConnectorServices, Cursor, FetchSink, Fetched, INTERFACE, ItemRef, Needs,
    Page, PlacesFn, Settings, SourceRef, Text,
};

/// The folder connector's id.
pub const ID: &str = "folder";

fn caps() -> Caps {
    Caps {
        sizes: true,
        resume: true,
        ..Caps::default()
    }
}

fn label() -> Text {
    Text::new("connect-folder", "Folder")
}

/// The folder connector's provider.
pub struct FolderProvider;

impl ConnectorProvider for FolderProvider {
    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            label: label(),
            description: Text::new(
                "connect-folder-about",
                "A folder on this computer or a share it mounts, with its subfolders",
            ),
            caps: caps(),
            needs: Needs::default(),
            settings: serde_json::Value::Null,
            platforms: Vec::new(),
            interface: INTERFACE,
            connect: Vec::new(),
        }
    }

    fn create(
        &self,
        _settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(Folder {
            local: Local::new(ID, label(), caps()),
            places: services.places.clone(),
        }))
    }
}

/// The folder connector.
pub struct Folder {
    local: Local,
    places: Option<PlacesFn>,
}

impl Folder {
    /// The source a folder is.
    pub fn source(path: &Path) -> SourceRef {
        let label = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        source_at(ID, path, label, None)
    }
}

impl Connector for Folder {
    fn describe(&self) -> ConnectorInfo {
        self.local.describe()
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(self
            .places
            .as_ref()
            .map_or_else(Vec::new, |places| places())
            .iter()
            .map(|p| Self::source(p))
            .collect())
    }

    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        self.local.list(source, from, page, cancel)
    }

    fn access(&self, item: &ItemRef) -> Access {
        Local::access(item)
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        Local::fetch(item, offset, sink, cancel)
    }
}
