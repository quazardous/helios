//! A connector for tests that declares a connection process: a form
//! (a server, a user, a secret password) and a test step that accepts
//! the password `right` only. It lists nothing. Each of its sources
//! has one parameter, the share it reads (`share`), and no copy or add
//! choice: its photos are always copied. The import window's tests use
//! it for "Add a source…", "Edit source…", Settings > Accounts and
//! the switch between a source's browsers (it declares three).

use helios_core::config::{ConfigKey, Contract, Contracts, KeyType};

use crate::{
    Access, AccountRef, Browser, Cancel, Caps, ConnectStep, Connector, ConnectorError,
    ConnectorInfo, ConnectorManifest, ConnectorProvider, ConnectorServices, FetchSink, Fetched,
    INTERFACE, ItemRef, Page, Probed, Settings, SourceRef, StepKind, Text,
};

/// Its id.
pub const ID: &str = "test-server";

/// The provider.
pub struct FakeProvider;

/// A connector created with the account's settings.
pub struct Fake {
    settings: Settings,
}

fn label() -> Text {
    Text::new("connect-test-server", "Test server")
}

impl ConnectorProvider for FakeProvider {
    fn config(&self) -> Contracts {
        Contracts {
            source: Contract::new().key(
                "share",
                ConfigKey::new(KeyType::String)
                    .default_value(serde_json::json!("photos"))
                    .label("Share")
                    .help("The share the photos are read from"),
            ),
            account: Contract::new()
                .key(
                    "region",
                    ConfigKey::new(KeyType::String)
                        .default_value(serde_json::json!("eu"))
                        .label("Region"),
                )
                .key("api-key", ConfigKey::new(KeyType::Secret).label("API key")),
            ..Contracts::default()
        }
    }

    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: ID.into(),
            version: "1.0.0".into(),
            label: label(),
            description: Text::new("connect-test-server-about", "A server for tests"),
            caps: Caps {
                accounts: true,
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
            // Three ways to browse, for the import window's switch.
            browsers: vec![Browser::Folder, Browser::Collections, Browser::Timeline],
            setup: None,
            connect: vec![
                ConnectStep {
                    id: "server".into(),
                    kind: StepKind::Form {
                        schema: serde_json::json!({ "fields": [
                            { "id": "host", "label": "Server", "required": true },
                            { "id": "user", "label": "User" },
                            { "id": "password", "label": "Password", "kind": "secret",
                              "required": true }
                        ] }),
                    },
                },
                ConnectStep {
                    id: "probe".into(),
                    kind: StepKind::Test,
                },
            ],
        }
    }

    fn create(
        &self,
        settings: &Settings,
        _services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(Fake {
            settings: settings.clone(),
        }))
    }
}

impl Connector for Fake {
    fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: ID.into(),
            label: label(),
            caps: FakeProvider.manifest().caps,
        }
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(Vec::new())
    }

    fn list(
        &self,
        _source: &SourceRef,
        _from: Option<&crate::Cursor>,
        _page: usize,
        _cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        Ok(Page {
            complete: true,
            ..Page::default()
        })
    }

    fn probe(
        &self,
        _account: Option<&AccountRef>,
        _cancel: &Cancel,
    ) -> Result<Probed, ConnectorError> {
        let get = |k: &str| self.settings.get(k).and_then(|v| v.as_str()).unwrap_or("");
        if get("password") != "right" {
            return Err(ConnectorError::AuthNeeded);
        }
        let user = get("user");
        let host = get("host");
        Ok(Probed {
            label: Some(if user.is_empty() {
                host.to_owned()
            } else {
                format!("{user}@{host}")
            }),
        })
    }

    fn access(&self, _item: &ItemRef) -> Access {
        Access::Fetch
    }

    fn fetch(
        &self,
        _item: &ItemRef,
        _offset: u64,
        _sink: &mut dyn FetchSink,
        _cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        Err(ConnectorError::NotFound)
    }
}
