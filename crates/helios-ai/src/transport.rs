//! How providers reach their servers: a [`Transport`] sends an HTTP call
//! and returns the status and body, or hands the body over line by line
//! (a pull's progress). [`Ureq`] is the real one; tests use
//! [`crate::fake::FakeRuntime`], which answers like a runtime would.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::Value;

/// The HTTP method of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One HTTP call.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpCall {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    pub timeout: Duration,
}

impl HttpCall {
    pub fn get(url: &str, timeout: Duration) -> Self {
        HttpCall {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout,
        }
    }

    pub fn post(url: &str, body: Value, timeout: Duration) -> Self {
        HttpCall {
            method: Method::Post,
            url: url.into(),
            headers: Vec::new(),
            body: Some(body),
            timeout,
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The path of the URL (`/api/tags`), without the query.
    pub fn path(&self) -> &str {
        let rest = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, r)| r);
        let path = rest.find('/').map_or("/", |i| &rest[i..]);
        path.split('?').next().unwrap_or(path)
    }
}

/// Sends HTTP calls.
pub trait Transport: Send + Sync {
    /// Sends the call; the status and the whole body.
    fn send(&self, call: &HttpCall) -> Result<(u16, String), String>;

    /// Sends the call and hands each line of the body to `line` as it
    /// arrives, until it returns false or the body ends; the status. A
    /// status other than 2xx hands its body over as one line.
    fn stream(&self, call: &HttpCall, line: &mut dyn FnMut(&str) -> bool) -> Result<u16, String>;
}

/// The body of a 2xx answer as JSON; any other status is an error naming
/// the URL, the status and the body.
pub fn json_reply(call: &HttpCall, status: u16, body: &str) -> Result<Value, String> {
    if !(200..300).contains(&status) {
        return Err(format!("{} answered {status}: {}", call.url, body.trim()));
    }
    serde_json::from_str(body).map_err(|e| format!("{}: {e}", call.url))
}

/// Sends `call` and reads its JSON answer.
pub fn send_json(transport: &dyn Transport, call: &HttpCall) -> Result<Value, String> {
    let (status, body) = transport.send(call)?;
    json_reply(call, status, &body)
}

/// The real transport, through `ureq`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ureq;

impl Ureq {
    fn response(call: &HttpCall) -> Result<ureq::http::Response<ureq::Body>, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(call.timeout))
            .http_status_as_error(false)
            .build()
            .into();
        let sent = match call.method {
            Method::Get => {
                let mut request = agent.get(&call.url);
                for (name, value) in &call.headers {
                    request = request.header(name.as_str(), value.as_str());
                }
                request.call()
            }
            Method::Post => {
                let mut request = agent.post(&call.url);
                for (name, value) in &call.headers {
                    request = request.header(name.as_str(), value.as_str());
                }
                match &call.body {
                    Some(body) => request.send_json(body),
                    None => request.send_empty(),
                }
            }
        };
        sent.map_err(|e| format!("{}: {e}", call.url))
    }
}

impl Transport for Ureq {
    fn send(&self, call: &HttpCall) -> Result<(u16, String), String> {
        let mut response = Self::response(call)?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{}: {e}", call.url))?;
        Ok((status, body))
    }

    fn stream(&self, call: &HttpCall, line: &mut dyn FnMut(&str) -> bool) -> Result<u16, String> {
        let mut response = Self::response(call)?;
        let status = response.status().as_u16();
        let reader = BufReader::new(response.body_mut().as_reader());
        for read in reader.lines() {
            let text = read.map_err(|e| format!("{}: {e}", call.url))?;
            if text.trim().is_empty() {
                continue;
            }
            if !line(&text) {
                break;
            }
        }
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_read_from_the_url() {
        let t = Duration::from_secs(1);
        assert_eq!(
            HttpCall::get("http://127.0.0.1:11434/api/tags", t).path(),
            "/api/tags"
        );
        assert_eq!(HttpCall::get("http://host", t).path(), "/");
        assert_eq!(
            HttpCall::get("https://h/v1/models?x=1", t).path(),
            "/v1/models"
        );
    }

    #[test]
    fn an_error_status_names_the_url_and_the_body() {
        let call = HttpCall::get("http://h/x", Duration::from_secs(1));
        let err = json_reply(&call, 401, "bad key\n").unwrap_err();
        assert_eq!(err, "http://h/x answered 401: bad key");
        assert_eq!(json_reply(&call, 200, "{\"a\":1}").unwrap()["a"], 1);
    }
}
