//! Google Photos as an import source, through the Photos Picker API
//! (`docs/design/import-connectors.md`, section 4.1). Since March 2025 an
//! application reads a person's library only through the Picker: the
//! person picks photos in Google Photos' own page, the application lists
//! and downloads what was picked.
//!
//! The flow, as Google documents it
//! (<https://developers.google.com/photos/picker/guides/get-started-picker>):
//!
//! 1. [`Picker::create`]: `POST v1/sessions` gives a session with its
//!    `pickerUri`, which Helios opens in the system browser
//!    (<https://developers.google.com/photos/picker/reference/rest/v1/sessions/create>).
//! 2. [`Picker::wait`]: `GET v1/sessions/{id}` polled at the interval its
//!    `pollingConfig` asks, until `mediaItemsSet`
//!    (<https://developers.google.com/photos/picker/guides/sessions>).
//! 3. [`Picker::list`]: `GET v1/mediaItems?sessionId=…`, at most 100 items
//!    a page, `nextPageToken` for the next
//!    (<https://developers.google.com/photos/picker/reference/rest/v1/mediaItems/list>).
//! 4. [`Picker::download`]: each item's `baseUrl` with `=d` (a photo, Exif
//!    kept but its location) or `=dv` (a video), with the bearer token;
//!    base URLs last 60 minutes
//!    (<https://developers.google.com/photos/picker/guides/media-items>).
//! 5. [`Picker::delete`]: `DELETE v1/sessions/{id}` once the items are in.
//!
//! The access token is Helios's (`helios_connect::oauth::OAuth::access_token`):
//! the plugin never sees the refresh token. A token Google refuses is
//! [`ConnectorError::AuthNeeded`], never retried: the account needs a
//! reconnection.
//!
//! [`GooglePhotos`] is the [`Connector`] over one picking session, which
//! `helios_library::fetch::import_remote` imports like any fetched
//! source: the source is the account (one `remote` volume per account,
//! so an item picked twice is known by its id), the item id Google's
//! media item id, no claimed hash (Google gives none), bytes re-encoded
//! ([`Originals::Rendered`]).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use helios_connect::{
    Access, AccountRef, Cancel, Caps, Connector, ConnectorError, ConnectorInfo, Cursor, FetchSink,
    Fetched, Item, ItemId, ItemKind, ItemRef, Originals, Page, Probed, SourceRef, Text,
};
use serde_json::{Value, json};

/// The connector's id.
pub const ID: &str = "google-photos";

/// Where the Picker API answers.
pub const API: &str = "https://photospicker.googleapis.com/v1";

/// The only scope the plugin asks for: read the items the person picks.
pub const SCOPE: &str = "https://www.googleapis.com/auth/photospicker.mediaitems.readonly";

/// The most items a list page holds (the API coerces larger sizes).
pub const PAGE_MAX: usize = 100;

/// How long a picking waits for the person at most when the session
/// gives no `timeoutIn`.
const WAIT: Duration = Duration::from_mins(30);

/// A picking session, as the Picker answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    /// The page the person picks on, to open in a browser.
    pub picker_uri: String,
    /// The person finished picking.
    pub items_set: bool,
    /// How often to poll, while not set.
    pub poll_every: Option<Duration>,
    /// How long polling makes sense, while not set; zero: stop.
    pub poll_for: Option<Duration>,
    /// When the session and its items expire (RFC 3339).
    pub expires: Option<String>,
}

impl Session {
    fn of(v: &Value) -> Result<Self, ConnectorError> {
        let id = v["id"]
            .as_str()
            .ok_or_else(|| ConnectorError::Fatal("the Picker's session has no id".into()))?;
        Ok(Self {
            id: id.into(),
            picker_uri: v["pickerUri"].as_str().unwrap_or_default().into(),
            items_set: v["mediaItemsSet"].as_bool().unwrap_or(false),
            poll_every: v["pollingConfig"]["pollInterval"]
                .as_str()
                .and_then(duration),
            poll_for: v["pollingConfig"]["timeoutIn"].as_str().and_then(duration),
            expires: v["expireTime"].as_str().map(String::from),
        })
    }

    /// The picking page, closing its tab once the person is done.
    pub fn autoclose_uri(&self) -> String {
        format!("{}/autoclose", self.picker_uri.trim_end_matches('/'))
    }
}

/// Where an item's bytes are fetched from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    /// The item's `baseUrl`, without a suffix.
    pub base_url: String,
    pub video: bool,
}

impl Download {
    /// The original's address: `=d` for a photo, `=dv` for a video.
    pub fn url(&self) -> String {
        format!("{}={}", self.base_url, if self.video { "dv" } else { "d" })
    }
}

/// A protobuf duration as JSON writes it: `"3.5s"`.
fn duration(s: &str) -> Option<Duration> {
    let secs: f64 = s.trim().strip_suffix('s')?.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Seconds since 1970 of an RFC 3339 time in UTC (`2024-05-01T10:20:30.5Z`,
/// or with an offset).
pub fn epoch(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time_end = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(time_end);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t.next()?.split('.').next()?.parse().ok()?;
    let offset = match zone.as_bytes().first() {
        Some(b'+' | b'-') => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (oh, om) = zone[1..].split_once(':')?;
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + sec - offset)
}

/// A JSON number or a string holding one (protobuf writes 64-bit
/// integers as strings), as text.
fn number(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// A picked media item as an [`Item`] and where its bytes are. The
/// metadata Google gives goes in under XMP names, as the source's
/// opinion: the file's own Exif wins once it is fetched.
pub fn item_of(v: &Value) -> Option<(Item, Download)> {
    let id = v["id"].as_str()?;
    let file = &v["mediaFile"];
    let base_url = file["baseUrl"].as_str()?;
    let video = v["type"].as_str() == Some("VIDEO");
    let mime = file["mimeType"].as_str().map(String::from);
    let meta = &file["mediaFileMetadata"];
    let photo = &meta["photoMetadata"];
    let mut metadata = BTreeMap::new();
    for (key, value) in [
        ("tiff:Make", &meta["cameraMake"]),
        ("tiff:Model", &meta["cameraModel"]),
        ("exif:PixelXDimension", &meta["width"]),
        ("exif:PixelYDimension", &meta["height"]),
        ("exif:FocalLength", &photo["focalLength"]),
        ("exif:FNumber", &photo["apertureFNumber"]),
        ("exif:ISOSpeedRatings", &photo["isoEquivalent"]),
    ] {
        if let Some(text) = number(value) {
            metadata.insert(key.to_string(), text);
        }
    }
    if let Some(t) = photo["exposureTime"].as_str().and_then(duration) {
        metadata.insert("exif:ExposureTime".into(), t.as_secs_f64().to_string());
    }
    let kind = match mime.as_deref() {
        _ if video => ItemKind::Video,
        Some(m) if m.starts_with("image/x-") || m == "image/dng" => ItemKind::Raw,
        _ => ItemKind::Photo,
    };
    let taken = v["createTime"].as_str().and_then(epoch);
    let item = Item {
        id: ItemId(id.into()),
        path: file["filename"].as_str().unwrap_or(id).into(),
        size: None,
        modified: taken,
        taken,
        kind,
        mime,
        metadata,
        claim: None,
        revision: None,
        group: None,
    };
    Some((
        item,
        Download {
            base_url: base_url.into(),
            video,
        },
    ))
}

/// Why an answer of `status` failed. A refused token is never retried:
/// the account needs a reconnection.
fn failure(
    status: u16,
    retry_after: Option<&str>,
    body: &str,
    gone: ConnectorError,
) -> ConnectorError {
    match status {
        401 | 403 => ConnectorError::AuthNeeded,
        404 | 410 => gone,
        429 => ConnectorError::RateLimited {
            retry_after: retry_after
                .and_then(|s| s.trim().parse().ok())
                .map(Duration::from_secs),
        },
        400 if body.contains("FAILED_PRECONDITION") => {
            ConnectorError::Transient("the picking is not finished".into())
        }
        500..=599 => ConnectorError::Transient(format!("Google Photos answered {status}")),
        _ => ConnectorError::Fatal(format!(
            "Google Photos answered {status}: {}",
            body.chars().take(200).collect::<String>()
        )),
    }
}

/// The Picker API under one access token.
pub struct Picker {
    api: String,
    token: String,
    http: ureq::Agent,
}

impl Picker {
    /// `api`: the API's base address ([`API`], another for tests).
    pub fn new(api: &str, token: &str) -> Self {
        let http = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_mins(1)))
            .build()
            .into();
        Self {
            api: api.trim_end_matches('/').into(),
            token: token.into(),
            http,
        }
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// One API call: its JSON answer, or why it failed.
    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        gone: ConnectorError,
    ) -> Result<Value, ConnectorError> {
        let url = format!("{}/{path}", self.api);
        let sent = match (method, body) {
            ("POST", Some(b)) => self
                .http
                .post(&url)
                .header("Authorization", &self.bearer())
                .header("Content-Type", "application/json")
                .send(b.to_string().as_bytes()),
            ("DELETE", _) => self
                .http
                .delete(&url)
                .header("Authorization", &self.bearer())
                .call(),
            _ => self
                .http
                .get(&url)
                .header("Authorization", &self.bearer())
                .call(),
        };
        let mut response = sent.map_err(|e| match e {
            ureq::Error::Timeout(_) => ConnectorError::Transient(format!("{e}")),
            _ => ConnectorError::Offline,
        })?;
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get("Retry-After")
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| ConnectorError::Transient(format!("the Picker's answer: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(failure(status, retry.as_deref(), &text, gone));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| ConnectorError::Fatal(format!("the Picker's answer: {e}")))
    }

    /// A new picking session; `max_items` caps what the person may pick
    /// (Google's ceiling is 2,000, its default).
    pub fn create(&self, max_items: Option<u32>) -> Result<Session, ConnectorError> {
        let body = match max_items {
            Some(n) => json!({ "pickingConfig": { "maxItemCount": n.to_string() } }),
            None => json!({}),
        };
        Session::of(&self.call("POST", "sessions", Some(&body), ConnectorError::NotFound)?)
    }

    /// A session as it stands.
    pub fn session(&self, id: &str) -> Result<Session, ConnectorError> {
        Session::of(&self.call("GET", &format!("sessions/{id}"), None, ConnectorError::Gone)?)
    }

    /// Polls a session until the person finished picking, at the interval
    /// the session asks, for as long as it says (`timeoutIn`), and never
    /// past `deadline`. `Ok(None)`: the time ran out with nothing picked.
    pub fn wait(
        &self,
        id: &str,
        deadline: Option<Duration>,
        cancel: &Cancel,
    ) -> Result<Option<Session>, ConnectorError> {
        let started = Instant::now();
        let mut until = deadline.unwrap_or(WAIT);
        loop {
            if cancel.is_cancelled() {
                return Err(ConnectorError::Cancelled);
            }
            let session = self.session(id)?;
            if session.items_set {
                return Ok(Some(session));
            }
            if let Some(timeout) = session.poll_for {
                if timeout.is_zero() {
                    return Ok(None);
                }
                until = until.min(started.elapsed() + timeout);
            }
            let every = session
                .poll_every
                .unwrap_or(Duration::from_secs(5))
                .max(Duration::from_millis(5));
            if started.elapsed() + every > until {
                return Ok(None);
            }
            std::thread::sleep(every);
        }
    }

    /// One page of the items picked in a session, and the next page's
    /// token.
    pub fn list(
        &self,
        session: &str,
        page_token: Option<&str>,
        page_size: usize,
    ) -> Result<(Vec<Value>, Option<String>), ConnectorError> {
        let mut path = format!(
            "mediaItems?sessionId={}&pageSize={}",
            query(session),
            page_size.clamp(1, PAGE_MAX)
        );
        if let Some(t) = page_token {
            path = format!("{path}&pageToken={}", query(t));
        }
        let answer = self.call("GET", &path, None, ConnectorError::Gone)?;
        let items = answer["mediaItems"].as_array().cloned().unwrap_or_default();
        let next = answer["nextPageToken"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(String::from);
        Ok((items, next))
    }

    /// Deletes a session once its items are in (Google asks for it).
    pub fn delete(&self, id: &str) -> Result<(), ConnectorError> {
        self.call(
            "DELETE",
            &format!("sessions/{id}"),
            None,
            ConnectorError::Gone,
        )
        .map(|_| ())
    }

    /// Writes an item's original into `sink`. An expired base URL is
    /// [`ConnectorError::Expired`]: listing again gives fresh ones.
    pub fn download(
        &self,
        download: &Download,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        let mut response = self
            .http
            .get(&download.url())
            .header("Authorization", &self.bearer())
            .call()
            .map_err(|_| ConnectorError::Offline)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let retry = response
                .headers()
                .get("Retry-After")
                .and_then(|h| h.to_str().ok())
                .map(String::from);
            let text = response.body_mut().read_to_string().unwrap_or_default();
            return Err(failure(
                status,
                retry.as_deref(),
                &text,
                ConnectorError::Expired,
            ));
        }
        let mime = response
            .headers()
            .get("Content-Type")
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut bytes = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(ConnectorError::Cancelled);
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| ConnectorError::Transient(format!("the download was cut: {e}")))?;
            if n == 0 {
                break;
            }
            sink.accept(&buf[..n])
                .map_err(|e| ConnectorError::Fatal(format!("writing the download: {e}")))?;
            bytes += n as u64;
        }
        Ok(Fetched { bytes, mime })
    }
}

/// A query value, percent-encoded.
fn query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The source an account's picked items come from: the account itself,
/// so the same media item picked in two sessions is one sighting.
pub fn source(account: &str, label: &str) -> SourceRef {
    SourceRef {
        connector: ID.into(),
        id: account.into(),
        label: label.into(),
        root: None,
        capacity: None,
    }
}

/// Google Photos as a [`Connector`]: the items picked in one session.
pub struct GooglePhotos {
    picker: Picker,
    session: String,
    /// Where each item listed is downloaded from.
    downloads: Mutex<HashMap<String, Download>>,
}

impl GooglePhotos {
    pub fn new(picker: Picker, session: &str) -> Self {
        Self {
            picker,
            session: session.into(),
            downloads: Mutex::default(),
        }
    }

    pub fn picker(&self) -> &Picker {
        &self.picker
    }

    /// Every item picked, every page listed.
    pub fn list_all(
        &self,
        source: &SourceRef,
        cancel: &Cancel,
    ) -> Result<Vec<Item>, ConnectorError> {
        let mut items = Vec::new();
        let mut from = None;
        loop {
            let page = self.list(source, from.as_ref(), PAGE_MAX, cancel)?;
            items.extend(page.items);
            match page.next {
                Some(next) => from = Some(next),
                None => return Ok(items),
            }
        }
    }
}

impl Connector for GooglePhotos {
    fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: ID.into(),
            label: Text::new("connect-google-photos", "Google Photos"),
            caps: Caps {
                accounts: true,
                originals: Originals::Rendered,
                ..Caps::default()
            },
        }
    }

    fn sources(&self, account: Option<&AccountRef>) -> Result<Vec<SourceRef>, ConnectorError> {
        Ok(account
            .map(|a| vec![source(&a.id, "Google Photos")])
            .unwrap_or_default())
    }

    fn list(
        &self,
        _source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        if cancel.is_cancelled() {
            return Err(ConnectorError::Cancelled);
        }
        let (values, next) = self
            .picker
            .list(&self.session, from.map(|c| c.0.as_str()), page)?;
        let mut items = Vec::with_capacity(values.len());
        let mut downloads = self
            .downloads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (item, download) in values.iter().filter_map(item_of) {
            downloads.insert(item.id.0.clone(), download);
            items.push(item);
        }
        Ok(Page {
            items,
            complete: next.is_none(),
            next: next.map(Cursor),
        })
    }

    /// A session created then deleted: the token is good for the Picker.
    fn probe(
        &self,
        _account: Option<&AccountRef>,
        _cancel: &Cancel,
    ) -> Result<Probed, ConnectorError> {
        let session = self.picker.create(Some(1))?;
        let _ = self.picker.delete(&session.id);
        Ok(Probed::default())
    }

    fn access(&self, _item: &ItemRef) -> Access {
        Access::Fetch
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
        let download = self
            .downloads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&item.id.0)
            .cloned()
            .ok_or(ConnectorError::Expired)?;
        self.picker.download(&download, sink, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_times_and_durations_read() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2024-05-01T10:20:30.123456Z"), Some(1_714_558_830));
        assert_eq!(epoch("2024-05-01T12:20:30+02:00"), Some(1_714_558_830));
        assert_eq!(duration("3.5s"), Some(Duration::from_millis(3500)));
        assert_eq!(duration("nope"), None);
    }

    #[test]
    fn a_picked_item_maps_to_an_item_with_its_metadata() {
        let v = json!({
            "id": "m1", "createTime": "2024-05-01T10:20:30Z", "type": "PHOTO",
            "mediaFile": {
                "baseUrl": "https://lh3.example/abc", "mimeType": "image/jpeg",
                "filename": "IMG_1.jpg",
                "mediaFileMetadata": {
                    "width": "4032", "height": 3024, "cameraMake": "Google",
                    "cameraModel": "Pixel 8",
                    "photoMetadata": { "focalLength": 6.9, "apertureFNumber": 1.7,
                        "isoEquivalent": 50, "exposureTime": "0.008s" }
                }
            }
        });
        let (item, download) = item_of(&v).unwrap();
        assert_eq!(item.path, "IMG_1.jpg");
        assert_eq!(item.taken, Some(1_714_558_830));
        assert_eq!(item.kind, ItemKind::Photo);
        assert_eq!(item.metadata["tiff:Model"], "Pixel 8");
        assert_eq!(item.metadata["exif:PixelXDimension"], "4032");
        assert_eq!(item.metadata["exif:PixelYDimension"], "3024");
        assert_eq!(item.metadata["exif:ExposureTime"], "0.008");
        assert_eq!(item.claim, None);
        assert_eq!(download.url(), "https://lh3.example/abc=d");
        let video = json!({"id": "v", "type": "VIDEO", "mediaFile": {"baseUrl": "u"}});
        let (item, download) = item_of(&video).unwrap();
        assert_eq!(
            (item.kind, download.url()),
            (ItemKind::Video, "u=dv".into())
        );
    }
}
