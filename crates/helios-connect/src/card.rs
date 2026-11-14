//! The mounted card connector: the cards and cameras the system mounts
//! (`helios_storage`), each read from its `DCIM` folder (the DCF rule) in
//! place. Its sources come and go (`caps.detect`): the host gives the
//! cards it polls, else the connector asks the system. It declares no
//! connection process.

use helios_core::config::Contracts;
use helios_storage::Card;

use crate::local::{Local, source_at, source_contract};
use crate::{
    Access, AccountRef, Browser, Cancel, Caps, CardsFn, Connector, ConnectorError, ConnectorInfo,
    ConnectorManifest, ConnectorProvider, ConnectorServices, Cursor, FetchSink, Fetched, INTERFACE,
    ItemRef, Needs, Page, Settings, SourceRef, Text,
};

/// The card connector's id.
pub const ID: &str = "card";

fn caps() -> Caps {
    Caps {
        detect: true,
        sizes: true,
        resume: true,
        ..Caps::default()
    }
}

fn label() -> Text {
    Text::new("connect-card", "Cards and cameras")
}

/// The card connector's provider.
pub struct CardProvider;

impl ConnectorProvider for CardProvider {
    fn config(&self) -> Contracts {
        Contracts {
            source: source_contract(true),
            ..Contracts::default()
        }
    }

    fn manifest(&self) -> ConnectorManifest {
        ConnectorManifest {
            id: ID.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            label: label(),
            description: Text::new(
                "connect-card-about",
                "Memory cards, and cameras and phones mounted as a disk",
            ),
            caps: caps(),
            needs: Needs::default(),
            settings: serde_json::Value::Null,
            platforms: Vec::new(),
            interface: INTERFACE,
            connect: Vec::new(),
            browsers: vec![Browser::Folder, Browser::Timeline],
            setup: Some(crate::Setup {
                asks: crate::SetupAsks::Volume,
                icon: "sd-card".into(),
                auto: Some(crate::AutoImport::Plugged),
            }),
        }
    }

    fn create(
        &self,
        _settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        Ok(Box::new(CardConnector {
            local: Local::new(ID, label(), caps()),
            cards: services.cards.clone(),
        }))
    }
}

/// The card connector.
pub struct CardConnector {
    local: Local,
    cards: Option<CardsFn>,
}

impl CardConnector {
    /// The source a card is: its mount point, labelled by its label.
    pub fn source(card: &Card) -> SourceRef {
        source_at(ID, &card.root, card.label.clone(), card.capacity)
    }
}

impl Connector for CardConnector {
    fn describe(&self) -> ConnectorInfo {
        self.local.describe()
    }

    fn sources(&self, _account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        let cards = match &self.cards {
            Some(cards) => cards(),
            None => helios_storage::system().cards(),
        };
        Ok(cards.iter().map(Self::source).collect())
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
