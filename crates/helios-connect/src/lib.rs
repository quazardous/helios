//! Import connectors (`docs/design/import-connectors.md`, section 2).
//!
//! A [`Connector`] knows one kind of source: it lists a source's items
//! page by page after an opaque [`Cursor`], gives each item a stable id, a
//! decorative path, metadata, a size and a claimed hash when it has them,
//! says how Helios reads the bytes ([`Access`]) and fetches them. Helios,
//! never the connector, computes the BLAKE3 identity of what it imports.
//!
//! A [`ConnectorProvider`] is a connector's manifest (data, readable
//! without running anything) and its constructor; [`registry`] lists the
//! providers Helios knows, as `helios_node::registry()` lists image
//! modules. The local folder ([`folder`]) and the mounted card ([`card`])
//! are its first two; plugins join it in a later slice.
//!
//! Connectors that need an account declare a connection process
//! (`[[connect]]`): [`connect`] drives it, [`account`] keeps what it
//! found.
//!
//! [`contract`] is the suite every connector passes; [`land`] is the part
//! file fetched bytes land in, hashed as they arrive.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use helios_core::config::Contracts;
use serde::{Deserialize, Serialize};

pub mod account;
pub mod card;
pub mod connect;
pub mod contract;
/// A connector declaring a form and a test step, for tests.
#[doc(hidden)]
pub mod fake;
pub mod folder;
pub mod keyring;
pub mod land;
pub mod local;
pub mod oauth;
pub mod ranged;

/// The version of the connector interface this crate speaks.
pub const INTERFACE: u32 = 1;

/// The page size a listing asks for when it has no reason to ask for
/// another: 500 items, as the importer protocol's version 2.
pub const PAGE: usize = 500;

/// One kind of source.
pub trait Connector: Send + Sync {
    /// Id, label and what it can do.
    fn describe(&self) -> ConnectorInfo;

    /// The sources present now under an account (cards plugged in, phones
    /// connected), for the import window's Sources column. A connector
    /// without detection returns the sources it was given.
    fn sources(&self, account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError>;

    /// One page of at most `page` items of a source, from the start
    /// (`from: None`) or after a cursor. The cursor is opaque: Helios keeps
    /// it and gives it back, possibly to another instance of the
    /// connector, possibly in another run.
    fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError>;

    /// What changed since a cursor, when `caps.changes` (a change feed).
    fn changes(
        &self,
        _source: &SourceRef,
        _since: &Cursor,
        _cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        Err(ConnectorError::Unsupported)
    }

    /// A small preview without fetching the file, when `caps.thumbnails`:
    /// an encoded picture (JPEG, PNG) at most `max_side` pixels on its
    /// long side, or about.
    fn thumbnail(
        &self,
        _item: &ItemRef,
        _max_side: u32,
    ) -> Result<Option<Vec<u8>>, ConnectorError> {
        Ok(None)
    }

    /// Proves the connection under an account (the `test` step of a
    /// connection process): the account's name as the source knows it
    /// (an e-mail address, a server), or why it fails.
    fn probe(
        &self,
        _account: Option<&AccountRef>,
        _cancel: &Cancel,
    ) -> Result<Probed, ConnectorError> {
        Err(ConnectorError::Unsupported)
    }

    /// Starts a picking under an account, when `caps.picks`: the page
    /// the person picks on, which Helios opens in the system browser.
    /// What is picked is then listed under the account's source.
    fn pick(&self, _account: &AccountRef, _cancel: &Cancel) -> Result<Pick, ConnectorError> {
        Err(ConnectorError::Unsupported)
    }

    /// Waits for the person to finish picking, at most until `deadline`:
    /// `true` once done, `false` at the deadline.
    fn picked(
        &self,
        _deadline: std::time::Instant,
        _cancel: &Cancel,
    ) -> Result<bool, ConnectorError> {
        Err(ConnectorError::Unsupported)
    }

    /// Ends the picking (the service forgets it), its items imported or
    /// given up.
    fn end_pick(&self) -> Result<(), ConnectorError> {
        Ok(())
    }

    /// How Helios reads the item's bytes: in place (a mounted folder) or
    /// fetched.
    fn access(&self, item: &ItemRef) -> Access;

    /// Writes the item's bytes into `sink`, from byte `offset` when the
    /// connector can resume (`caps.resume`). Helios owns the sink.
    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError>;
}

/// What a connector is and can do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInfo {
    /// `folder`, `card`, later `google-photos`, `mtp`…
    pub id: String,
    /// Its name, as a text key and its English fallback.
    pub label: Text,
    pub caps: Caps,
}

/// A text shown to the person: its key in the translations and the
/// English it falls back to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub key: String,
    pub en: String,
}

impl Text {
    pub fn new(key: &str, en: &str) -> Self {
        Self {
            key: key.to_owned(),
            en: en.to_owned(),
        }
    }
}

/// What a connector can do.
// A table of independent capability flags, read by name, as the manifest
// declares them: not a state machine hiding in booleans.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caps {
    /// It needs a sign-in.
    pub accounts: bool,
    /// Its sources come and go (cards, phones).
    pub detect: bool,
    /// The hash it claims for its items, if any.
    pub claims: Option<HashAlgo>,
    /// Sizes are known before a fetch.
    pub sizes: bool,
    pub thumbnails: bool,
    /// A fetch can start from an offset.
    pub resume: bool,
    /// A change feed ([`Connector::changes`]).
    pub changes: bool,
    pub originals: Originals,
    /// Its items are picked by the person in the service's own page
    /// ([`Connector::pick`]): the import window offers "Pick photos…".
    pub picks: bool,
    /// What its service lets Helios ask, so reading ahead (glances of
    /// items not shown yet) stays within it.
    pub limits: Limits,
}

/// How many requests a connector's service allows: the read-ahead of
/// glances pauses at them. `None`: no limit declared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Requests a minute.
    pub per_minute: Option<u32>,
    /// Requests a day: a hint of the service's daily quota.
    pub per_day: Option<u32>,
}

/// A hash algorithm a connector may claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HashAlgo {
    Blake3,
    Md5,
    Sha1,
    Sha256,
    /// One Helios cannot compute: recorded, reported as unchecked.
    Other(String),
}

impl HashAlgo {
    /// Its name, as manifests and the claimed-hash table write it
    /// (`md5`, `sha1`…; an unknown one as given).
    pub fn name(&self) -> &str {
        match self {
            Self::Blake3 => "blake3",
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Other(name) => name,
        }
    }
}

/// Whether a source's bytes are the ones uploaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Originals {
    /// The very file (a card, a folder).
    #[default]
    Exact,
    /// Bytes that may differ from the upload (a service re-encoding).
    Rendered,
}

/// One identity on a connector, when it has any.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountRef {
    pub connector: String,
    pub id: String,
}

/// A picking started ([`Connector::pick`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    /// The page the person picks on.
    pub url: String,
    /// The source the picked items are listed under.
    pub source: SourceRef,
}

/// What a connection's test found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probed {
    /// The account's name as the source knows it.
    pub label: Option<String>,
}

/// One place a connector lists: a folder, a card, a phone.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRef {
    /// The connector it belongs to.
    pub connector: String,
    /// Stable within the connector: for a local source, its path.
    pub id: String,
    /// Its name as shown: a card's label, a folder's name.
    pub label: String,
    /// Where it is mounted, for a local source.
    pub root: Option<PathBuf>,
    /// Its size in bytes, when known (a card's capacity).
    pub capacity: Option<u64>,
}

/// An opaque position in a source's listing, kept by Helios between runs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cursor(pub String);

/// A cancellation flag, shared with whoever may stop the work.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// A flag set elsewhere (a scan's stop).
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// One page of a listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub items: Vec<Item>,
    /// Where the next page starts: `None` once the listing is complete.
    pub next: Option<Cursor>,
    /// Every item of the source was listed.
    pub complete: bool,
}

/// An item's id: stable within its source, the resume and dedup key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub String);

/// One file a source holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    /// Decorative: shown, used for names, never trusted for identity.
    pub path: String,
    pub size: Option<u64>,
    /// When the bytes last changed, in seconds since 1970.
    pub modified: Option<i64>,
    /// The capture time, when the source knows it, in seconds since 1970.
    pub taken: Option<i64>,
    pub kind: ItemKind,
    pub mime: Option<String>,
    /// The source's opinion, keyed by XMP property names (`tiff:Model`,
    /// `exif:DateTimeOriginal`, `dc:subject`…); the file's own metadata
    /// wins after a fetch.
    pub metadata: BTreeMap<String, String>,
    /// The hash the source states, in its algorithm.
    pub claim: Option<Claim>,
    /// Changes when the item's bytes change (an `ETag`).
    pub revision: Option<String>,
    /// Items the source knows belong together: a RAW+JPEG pair and its
    /// sidecars share one (a DCF object's base name).
    pub group: Option<String>,
}

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    Photo,
    Raw,
    Video,
    Sidecar,
}

/// A hash as a source states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub algorithm: HashAlgo,
    /// Lowercase hexadecimal.
    pub digest: String,
}

/// An item as Helios asks for it again: its source and id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemRef {
    pub source: SourceRef,
    pub id: ItemId,
}

/// How Helios reads an item's bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// The file is here, readable where it is.
    InPlace(PathBuf),
    /// The bytes come through [`Connector::fetch`].
    Fetch,
}

/// Where a fetch writes: Helios's, which hashes as bytes arrive.
pub trait FetchSink {
    fn accept(&mut self, chunk: &[u8]) -> std::io::Result<()>;
}

impl FetchSink for Vec<u8> {
    fn accept(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        self.extend_from_slice(chunk);
        Ok(())
    }
}

/// What a fetch wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub bytes: u64,
    pub mime: Option<String>,
}

/// Why a connector call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorError {
    Unsupported,
    NotFound,
    /// The item went away.
    Gone,
    Offline,
    /// Retry after a sign-in.
    AuthNeeded,
    /// Retry after listing again (a download link expired).
    Expired,
    RateLimited {
        retry_after: Option<Duration>,
    },
    /// The work was cancelled.
    Cancelled,
    Transient(String),
    Fatal(String),
}

impl std::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("not supported by this source"),
            Self::NotFound => f.write_str("not found"),
            Self::Gone => f.write_str("no longer there"),
            Self::Offline => f.write_str("offline"),
            Self::AuthNeeded => f.write_str("sign-in needed"),
            Self::Expired => f.write_str("expired"),
            Self::RateLimited {
                retry_after: Some(d),
            } => write!(f, "asked to wait {} s", d.as_secs()),
            Self::RateLimited { retry_after: None } => f.write_str("asked to wait"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Transient(e) | Self::Fatal(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ConnectorError {}

/// A connector's manifest: what Helios knows of it without running it
/// (section 8.1 of the design).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub id: String,
    pub version: String,
    pub label: Text,
    pub description: Text,
    pub caps: Caps,
    /// What it needs from the machine and the person.
    #[serde(default)]
    pub needs: Needs,
    /// Its settings' schema; `null` when it has none.
    #[serde(default)]
    pub settings: serde_json::Value,
    /// The platforms it runs on (`linux`, `windows`, `macos`); empty: all.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// The connector interface version it speaks ([`INTERFACE`]).
    pub interface: u32,
    /// The connection process Helios drives before the first listing
    /// (`[[connect]]`, section 2.4.1); empty for folders and cards.
    #[serde(default)]
    pub connect: Vec<ConnectStep>,
    /// How the import window browses its sources, in the order offered
    /// (section 3.9); empty: [`ConnectorManifest::browsers`] derives them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub browsers: Vec<Browser>,
}

impl ConnectorManifest {
    /// The browsers its sources are shown in: the ones declared, else
    /// the picker for a connector whose items are picked in the service's
    /// own page, else one flat folder.
    pub fn browsers(&self) -> Vec<Browser> {
        if !self.browsers.is_empty() {
            let mut seen = Vec::with_capacity(self.browsers.len());
            for b in &self.browsers {
                if !seen.contains(b) {
                    seen.push(*b);
                }
            }
            return seen;
        }
        if self.caps.picks {
            vec![Browser::Picker]
        } else {
            vec![Browser::Folder]
        }
    }
}

/// A way of browsing a source in the import window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Browser {
    /// Its folders and subfolders, one folder's photos (with its
    /// subfolders') at a time.
    Tree,
    /// Every photo in one flat folder.
    Folder,
    /// The albums of a service.
    Collections,
    /// By capture date.
    Timeline,
    /// The service's own picking page ([`Connector::pick`]).
    Picker,
}

impl Browser {
    /// Every browser, in the order the import window offers them.
    pub const ALL: [Self; 5] = [
        Self::Tree,
        Self::Folder,
        Self::Collections,
        Self::Timeline,
        Self::Picker,
    ];

    /// Its name, as manifests and the saved sources write it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Folder => "folder",
            Self::Collections => "collections",
            Self::Timeline => "timeline",
            Self::Picker => "picker",
        }
    }

    /// The browser of that name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.name() == name)
    }
}

/// What a connector needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Needs {
    pub account: bool,
    pub network: bool,
    pub usb: bool,
}

/// One step of a declared connection process, which Helios drives itself
/// so every connector gets the same screens, states and tests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectStep {
    /// Names the step, so a reconnection replays only the steps that
    /// failed.
    pub id: String,
    #[serde(flatten)]
    pub kind: StepKind,
}

/// What a connection step asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StepKind {
    /// The system browser on the consent page, PKCE and a loopback
    /// redirect.
    Oauth {
        authorize: String,
        token: String,
        #[serde(default)]
        scopes: Vec<String>,
        /// Where a token is revoked on signing out, when the service has
        /// one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revoke: Option<String>,
        /// The client id, when the connector ships one; the environment
        /// and the account's own override it ([`oauth`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
    /// A form built from a schema; its secret fields go to the keyring.
    Form { schema: serde_json::Value },
    /// An instruction to follow on the device, and a wait for it.
    Device {
        instruction: Text,
        #[serde(default = "device_timeout")]
        timeout_secs: u64,
    },
    /// A fingerprint to confirm once, remembered per account.
    Trust,
    /// A code shown on one side and typed on the other.
    Code,
    /// A call proving the connection, as the last step.
    Test,
}

fn device_timeout() -> u64 {
    120
}

/// A connector's settings, as its manifest's schema describes them.
pub type Settings = serde_json::Value;

/// The cards and cameras mounted now.
pub type CardsFn = Arc<dyn Fn() -> Vec<helios_storage::Card> + Send + Sync>;
/// The folders a host gives the folder connector as its sources.
pub type PlacesFn = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

/// An account's access token, by account id.
pub type TokenFn = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// The values the person gave an account's or a source's settings
/// (`level`), by key, for the account or source `id`, the keys of
/// `contract` read: secrets from the keyring as the call is made. What a
/// plugin's connector hands its plugin with each call.
pub type ValuesFn = Arc<
    dyn Fn(
            helios_core::config::Level,
            &str,
            &helios_core::config::Contract,
        ) -> serde_json::Map<String, serde_json::Value>
        + Send
        + Sync,
>;

/// What the host gives connectors.
#[derive(Clone, Default)]
pub struct ConnectorServices {
    /// The cards mounted, as the host last polled them; `None`: the card
    /// connector asks the system itself.
    pub cards: Option<CardsFn>,
    /// The folders the person keeps (recent, favourites): the folder
    /// connector's sources.
    pub places: Option<PlacesFn>,
    /// A valid access token for an account, by id, renewed when it
    /// expired (`oauth::OAuth::access_token`): what a connector signed in
    /// through an `oauth` step calls its service with.
    pub token: Option<TokenFn>,
    /// The accounts' and sources' configuration values; `None`: their
    /// contracts' defaults.
    pub values: Option<ValuesFn>,
}

impl std::fmt::Debug for ConnectorServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorServices")
            .field("cards", &self.cards.is_some())
            .field("places", &self.places.is_some())
            .field("token", &self.token.is_some())
            .field("values", &self.values.is_some())
            .finish()
    }
}

/// A connector's manifest and constructor.
pub trait ConnectorProvider: Send + Sync {
    /// Data, readable without running the connector.
    fn manifest(&self) -> ConnectorManifest;

    /// What configures it, each account it connects and each source it
    /// reads (the configuration contract, [`helios_core::config`]);
    /// nothing by default.
    fn config(&self) -> Contracts {
        Contracts::default()
    }

    fn create(
        &self,
        settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError>;
}

/// A provider in the registry.
pub type ProviderRef = Arc<dyn ConnectorProvider>;

/// The connectors Helios knows, by id. In id order, which puts cards
/// before folders, as the import window shows them.
pub fn registry() -> BTreeMap<&'static str, ProviderRef> {
    BTreeMap::from([
        (card::ID, Arc::new(card::CardProvider) as ProviderRef),
        (folder::ID, Arc::new(folder::FolderProvider) as ProviderRef),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_lists_cards_then_folders_each_under_its_manifest_id() {
        let registry = registry();
        assert_eq!(
            registry.keys().copied().collect::<Vec<_>>(),
            ["card", "folder"]
        );
        for (id, provider) in &registry {
            let manifest = provider.manifest();
            assert_eq!(manifest.id, *id);
            assert_eq!(manifest.interface, INTERFACE);
            // Folders and cards declare no connection process.
            assert!(manifest.connect.is_empty());
            assert!(manifest.browsers().contains(&Browser::Timeline), "{id}");
            let connector = provider
                .create(&Settings::Null, &ConnectorServices::default())
                .unwrap();
            assert_eq!(connector.describe().id, *id);
            assert_eq!(connector.describe().caps, manifest.caps);
            // Each declares its sources' choices, soundly, and nothing
            // for itself or accounts it does not have.
            let config = provider.config();
            assert_eq!(config.problems(), Vec::<String>::new(), "{id}");
            assert!(config.own.is_empty() && config.account.is_empty());
            for key in [
                "copy",
                "into",
                "layout",
                "rename",
                "duplicates",
                "darktable",
            ] {
                assert!(config.source.get(key).is_some(), "{id}: {key}");
            }
        }
        // A card's photos are copied unless the person switches; a
        // folder's are added where they are.
        let copy = |id: &str| registry[id].config().source.defaults()["copy"].clone();
        assert_eq!(copy("card"), serde_json::json!(true));
        assert_eq!(copy("folder"), serde_json::json!(false));
    }

    #[test]
    fn a_declared_connection_process_reads_as_data() {
        let manifest: ConnectorManifest = serde_json::from_value(serde_json::json!({
            "id": "nas",
            "version": "1.0.0",
            "label": { "key": "connect-nas", "en": "NAS" },
            "description": { "key": "connect-nas-about", "en": "A share" },
            "caps": { "sizes": true },
            "interface": 1,
            "connect": [
                { "id": "server", "kind": "form", "schema": { "host": "string" } },
                { "id": "cert", "kind": "trust" },
                { "id": "allow", "kind": "device",
                  "instruction": { "key": "k", "en": "Allow access on the phone" } },
                { "id": "probe", "kind": "test" }
            ]
        }))
        .unwrap();
        let kinds: Vec<_> = manifest.connect.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(kinds, ["server", "cert", "allow", "probe"]);
        assert!(matches!(
            manifest.connect[2].kind,
            StepKind::Device {
                timeout_secs: 120,
                ..
            }
        ));
        assert_eq!(manifest.connect[3].kind, StepKind::Test);
        assert!(manifest.caps.sizes && !manifest.caps.detect);
        // A round trip keeps it whole.
        let again: ConnectorManifest =
            serde_json::from_value(serde_json::to_value(&manifest).unwrap()).unwrap();
        assert_eq!(again, manifest);
    }
}
