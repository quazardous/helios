//! The OAuth flow (`oauth::OAuth`) against a fake authorisation server
//! on a local port: PKCE verified by the server, the state checked, a
//! refused consent, a token renewed when it expired, a renewal refused
//! leaving the account needing attention, and signing out revoking the
//! token and forgetting it. The "browser" is a plain HTTP client that
//! follows the consent page's redirect to Helios's loopback; no real
//! browser is opened.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_connect::account::{
    Account, AccountBook, AccountState, MemoryAccounts, MemorySecrets, Secrets,
};
use helios_connect::oauth::{self, OAuth};
use helios_connect::{Cancel, StepKind};

const CONNECTOR: &str = "fake-photos";
const CLIENT: &str = "client-123";

/// How the fake consent page answers.
#[derive(Clone, Copy, PartialEq)]
enum Consent {
    Grant,
    Deny,
    /// Answers with a state other than the one asked.
    ForgedState,
}

#[derive(Default)]
struct Seen {
    challenge: Option<String>,
    method: Option<String>,
    redirect: Option<String>,
    client: Option<String>,
    /// The refresh token the server still honours.
    live_refresh: Option<String>,
    revoked: Vec<String>,
    refreshes: usize,
}

struct Server {
    base: String,
    seen: Arc<Mutex<Seen>>,
}

fn read_request(stream: &mut TcpStream) -> (String, String, String) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break data.len();
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let length: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);
    while data.len() < head_end + length {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let body = String::from_utf8_lossy(&data[head_end..]).to_string();
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_owned();
    let target = first.next().unwrap_or("").to_owned();
    (method, target, body)
}

fn reply(stream: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn pairs(s: &str) -> BTreeMap<String, String> {
    oauth::query_pairs(s).into_iter().collect()
}

impl Server {
    fn start(consent: Consent) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let s = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (method, target, body) = read_request(&mut stream);
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let mut seen = s.lock().unwrap();
                match (method.as_str(), path) {
                    ("GET", "/authorize") => {
                        let q = pairs(query);
                        seen.challenge = q.get("code_challenge").cloned();
                        seen.method = q.get("code_challenge_method").cloned();
                        seen.redirect = q.get("redirect_uri").cloned();
                        seen.client = q.get("client_id").cloned();
                        let redirect = q["redirect_uri"].clone();
                        let state = q["state"].clone();
                        let to = match consent {
                            Consent::Grant => {
                                oauth::with_query(&redirect, &[("code", "c0de"), ("state", &state)])
                            }
                            Consent::Deny => oauth::with_query(
                                &redirect,
                                &[("error", "access_denied"), ("state", &state)],
                            ),
                            Consent::ForgedState => oauth::with_query(
                                &redirect,
                                &[("code", "c0de"), ("state", "forged")],
                            ),
                        };
                        reply(&mut stream, "302 Found", &format!("Location: {to}\r\n"), "");
                    }
                    ("POST", "/token") => {
                        let f = pairs(&body);
                        let grant = f.get("grant_type").map(String::as_str);
                        let ok = match grant {
                            Some("authorization_code") => {
                                f.get("code").map(String::as_str) == Some("c0de")
                                    && f.get("client_id") == seen.client.as_ref()
                                    && f.get("redirect_uri") == seen.redirect.as_ref()
                                    && f.get("code_verifier")
                                        .map(|v| oauth::challenge_of(v.as_str()))
                                        == seen.challenge
                            }
                            Some("refresh_token") => {
                                seen.refreshes += 1;
                                f.contains_key("refresh_token")
                                    && f.get("refresh_token") == seen.live_refresh.as_ref()
                                    && f.get("client_id").map(String::as_str) == Some(CLIENT)
                            }
                            _ => false,
                        };
                        if ok && grant == Some("authorization_code") {
                            seen.live_refresh = Some("refresh-1".into());
                            reply(
                                &mut stream,
                                "200 OK",
                                "",
                                r#"{"access_token":"access-1","expires_in":3600,"refresh_token":"refresh-1","token_type":"Bearer"}"#,
                            );
                        } else if ok {
                            reply(
                                &mut stream,
                                "200 OK",
                                "",
                                r#"{"access_token":"access-2","expires_in":3600,"token_type":"Bearer"}"#,
                            );
                        } else {
                            reply(
                                &mut stream,
                                "400 Bad Request",
                                "",
                                r#"{"error":"invalid_grant"}"#,
                            );
                        }
                    }
                    ("POST", "/revoke") => {
                        let f = pairs(&body);
                        if let Some(t) = f.get("token") {
                            seen.revoked.push(t.clone());
                            if seen.live_refresh.as_ref() == Some(t) {
                                seen.live_refresh = None;
                            }
                        }
                        reply(&mut stream, "200 OK", "", "");
                    }
                    _ => reply(&mut stream, "404 Not Found", "", ""),
                }
            }
        });
        Self { base, seen }
    }

    fn step(&self) -> StepKind {
        StepKind::Oauth {
            authorize: format!("{}/authorize", self.base),
            token: format!("{}/token", self.base),
            scopes: vec!["photos.read".into(), "openid".into()],
            revoke: Some(format!("{}/revoke", self.base)),
            client_id: None,
        }
    }
}

/// The flow with the client id given by the environment.
fn flow() -> OAuth {
    OAuth::with_env(Arc::new(|name| {
        (name == "HELIOS_FAKE_PHOTOS_CLIENT_ID").then(|| CLIENT.to_owned())
    }))
    .waiting(Duration::from_secs(20))
}

/// A browser that follows the consent page's redirect, off the flow's
/// thread (the flow answers the loopback only once it is called back).
fn browser(opened: &Mutex<Option<String>>) -> impl Fn(&str) + '_ {
    move |url: &str| {
        *opened.lock().unwrap() = Some(url.to_owned());
        let url = url.to_owned();
        std::thread::spawn(move || {
            let _ = ureq::get(&url).call();
        });
    }
}

struct World {
    book: MemoryAccounts,
    secrets: MemorySecrets,
    account: Account,
}

fn world() -> World {
    let mut account = Account::new(CONNECTOR, "Fake Photos", &[]);
    account.state = AccountState::Connecting {
        step: "sign-in".into(),
    };
    World {
        book: MemoryAccounts::default(),
        secrets: MemorySecrets::default(),
        account,
    }
}

fn sign_in(server: &Server, w: &mut World) -> (Result<(), String>, String) {
    let opened = Mutex::new(None);
    let done = flow().sign_in(
        &mut w.account,
        &server.step(),
        &w.secrets,
        &browser(&opened),
        &Cancel::new(),
    );
    let url = opened.lock().unwrap().clone().unwrap_or_default();
    (done, url)
}

#[test]
fn a_granted_consent_keeps_the_tokens_in_the_keyring_after_pkce_checks() {
    let server = Server::start(Consent::Grant);
    let mut w = world();
    let (done, url) = sign_in(&server, &mut w);
    assert_eq!(done, Ok(()));
    // The consent page was asked with S256 PKCE, the scopes and a
    // loopback redirect on an ephemeral port.
    let q = pairs(url.split_once('?').unwrap().1);
    assert_eq!(q["scope"], "photos.read openid");
    assert_eq!(q["response_type"], "code");
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.method.as_deref(), Some("S256"));
    assert!(
        seen.redirect
            .as_deref()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    assert_eq!(seen.client.as_deref(), Some(CLIENT));
    drop(seen);
    let id = w.account.id.clone();
    assert_eq!(
        w.secrets.get(&id, oauth::REFRESH_TOKEN).unwrap().as_deref(),
        Some("refresh-1")
    );
    assert_eq!(
        w.secrets.get(&id, oauth::ACCESS_TOKEN).unwrap().as_deref(),
        Some("access-1")
    );
    // The account's document holds where tokens renew, never a token.
    let doc = w.account.to_document().to_string();
    assert!(!doc.contains("refresh-1") && !doc.contains("access-1"));
    assert_eq!(w.account.facts[oauth::STEP], "sign-in");
    // A valid access token is given as kept, without a call.
    w.account.state = AccountState::Connected;
    w.book.save(&w.account).unwrap();
    assert_eq!(
        flow().access_token(&w.book, &w.secrets, &id).unwrap(),
        "access-1"
    );
    assert_eq!(server.seen.lock().unwrap().refreshes, 0);
}

#[test]
fn a_refused_consent_or_a_forged_state_fails_the_step_and_keeps_nothing() {
    for (consent, said) in [
        (Consent::Deny, "refused on the consent page"),
        (Consent::ForgedState, "state"),
    ] {
        let server = Server::start(consent);
        let mut w = world();
        let (done, _) = sign_in(&server, &mut w);
        let why = done.unwrap_err();
        assert!(why.contains(said), "{why}");
        assert!(w.account.secrets.is_empty());
        // The forged answer's code never reached the token endpoint.
        assert!(server.seen.lock().unwrap().live_refresh.is_none());
    }
}

#[test]
fn a_missing_client_says_where_to_set_one_in_settings() {
    let server = Server::start(Consent::Grant);
    let mut w = world();
    let opened = Mutex::new(None);
    let done = OAuth::with_env(Arc::new(|_| None))
        .with_built_in(Arc::new(|_| None))
        .sign_in(
            &mut w.account,
            &server.step(),
            &w.secrets,
            &browser(&opened),
            &Cancel::new(),
        );
    let why = done.unwrap_err();
    assert!(why.contains("Use my own OAuth client"), "{why}");
    assert!(
        opened.lock().unwrap().is_none(),
        "no browser without a client id"
    );
}

#[test]
fn an_expired_access_token_is_renewed_and_a_refused_renewal_needs_attention() {
    let server = Server::start(Consent::Grant);
    let mut w = world();
    sign_in(&server, &mut w).0.unwrap();
    w.account.state = AccountState::Connected;
    w.book.save(&w.account).unwrap();
    let id = w.account.id.clone();
    w.secrets.set(&id, oauth::ACCESS_EXPIRES, "1").unwrap();
    assert_eq!(
        flow().access_token(&w.book, &w.secrets, &id).unwrap(),
        "access-2"
    );
    assert_eq!(server.seen.lock().unwrap().refreshes, 1);
    // The refresh token is kept when the renewal hands none.
    assert_eq!(
        w.secrets.get(&id, oauth::REFRESH_TOKEN).unwrap().as_deref(),
        Some("refresh-1")
    );
    assert_eq!(
        w.book.account(&id).unwrap().unwrap().state,
        AccountState::Connected
    );

    // The person revoked Helios's access on the service's side.
    server.seen.lock().unwrap().live_refresh = None;
    w.secrets.set(&id, oauth::ACCESS_EXPIRES, "1").unwrap();
    let why = flow().access_token(&w.book, &w.secrets, &id).unwrap_err();
    assert!(why.contains("sign in again"), "{why}");
    match w.book.account(&id).unwrap().unwrap().state {
        AccountState::NeedsAttention { step, .. } => assert_eq!(step.as_deref(), Some("sign-in")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn signing_out_revokes_the_token_and_forgets_it() {
    let server = Server::start(Consent::Grant);
    let mut w = world();
    sign_in(&server, &mut w).0.unwrap();
    w.account.state = AccountState::Connected;
    w.book.save(&w.account).unwrap();
    let id = w.account.id.clone();
    let out = flow().sign_out(&w.book, &w.secrets, &id).unwrap();
    assert_eq!(out.state, AccountState::NotConnected);
    assert_eq!(server.seen.lock().unwrap().revoked, ["refresh-1"]);
    for name in [
        oauth::REFRESH_TOKEN,
        oauth::ACCESS_TOKEN,
        oauth::ACCESS_EXPIRES,
    ] {
        assert_eq!(w.secrets.get(&id, name).unwrap(), None, "{name}");
    }
    // Signed out, a token is asked for in vain: sign in again.
    assert!(flow().access_token(&w.book, &w.secrets, &id).is_err());
}

#[test]
fn the_person_s_own_client_signs_in_over_the_environment_s() {
    let server = Server::start(Consent::Grant);
    let mut w = world();
    let opened = Mutex::new(None);
    let mine = oauth::Client::of(Some("own-client"), Some("own-secret")).unwrap();
    let own = mine.clone();
    let done = flow()
        .with_own(Arc::new(move |c| (c == CONNECTOR).then(|| own.clone())))
        .sign_in(
            &mut w.account,
            &server.step(),
            &w.secrets,
            &browser(&opened),
            &Cancel::new(),
        );
    assert_eq!(done, Ok(()));
    assert_eq!(
        server.seen.lock().unwrap().client.as_deref(),
        Some("own-client")
    );
    assert_eq!(w.account.facts[oauth::CLIENT_ID], "own-client");
}
