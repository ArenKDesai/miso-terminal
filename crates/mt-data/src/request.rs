//! Requests and responses: a method, headers and a body, so sources that need
//! API keys or send JSON can go through the same [`crate::FetchCtx`] as plain
//! GETs. Sensitive header values are [`Secret`]s and never appear in logs.

use std::fmt;

use bytes::Bytes;

use crate::Secret;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A header value: plain, or secret (an API key) and redacted wherever it is shown.
#[derive(Clone, Debug)]
pub enum HeaderValue {
    Plain(String),
    Secret(Secret),
}

impl HeaderValue {
    /// The value to put on the wire.
    pub fn expose(&self) -> &str {
        match self {
            Self::Plain(v) => v,
            Self::Secret(s) => s.expose(),
        }
    }

    pub fn is_secret(&self) -> bool {
        matches!(self, Self::Secret(_))
    }

    /// The value as logs show it.
    pub fn redacted(&self) -> &str {
        match self {
            Self::Plain(v) => v,
            Self::Secret(_) => "***",
        }
    }
}

/// Header names whose values are redacted even when given as plain text.
fn is_sensitive_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization"
        || n == "cookie"
        || n == "proxy-authorization"
        || n.contains("key")
        || n.contains("secret")
        || n.contains("token")
}

/// One HTTP request (or WebSocket handshake).
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, HeaderValue)>,
    pub body: Option<Bytes>,
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::Post, url)
    }

    pub fn delete(url: impl Into<String>) -> Self {
        Self::new(Method::Delete, url)
    }

    /// Add a header. Values of headers named like credentials (`Authorization`,
    /// `*key*`, `*secret*`, `*token*`, `Cookie`) are treated as secret.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();
        let value = if is_sensitive_name(&name) {
            HeaderValue::Secret(Secret::new(value))
        } else {
            HeaderValue::Plain(value)
        };
        self.headers.push((name, value));
        self
    }

    /// Add a header whose value is a secret.
    pub fn secret_header(mut self, name: impl Into<String>, value: Secret) -> Self {
        self.headers.push((name.into(), HeaderValue::Secret(value)));
        self
    }

    /// A JSON body (sets `Content-Type`).
    pub fn json(mut self, body: &impl serde::Serialize) -> Result<Self, serde_json::Error> {
        self.body = Some(Bytes::from(serde_json::to_vec(body)?));
        Ok(self.header("Content-Type", "application/json"))
    }

    pub fn body(mut self, body: impl Into<Bytes>, content_type: &str) -> Self {
        self.body = Some(body.into());
        self.header("Content-Type", content_type)
    }

    /// The first header with this name (case-insensitive).
    pub fn header_value(&self, name: &str) -> Option<&HeaderValue> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn host(&self) -> &str {
        host_of(&self.url)
    }

    /// `GET https://host/path` plus any headers, with secret values replaced by
    /// `***`. This is what the event log shows.
    pub fn describe(&self) -> String {
        let mut out = format!("{} {}", self.method, self.url);
        let shown: Vec<String> = self
            .headers
            .iter()
            .map(|(n, v)| format!("{n}: {}", v.redacted()))
            .collect();
        if !shown.is_empty() {
            out.push_str(" [");
            out.push_str(&shown.join(", "));
            out.push(']');
        }
        out
    }
}

impl From<&str> for Request {
    fn from(url: &str) -> Self {
        Self::get(url)
    }
}

impl From<&String> for Request {
    fn from(url: &String) -> Self {
        Self::get(url.as_str())
    }
}

impl From<String> for Request {
    fn from(url: String) -> Self {
        Self::get(url)
    }
}

impl From<&Request> for Request {
    fn from(req: &Request) -> Self {
        req.clone()
    }
}

/// What came back: any status, so callers that need a 4xx body (an API's
/// error message) can read it. [`crate::FetchCtx::get`] turns failures into errors.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Response {
    pub fn ok(body: impl Into<Bytes>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The start of the body as text, for error messages (APIs explain
    /// rejections in the body).
    pub fn excerpt(&self, max: usize) -> String {
        let text = String::from_utf8_lossy(&self.body);
        let text = text.trim();
        match text.char_indices().nth(max) {
            Some((i, _)) => format!("{}…", &text[..i]),
            None => text.to_owned(),
        }
    }
}

/// The host part of a URL, without scheme, port or path.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    authority.split(':').next().unwrap_or(authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_redacts_secrets() {
        let req = Request::get("https://data.example.com/v2/bars?symbols=XLU")
            .header("APCA-API-KEY-ID", "PKTEST123")
            .secret_header("X-Custom", Secret::new("hunter2"))
            .header("Authorization", "Bearer abc.def")
            .header("Accept", "application/json");
        let shown = req.describe();
        for secret in ["PKTEST123", "hunter2", "abc.def"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("APCA-API-KEY-ID: ***"));
        assert!(shown.contains("Accept: application/json"));
        assert!(!format!("{req:?}").contains("hunter2"), "Debug redacts too");
        assert_eq!(
            req.header_value("apca-api-key-id").map(HeaderValue::expose),
            Some("PKTEST123")
        );
    }

    #[test]
    fn hosts_and_excerpts() {
        assert_eq!(
            host_of("https://paper-api.alpaca.markets/v2/orders"),
            "paper-api.alpaca.markets"
        );
        assert_eq!(
            host_of("wss://user@stream.example.com:443/v2/iex"),
            "stream.example.com"
        );
        assert_eq!(host_of("http://localhost:8080"), "localhost");
        let resp = Response {
            status: 403,
            headers: vec![],
            body: Bytes::from_static(b" {\"message\":\"forbidden\"} "),
        };
        assert_eq!(resp.excerpt(100), "{\"message\":\"forbidden\"}");
        assert_eq!(resp.excerpt(4), "{\"me…");
    }

    #[test]
    fn json_bodies_set_the_content_type() {
        let req = Request::post("https://h/orders")
            .json(&serde_json::json!({"symbol": "XLU", "qty": "1"}))
            .unwrap();
        assert_eq!(
            req.header_value("content-type").map(HeaderValue::expose),
            Some("application/json")
        );
        assert!(req.body.is_some_and(|b| b.starts_with(b"{")));
    }
}
