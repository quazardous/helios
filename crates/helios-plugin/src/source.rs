//! The `import.source` service as a [`helios_connect::Connector`]: a
//! plugin that lists and downloads a service's photos (Google Photos'
//! Picker), driven through the [`Broker`] so the import window and
//! `helios_library::fetch::import_remote` use it as any connector
//! (`docs/design/import-connectors.md`, 2.6).
//!
//! The plugin keeps no token: every call carries `auth`, the account's
//! access token as the host renews it ([`ConnectorServices::token`]).
//! Every call also carries `config`: the plugin's own settings
//! (`plugin`, put by the broker), the account's (`account`) and the
//! source's (`source`), each its contract's defaults under the values
//! the person gave ([`ConnectorServices::values`], secrets read from the
//! keyring as the call is made); a value its contract refuses is left
//! out, said in the log without the value.
//! Its error -32010 is "sign-in needed" ([`ConnectorError::AuthNeeded`]):
//! the account needs a reconnection, nothing is retried.
//!
//! Methods: `source.probe`, `source.pick` (`{session, pickerUri}`),
//! `source.wait` (`{ready}`, at most `timeoutSecs`), `source.list`
//! (`{items, next, complete}`), `source.fetch` (the items into a staging
//! folder Helios gives, `{fetched, failed}`), `source.thumbnail` (an
//! item's preview into the staging folder, `{file}`, when the manifest
//! says `thumbnails`), `source.close`.
//!
//! The manifest's `thumbnails` and `[limits]` are the connector's caps:
//! the import window reads glances ahead within those limits.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_connect::{
    Access, AccountRef, Cancel, Caps, ConnectStep, Connector, ConnectorError, ConnectorInfo,
    ConnectorManifest, ConnectorProvider, ConnectorServices, Cursor, FetchSink, Fetched, Item,
    ItemId, ItemKind, ItemRef, Needs, Originals, Page, Pick, Probed, Settings, SourceRef, Text,
    TokenFn, ValuesFn,
};
use helios_core::config::{Contracts, Level};
use serde_json::{Value as Json, json};

use crate::Broker;
use crate::rpc::{self, CallError};

/// The service a source plugin provides.
pub const SERVICE: &str = "import.source";
/// The event that starts it.
const EVENT: &str = "onService:import.source";
/// How long one `source.wait` lasts at most: the wait is a loop of them,
/// so a cancel is seen between two.
const WAIT_SLICE: u64 = 2;
/// A call's time beyond what it asks the service to wait.
const CALL: Duration = Duration::from_mins(1);
/// One item's download.
const FETCH: Duration = Duration::from_mins(10);

static NEXT: AtomicU64 = AtomicU64::new(0);

/// The connectors of the enabled `import.source` plugins.
pub fn providers(broker: &Arc<Broker>) -> Vec<SourceProvider> {
    broker
        .providers(SERVICE)
        .into_iter()
        .filter_map(|id| SourceProvider::new(broker.clone(), id))
        .collect()
}

/// The plugin behind the connector `connector` (the last part of its id,
/// as [`SourceProvider::new`] names it), when one serves
/// `import.source`.
pub fn plugin_of(broker: &Broker, connector: &str) -> Option<String> {
    broker
        .providers(SERVICE)
        .into_iter()
        .find(|id| id.rsplit('.').next() == Some(connector))
        .map(String::from)
}

/// A source plugin's connector provider: its manifest read from the
/// plugin's, its connectors calling it through the broker.
pub struct SourceProvider {
    broker: Arc<Broker>,
    plugin: String,
    manifest: ConnectorManifest,
    /// The plugin's configuration contracts, from its manifest.
    config: Contracts,
}

impl SourceProvider {
    /// The provider of the plugin `plugin`, when it serves
    /// `import.source`. Its connector id is the last part of the
    /// plugin's id (`google-photos`): the accounts and the client id's
    /// environment variable are named after it.
    pub fn new(broker: Arc<Broker>, plugin: &str) -> Option<Self> {
        let m = broker.manifest(plugin)?;
        if !m.services.iter().any(|s| s == SERVICE) {
            return None;
        }
        let id = plugin.rsplit('.').next().unwrap_or(plugin).to_owned();
        let title = if m.title.is_empty() {
            &m.name
        } else {
            &m.title
        };
        let connect: Vec<ConnectStep> = m
            .connect
            .iter()
            .filter_map(|step| {
                let decl = serde_json::to_value(step).ok()?;
                serde_json::from_value(decl)
                    .map_err(|e| log::warn!("plugin `{plugin}`, step `{}`: {e}", step.id))
                    .ok()
            })
            .collect();
        let declares = |c: &str| m.capabilities.iter().any(|d| d.name == c);
        let config = m.contracts();
        let manifest = ConnectorManifest {
            label: Text::new(&format!("connect-{id}"), title),
            description: Text::new(&format!("connect-{id}-about"), &m.description),
            version: m.version.clone(),
            caps: Caps {
                accounts: !connect.is_empty(),
                originals: Originals::Rendered,
                picks: true,
                // Not served to plugins yet: the manifest refuses `true`.
                collections: false,
                thumbnails: m.thumbnails,
                limits: m.limits,
                ..Caps::default()
            },
            needs: Needs {
                account: !connect.is_empty(),
                network: declares("network"),
                usb: declares("device.usb"),
            },
            settings: Json::Null,
            platforms: Vec::new(),
            interface: helios_connect::INTERFACE,
            connect,
            browsers: m
                .browsers
                .iter()
                .filter_map(|b| helios_connect::Browser::from_name(b))
                .collect(),
            id,
        };
        Some(Self {
            plugin: plugin.to_owned(),
            config,
            broker,
            manifest,
        })
    }

    /// The plugin's id.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// Its connector, calling the plugin with the tokens `token` gives
    /// and the account's and source's values `values` gives (their
    /// defaults without).
    pub fn connector(&self, token: TokenFn, values: Option<ValuesFn>) -> PluginSource {
        PluginSource {
            config: self.config.clone(),
            values,
            broker: self.broker.clone(),
            plugin: self.plugin.clone(),
            id: self.manifest.id.clone(),
            label: self.manifest.label.clone(),
            caps: self.manifest.caps.clone(),
            token,
            session: Mutex::new(None),
            downloads: Mutex::new(HashMap::new()),
            staging: std::env::temp_dir().join(format!(
                "helios-source-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )),
        }
    }
}

impl ConnectorProvider for SourceProvider {
    fn manifest(&self) -> ConnectorManifest {
        self.manifest.clone()
    }

    fn config(&self) -> Contracts {
        self.config.clone()
    }

    fn create(
        &self,
        _settings: &Settings,
        services: &ConnectorServices,
    ) -> Result<Box<dyn Connector>, ConnectorError> {
        let token = services.token.clone().unwrap_or_else(|| {
            Arc::new(|id: &str| Err(format!("no sign-in for the account {id}")))
        });
        Ok(Box::new(self.connector(token, services.values.clone())))
    }
}

/// A source plugin as a [`Connector`]: one picking at a time, its items
/// listed under the account's source, each downloaded by the plugin into
/// a staging folder of its own, then handed over and removed.
pub struct PluginSource {
    broker: Arc<Broker>,
    plugin: String,
    id: String,
    label: Text,
    /// Its manifest's: what it does, its limits.
    caps: Caps,
    token: TokenFn,
    /// Its plugin's contracts: what the account's and source's values
    /// are checked against.
    config: Contracts,
    values: Option<ValuesFn>,
    /// The picking going: its account and session.
    session: Mutex<Option<(String, String)>>,
    /// Each item listed, as the plugin gave it (its `download` beside).
    downloads: Mutex<HashMap<String, Json>>,
    /// Where the plugin downloads, removed with the connector.
    staging: PathBuf,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A plugin's error as a connector's.
pub fn error_of(e: CallError) -> ConnectorError {
    match e {
        CallError::Rpc { code: -32010, .. } => ConnectorError::AuthNeeded,
        CallError::Rpc { code: -32011, .. } => ConnectorError::RateLimited { retry_after: None },
        CallError::Rpc { code: -32012, .. } => ConnectorError::Expired,
        CallError::Rpc { code: -32013, .. } => ConnectorError::Offline,
        CallError::Rpc { code: -32800, .. } | CallError::Cancelled => ConnectorError::Cancelled,
        CallError::Rpc { message, .. } => ConnectorError::Fatal(message),
        CallError::Timeout(_) => ConnectorError::Transient(e.to_string()),
        CallError::Stopped(why) => ConnectorError::Fatal(why),
    }
}

impl PluginSource {
    /// The source an account's items are listed under.
    pub fn source(&self, account: &str) -> SourceRef {
        SourceRef {
            connector: self.id.clone(),
            id: account.to_owned(),
            label: self.label.en.clone(),
            root: None,
            capacity: None,
        }
    }

    /// Calls the plugin with the account's token, cancelled with
    /// `cancel`.
    fn call(
        &self,
        account: &str,
        method: &str,
        mut params: Json,
        timeout: Duration,
        cancel: &Cancel,
    ) -> Result<Json, ConnectorError> {
        if cancel.is_cancelled() {
            return Err(ConnectorError::Cancelled);
        }
        let token = (self.token)(account).map_err(|e| {
            log::warn!("{}: the account {account}'s token: {e}", self.id);
            ConnectorError::AuthNeeded
        })?;
        params["auth"] = json!(token);
        params["config"] = json!({
            "account": self.values_of(Level::Account, account),
            "source": self.values_of(Level::Source, account),
        });
        // The broker's cancel follows the connector's, polled while the
        // call runs.
        let stop = rpc::Cancel::new();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let (stop, cancel, done) = (stop.clone(), cancel.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if cancel.is_cancelled() {
                        stop.cancel();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
        };
        let answer =
            self.broker
                .call_rpc(&self.plugin, EVENT, method, params, timeout, Some(&stop));
        done.store(true, Ordering::Relaxed);
        let _ = watcher.join();
        answer.map_err(error_of)
    }

    /// The values of the account or source `id` at `level`, as the plugin
    /// is handed them: the contract's defaults under the person's values
    /// that fit. A plugin source's source is its account: both are named
    /// by the account's id.
    fn values_of(&self, level: Level, id: &str) -> serde_json::Map<String, Json> {
        let contract = self.config.get(level);
        if contract.is_empty() {
            return serde_json::Map::new();
        }
        let given = self
            .values
            .as_ref()
            .map(|f| f(level, id, contract))
            .unwrap_or_default();
        let (values, refused) = contract.resolve(&given);
        let at = if level == Level::Account {
            "account"
        } else {
            "source"
        };
        for r in refused {
            log::warn!("{}: the {at} {id}'s settings: {r}", self.id);
        }
        values
    }

    /// The plugin's download of one item listed into the staging folder:
    /// its answer, or why the item failed.
    fn fetch_one(&self, item: &ItemRef, cancel: &Cancel) -> Result<Json, ConnectorError> {
        let listed = lock(&self.downloads)
            .get(&item.id.0)
            .cloned()
            .ok_or(ConnectorError::Expired)?;
        let answer = self.call(
            &item.source.id,
            "source.fetch",
            json!({ "items": [listed], "staging": self.staging }),
            FETCH,
            cancel,
        )?;
        if let Some(failed) = answer["failed"].as_array().and_then(|f| f.first()) {
            let code = failed["code"].as_i64().unwrap_or(-32000);
            let message = failed["message"].as_str().unwrap_or_default().to_owned();
            return Err(error_of(CallError::Rpc { code, message }));
        }
        Ok(answer)
    }

    /// The plugin's preview of one item listed, written into the staging
    /// folder: its bytes.
    fn thumbnail_one(&self, item: &ItemRef, max_side: u32) -> Result<Vec<u8>, ConnectorError> {
        let listed = lock(&self.downloads)
            .get(&item.id.0)
            .cloned()
            .ok_or(ConnectorError::Expired)?;
        let answer = self.call(
            &item.source.id,
            "source.thumbnail",
            json!({ "item": listed, "maxSide": max_side, "staging": self.staging }),
            CALL,
            &Cancel::new(),
        )?;
        let file = staged(&self.staging, answer["file"].as_str().unwrap_or_default())?;
        let bytes = std::fs::read(&file);
        let _ = std::fs::remove_file(&file);
        bytes.map_err(|e| ConnectorError::Transient(e.to_string()))
    }

    /// The picking going listed again, every page: each item's download
    /// address renewed.
    fn refresh(&self, cancel: &Cancel) -> Result<(), ConnectorError> {
        let (account, _) = self.session()?;
        let source = self.source(&account);
        let mut from = None;
        loop {
            let page = self.list(&source, from.as_ref(), helios_connect::PAGE, cancel)?;
            match page.next {
                Some(next) if !page.complete => from = Some(next),
                _ => return Ok(()),
            }
        }
    }

    /// The picking going, its account and session.
    fn session(&self) -> Result<(String, String), ConnectorError> {
        lock(&self.session).clone().ok_or(ConnectorError::Expired)
    }
}

impl Drop for PluginSource {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// An item as the plugin lists it.
fn item_of(v: &Json) -> Option<Item> {
    let text = |k: &str| v[k].as_str().map(str::to_owned);
    Some(Item {
        id: ItemId(text("id")?),
        path: text("path").unwrap_or_default(),
        size: v["size"].as_u64(),
        modified: v["modified"].as_i64(),
        taken: v["taken"].as_i64(),
        kind: serde_json::from_value::<ItemKind>(v["kind"].clone()).unwrap_or(ItemKind::Photo),
        mime: text("mime"),
        metadata: v["metadata"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default(),
        claim: serde_json::from_value(v["claim"].clone()).ok(),
        revision: text("revision"),
        group: text("group"),
    })
}

/// The file a plugin says it wrote, held to the staging folder.
fn staged(staging: &Path, file: &str) -> Result<PathBuf, ConnectorError> {
    let file = Path::new(file);
    let (Ok(dir), Ok(real)) = (staging.canonicalize(), file.canonicalize()) else {
        return Err(ConnectorError::Fatal(format!(
            "the plugin wrote no file {}",
            file.display()
        )));
    };
    if real.parent() != Some(dir.as_path()) {
        return Err(ConnectorError::Fatal(format!(
            "the plugin wrote {} outside its staging folder",
            file.display()
        )));
    }
    Ok(real)
}

impl Connector for PluginSource {
    fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: self.id.clone(),
            label: self.label.clone(),
            caps: self.caps.clone(),
        }
    }

    fn sources(&self, account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(account
            .map(|a| vec![self.source(&a.id)])
            .unwrap_or_default())
    }

    fn probe(
        &self,
        account: Option<&AccountRef>,
        cancel: &Cancel,
    ) -> Result<Probed, ConnectorError> {
        let account = account.ok_or(ConnectorError::AuthNeeded)?;
        let answer = self.call(&account.id, "source.probe", json!({}), CALL, cancel)?;
        Ok(Probed {
            label: answer["label"].as_str().map(str::to_owned),
        })
    }

    fn pick(&self, account: &AccountRef, cancel: &Cancel) -> Result<Pick, ConnectorError> {
        let answer = self.call(&account.id, "source.pick", json!({}), CALL, cancel)?;
        let (Some(session), Some(url)) = (answer["session"].as_str(), answer["pickerUri"].as_str())
        else {
            return Err(ConnectorError::Fatal(
                "the plugin gave no picking session".into(),
            ));
        };
        *lock(&self.session) = Some((account.id.clone(), session.to_owned()));
        lock(&self.downloads).clear();
        Ok(Pick {
            url: url.to_owned(),
            source: self.source(&account.id),
        })
    }

    fn picked(&self, deadline: Instant, cancel: &Cancel) -> Result<bool, ConnectorError> {
        let (account, session) = self.session()?;
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs();
            if left == 0 {
                return Ok(false);
            }
            let slice = left.min(WAIT_SLICE);
            let answer = self.call(
                &account,
                "source.wait",
                json!({ "session": session, "timeoutSecs": slice }),
                CALL + Duration::from_secs(slice),
                cancel,
            )?;
            if answer["ready"] == true {
                return Ok(true);
            }
        }
    }

    fn end_pick(&self) -> Result<(), ConnectorError> {
        let Some((account, session)) = lock(&self.session).take() else {
            return Ok(());
        };
        self.call(
            &account,
            "source.close",
            json!({ "session": session }),
            CALL,
            &Cancel::new(),
        )
        .map(|_| ())
    }

    fn list(
        &self,
        _source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        let (account, session) = self.session()?;
        let answer = self.call(
            &account,
            "source.list",
            json!({ "session": session, "cursor": from.map(|c| &c.0), "pageSize": page }),
            CALL,
            cancel,
        )?;
        let mut items = Vec::new();
        let mut downloads = lock(&self.downloads);
        for v in answer["items"].as_array().into_iter().flatten() {
            if let Some(item) = item_of(v) {
                downloads.insert(item.id.0.clone(), v.clone());
                items.push(item);
            }
        }
        let next = answer["next"].as_str().map(|c| Cursor(c.to_owned()));
        Ok(Page {
            items,
            complete: answer["complete"] == true || next.is_none(),
            next,
        })
    }

    fn access(&self, _item: &ItemRef) -> Access {
        Access::Fetch
    }

    /// The item's preview, when the manifest says `thumbnails`. An
    /// address the service let expire is renewed as for a fetch: the
    /// picking listed again, the preview asked once more.
    fn thumbnail(&self, item: &ItemRef, max_side: u32) -> Result<Option<Vec<u8>>, ConnectorError> {
        if !self.caps.thumbnails {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.staging)
            .map_err(|e| ConnectorError::Fatal(format!("{}: {e}", self.staging.display())))?;
        let bytes = match self.thumbnail_one(item, max_side) {
            Err(ConnectorError::Expired) => {
                self.refresh(&Cancel::new())?;
                self.thumbnail_one(item, max_side)?
            }
            other => other?,
        };
        Ok(Some(bytes))
    }

    fn fetch(
        &self,
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        if offset > 0 {
            return Err(ConnectorError::Unsupported);
        }
        std::fs::create_dir_all(&self.staging)
            .map_err(|e| ConnectorError::Fatal(format!("{}: {e}", self.staging.display())))?;
        // A download address the service let expire (Google's last an
        // hour) is no error: the picking is listed again for fresh ones,
        // and the item fetched once more.
        let answer = match self.fetch_one(item, cancel) {
            Err(ConnectorError::Expired) => {
                self.refresh(cancel)?;
                self.fetch_one(item, cancel)?
            }
            other => other?,
        };
        let done = answer["fetched"]
            .as_array()
            .and_then(|f| f.iter().find(|d| d["id"] == item.id.0.as_str()))
            .ok_or_else(|| ConnectorError::Fatal("the plugin fetched nothing".into()))?;
        let file = staged(&self.staging, done["file"].as_str().unwrap_or_default())?;
        let copied = (|| -> std::io::Result<u64> {
            let mut f = std::fs::File::open(&file)?;
            let (mut buf, mut total) = (vec![0u8; 1 << 16], 0u64);
            loop {
                if cancel.is_cancelled() {
                    return Err(std::io::Error::other("cancelled"));
                }
                let n = f.read(&mut buf)?;
                if n == 0 {
                    return Ok(total);
                }
                sink.accept(&buf[..n])?;
                total += n as u64;
            }
        })();
        let _ = std::fs::remove_file(&file);
        if cancel.is_cancelled() {
            return Err(ConnectorError::Cancelled);
        }
        let bytes = copied.map_err(|e| ConnectorError::Transient(e.to_string()))?;
        Ok(Fetched {
            bytes,
            mime: done["mime"].as_str().map(str::to_owned),
        })
    }
}

/// Picks through a connector that picks: the page opened by `open`, the
/// person waited for until `deadline` (or `cancel`), then every item
/// picked listed. The picking stays open for the import's fetches: end it
/// with [`Connector::end_pick`]. `Ok(None)`: nothing picked by the
/// deadline (the picking ended).
pub fn pick_and_list(
    connector: &dyn Connector,
    account: &AccountRef,
    deadline: Instant,
    cancel: &Cancel,
    open: &dyn Fn(&str),
) -> Result<Option<(SourceRef, Vec<Item>)>, ConnectorError> {
    let pick = connector.pick(account, cancel)?;
    open(&pick.url);
    let listed = connector.picked(deadline, cancel).and_then(|ready| {
        if !ready {
            return Ok(None);
        }
        let mut items = Vec::new();
        let mut from = None;
        loop {
            let page = connector.list(&pick.source, from.as_ref(), helios_connect::PAGE, cancel)?;
            items.extend(page.items);
            match page.next {
                Some(next) if !page.complete => from = Some(next),
                _ => return Ok(Some((pick.source.clone(), items))),
            }
        }
    });
    if !matches!(listed, Ok(Some(_))) {
        let _ = connector.end_pick();
    }
    listed
}
