//! Signing in to a service with OAuth 2.0 (`docs/design/import-connectors.md`,
//! section 2.4.2): the authorisation-code flow with PKCE for installed
//! applications, as an `oauth` connection step runs it.
//!
//! [`OAuth`] is the [`WebSteps`] the desktop gives a connection: it makes
//! a code verifier and its S256 challenge, listens on a loopback port
//! (`http://127.0.0.1:<port>`, chosen by the system), has the system
//! browser opened on the consent page, takes the code the service sends
//! back there (its `state` checked), and exchanges it for tokens. The
//! refresh token and the access token go to [`Secrets`] (the system
//! keyring), never to the vault; the account keeps only where they are
//! renewed and revoked ([`TOKEN_URL`], [`REVOKE_URL`], [`CLIENT_ID`]).
//!
//! [`OAuth::access_token`] gives a connector a valid access token,
//! renewed when it expired; a refresh the service refuses leaves the
//! account needing attention at its `oauth` step. [`OAuth::sign_out`]
//! revokes the token where the service can, then forgets it.
//!
//! The client id is never built in: it comes from the environment
//! (`HELIOS_<CONNECTOR>_CLIENT_ID`, the connector id upper-cased with `-`
//! as `_`), then the account's own (a form field `client_id`, as rclone
//! asks), then the connector's manifest. Without one the step fails and
//! says which variable to set. A client secret, which Google issues to
//! desktop clients without treating it as confidential, is read the same
//! way (`HELIOS_<CONNECTOR>_CLIENT_SECRET`, or a secret `client_secret`).

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Cancel;
use crate::StepKind;
use crate::account::{Account, AccountBook, AccountState, Secrets};
use crate::connect::WebSteps;

/// The secret holding the refresh token.
pub const REFRESH_TOKEN: &str = "refresh_token";
/// The secret holding the access token.
pub const ACCESS_TOKEN: &str = "access_token";
/// The secret holding when the access token expires (Unix seconds).
pub const ACCESS_EXPIRES: &str = "access_expires";
/// The account fact naming the token endpoint, for renewals.
pub const TOKEN_URL: &str = "oauth_token_url";
/// The account fact naming the revocation endpoint.
pub const REVOKE_URL: &str = "oauth_revoke_url";
/// The account fact holding the client id used (public by nature).
pub const CLIENT_ID: &str = "client_id";
/// The account fact naming its `oauth` step, so an expired sign-in
/// replays from it.
pub const STEP: &str = "oauth_step";

/// An access token this close to its end is renewed first.
const MARGIN: u64 = 60;
/// How often the loopback wait looks at its cancel flag.
const POLL: Duration = Duration::from_millis(50);

/// Reads an environment variable (the process's own, or a test's map).
pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The OAuth flow and its token keeping.
#[derive(Clone)]
pub struct OAuth {
    env: Env,
    /// How long the consent page is waited for.
    wait: Duration,
    http: ureq::Agent,
}

impl OAuth {
    /// The flow reading the process's environment.
    pub fn system() -> Self {
        Self::with_env(Arc::new(|name| std::env::var(name).ok()))
    }

    /// The flow reading `env` for client ids.
    pub fn with_env(env: Env) -> Self {
        let http = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            env,
            wait: Duration::from_mins(10),
            http,
        }
    }

    /// The consent page is waited for at most `wait`.
    pub fn waiting(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    fn client_id(&self, account: &Account, declared: Option<&String>) -> Result<String, String> {
        let var = env_name(&account.connector, "CLIENT_ID");
        (self.env)(&var)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                account
                    .facts
                    .get(CLIENT_ID)
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
            })
            .or_else(|| declared.cloned())
            .map(|s| s.trim().to_owned())
            .ok_or_else(|| {
                format!(
                    "no OAuth client id for {}: set the environment variable {var} \
                     to the client id of a desktop application",
                    account.connector
                )
            })
    }

    fn client_secret(&self, account: &Account, secrets: &dyn Secrets) -> Option<String> {
        (self.env)(&env_name(&account.connector, "CLIENT_SECRET"))
            .or_else(|| secrets.get(&account.id, "client_secret").ok().flatten())
            .filter(|s| !s.trim().is_empty())
    }

    /// Runs the flow for `account`: tokens kept in `secrets`, where they
    /// renew in its facts.
    pub fn sign_in(
        &self,
        account: &mut Account,
        step: &StepKind,
        secrets: &dyn Secrets,
        browser: &dyn Fn(&str),
        cancel: &Cancel,
    ) -> Result<(), String> {
        let StepKind::Oauth {
            authorize,
            token,
            scopes,
            revoke,
            client_id,
        } = step
        else {
            return Err("not an oauth step".into());
        };
        let client = self.client_id(account, client_id.as_ref())?;
        let verifier = random(32)?;
        let challenge = challenge_of(&verifier);
        let state = random(16)?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("cannot listen for the sign-in's answer: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let redirect = format!("http://127.0.0.1:{port}");
        let url = with_query(
            authorize,
            &[
                ("response_type", "code"),
                ("client_id", &client),
                ("redirect_uri", &redirect),
                ("scope", &scopes.join(" ")),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("state", &state),
            ],
        );
        browser(&url);
        let code = wait_for_code(&listener, &state, self.wait, cancel)?;
        let mut form = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code),
            ("redirect_uri", redirect),
            ("client_id", client.clone()),
            ("code_verifier", verifier),
        ];
        if let Some(s) = self.client_secret(account, secrets) {
            form.push(("client_secret", s));
        }
        let tokens = self.post(token, &form)?.map_err(|e| e.said())?;
        if let AccountState::Connecting { step } = &account.state {
            account.facts.insert(STEP.into(), step.clone().into());
        }
        account.facts.insert(TOKEN_URL.into(), token.clone().into());
        account.facts.insert(CLIENT_ID.into(), client.into());
        match revoke {
            Some(r) => account.facts.insert(REVOKE_URL.into(), r.clone().into()),
            None => account.facts.remove(REVOKE_URL),
        };
        keep(account, secrets, &tokens)
    }

    /// A valid access token for the account `id`, renewed with its
    /// refresh token when it expired. A renewal the service refuses (the
    /// person revoked the access, the token aged out) leaves the account
    /// needing attention at its `oauth` step.
    pub fn access_token(
        &self,
        book: &dyn AccountBook,
        secrets: &dyn Secrets,
        id: &str,
    ) -> Result<String, String> {
        let mut account = book
            .account(id)?
            .ok_or_else(|| format!("no account {id}"))?;
        let expires: u64 = secrets
            .get(id, ACCESS_EXPIRES)?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if let Some(access) = secrets.get(id, ACCESS_TOKEN)?
            && expires > now() + MARGIN
        {
            return Ok(access);
        }
        let expired = |account: &mut Account, why: &str| {
            account.state = AccountState::NeedsAttention {
                step: account
                    .facts
                    .get(STEP)
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                why: why.to_owned(),
            };
            let _ = book.save(account);
            why.to_owned()
        };
        let Some(refresh) = secrets.get(id, REFRESH_TOKEN)? else {
            return Err(expired(&mut account, "signed out: sign in again"));
        };
        let fact = |k: &str| {
            account
                .facts
                .get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let (Some(token), Some(client)) = (fact(TOKEN_URL), fact(CLIENT_ID)) else {
            return Err(expired(
                &mut account,
                "the sign-in is incomplete: sign in again",
            ));
        };
        let mut form = vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh),
            ("client_id", client),
        ];
        if let Some(s) = self.client_secret(&account, secrets) {
            form.push(("client_secret", s));
        }
        match self.post(&token, &form)? {
            Ok(tokens) => {
                keep(&mut account, secrets, &tokens)?;
                book.save(&account)?;
                secrets
                    .get(id, ACCESS_TOKEN)?
                    .ok_or_else(|| "the service gave no access token".into())
            }
            Err(e) if e.code == "invalid_grant" => Err(expired(
                &mut account,
                "the sign-in expired or was revoked: sign in again",
            )),
            Err(e) => Err(e.said()),
        }
    }

    /// Signs the account `id` out: its token revoked where the service
    /// can (a failure there does not stop it), then its secrets
    /// forgotten and its state "not connected".
    pub fn sign_out(
        &self,
        book: &dyn AccountBook,
        secrets: &dyn Secrets,
        id: &str,
    ) -> Result<Account, String> {
        if let Some(account) = book.account(id)?
            && let Some(url) = account.facts.get(REVOKE_URL).and_then(Value::as_str)
        {
            let token = match secrets.get(id, REFRESH_TOKEN)? {
                Some(t) => Some(t),
                None => secrets.get(id, ACCESS_TOKEN)?,
            };
            if let Some(token) = token
                && let Err(e) = self
                    .post(url, &[("token", token)])
                    .and_then(|r| r.map_err(|e| e.said()))
            {
                log::warn!("signing {id} out: the token was not revoked: {e}");
            }
        }
        crate::account::sign_out(book, secrets, id)
    }

    /// Posts a form; the service's JSON answer, or its OAuth error.
    fn post(
        &self,
        url: &str,
        form: &[(&str, String)],
    ) -> Result<Result<Value, ServiceError>, String> {
        let mut response = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .send_form(form.iter().map(|(k, v)| (*k, v.as_str())))
            .map_err(|e| format!("the service cannot be reached: {e}"))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("the service's answer: {e}"))?;
        let json: Value = if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&body).unwrap_or(Value::Null)
        };
        if (200..300).contains(&status) {
            return Ok(Ok(json));
        }
        let text = |k: &str| json.get(k).and_then(Value::as_str).map(str::to_owned);
        Ok(Err(ServiceError {
            code: text("error").unwrap_or_else(|| format!("http {status}")),
            description: text("error_description"),
        }))
    }
}

impl WebSteps for OAuth {
    fn oauth(
        &self,
        account: &mut Account,
        step: &StepKind,
        secrets: &dyn Secrets,
        browser: &dyn Fn(&str),
        cancel: &Cancel,
    ) -> Result<(), String> {
        self.sign_in(account, step, secrets, browser, cancel)
    }

    fn sign_out(
        &self,
        book: &dyn AccountBook,
        secrets: &dyn Secrets,
        id: &str,
    ) -> Result<Account, String> {
        OAuth::sign_out(self, book, secrets, id)
    }

    fn access_token(
        &self,
        book: &dyn AccountBook,
        secrets: &dyn Secrets,
        id: &str,
    ) -> Result<String, String> {
        OAuth::access_token(self, book, secrets, id)
    }
}

/// An OAuth error a service answered.
struct ServiceError {
    code: String,
    description: Option<String>,
}

impl ServiceError {
    fn said(&self) -> String {
        match &self.description {
            Some(d) => format!("the service refused: {} ({d})", self.code),
            None => format!("the service refused: {}", self.code),
        }
    }
}

/// Keeps the tokens of a token answer.
fn keep(account: &mut Account, secrets: &dyn Secrets, tokens: &Value) -> Result<(), String> {
    let text = |k: &str| tokens.get(k).and_then(Value::as_str);
    let access = text("access_token").ok_or("the service gave no access token")?;
    let lasts = tokens
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    let mut put = |name: &str, value: &str| -> Result<(), String> {
        secrets.set(&account.id, name, value)?;
        if !account.secrets.iter().any(|s| s == name) {
            account.secrets.push(name.to_owned());
        }
        Ok(())
    };
    put(ACCESS_TOKEN, access)?;
    put(ACCESS_EXPIRES, &(now() + lasts).to_string())?;
    // A renewal may hand a new refresh token (rotation); keep the old
    // one otherwise.
    if let Some(refresh) = text("refresh_token") {
        put(REFRESH_TOKEN, refresh)?;
    }
    Ok(())
}

/// The environment variable for `what` of a connector:
/// `HELIOS_GOOGLE_PHOTOS_CLIENT_ID` for `google-photos`.
pub fn env_name(connector: &str, what: &str) -> String {
    let id: String = connector
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("HELIOS_{id}_{what}")
}

/// The S256 challenge of a code verifier.
pub fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `n` random bytes, base64url without padding.
fn random(n: usize) -> Result<String, String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| format!("no randomness: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `base` with `pairs` added to its query.
pub fn with_query(base: &str, pairs: &[(&str, &str)]) -> String {
    let mut url = base.to_owned();
    for (i, (k, v)) in pairs.iter().enumerate() {
        let sep = if i == 0 && !base.contains('?') {
            '?'
        } else {
            '&'
        };
        url.push(sep);
        url.push_str(&encode(k));
        url.push('=');
        url.push_str(&encode(v));
    }
    url
}

/// Percent-encodes everything but the unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The pairs of a query or a form body (`a=1&b=x%20y`), decoded.
pub fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The page the browser shows once the answer is in.
fn page(title: &str, line: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Helios</title></head>\
         <body style=\"font-family:sans-serif;margin:3em\"><h1>{title}</h1><p>{line}</p></body></html>"
    )
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// Waits on the loopback for the service's answer: the code, once its
/// `state` is checked.
fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    wait: Duration,
    cancel: &Cancel,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("the sign-in's listener: {e}"))?;
    let deadline = Instant::now() + wait;
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        if Instant::now() > deadline {
            return Err("no answer from the sign-in page in time: try again".into());
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(e) => return Err(format!("the sign-in's listener: {e}")),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Some(target) = request_target(&mut stream) else {
            continue;
        };
        let query = target.split_once('?').map_or("", |(_, q)| q);
        let pairs = query_pairs(query);
        let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let (code, error) = (get("code"), get("error"));
        if code.is_none() && error.is_none() {
            // A favicon or a stray request: not the answer.
            respond(&mut stream, "404 Not Found", "");
            continue;
        }
        if get("state").as_deref() != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                &page(
                    "Sign-in refused",
                    "This answer was not for Helios's request.",
                ),
            );
            return Err("the sign-in's answer did not carry Helios's state: refused".into());
        }
        if let Some(error) = error {
            respond(
                &mut stream,
                "200 OK",
                &page(
                    "Not signed in",
                    "You can close this tab and go back to Helios.",
                ),
            );
            return Err(if error == "access_denied" {
                "the access was refused on the consent page".into()
            } else {
                format!("the service refused the sign-in: {error}")
            });
        }
        respond(
            &mut stream,
            "200 OK",
            &page("Signed in", "You can close this tab and go back to Helios."),
        );
        return code.ok_or_else(|| "no code".into());
    }
}

/// The target of an HTTP request's first line (`/?code=…`).
fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_owned))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_rfc_7636s_and_queries_round_trip() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let url = with_query("https://a.test/auth", &[("scope", "a b"), ("x", "1/2&")]);
        assert_eq!(url, "https://a.test/auth?scope=a%20b&x=1%2F2%26");
        let pairs = query_pairs(url.split_once('?').unwrap().1);
        assert_eq!(pairs[0], ("scope".into(), "a b".into()));
        assert_eq!(pairs[1], ("x".into(), "1/2&".into()));
        assert_eq!(query_pairs("a=x+y%2"), [("a".into(), "x y%2".into())]);
        assert_eq!(
            env_name("google-photos", "CLIENT_ID"),
            "HELIOS_GOOGLE_PHOTOS_CLIENT_ID"
        );
    }
}
