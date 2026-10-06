use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;

use crate::stream::{Frame, StreamConn};
use crate::{FetchError, Method, Request, Response};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How long a WebSocket handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Where bytes come from. Queries never see this directly; they go through
/// [`crate::FetchCtx`], which adds caching, throttling, budgets and logging on
/// top. Streams reach it through the hub.
pub trait Transport: Send + Sync + 'static {
    /// One round trip. Any HTTP status is a `Response`; errors are for
    /// requests that got no answer.
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>>;

    /// Open a WebSocket. `req` carries the URL and any handshake headers.
    fn connect<'a>(
        &'a self,
        req: &'a Request,
    ) -> BoxFuture<'a, Result<Box<dyn StreamConn>, FetchError>> {
        Box::pin(async move {
            Err(FetchError::Other(format!(
                "{} cannot open {}",
                self.describe(),
                req.url
            )))
        })
    }

    /// Short description for the status bar.
    fn describe(&self) -> String;

    /// Whether this transport reaches the real source (false for replays).
    fn is_live(&self) -> bool {
        true
    }
}

/// The real network: reqwest for requests, tungstenite for streams, both on
/// native TLS (SChannel on Windows, so the system's certificate store).
///
/// Requests that carry keys ([`Request::secret_header`]) go only over TLS
/// (or to this machine, for tests) and never follow a redirect: reqwest
/// drops `Authorization` when a redirect leaves the host, but keeps custom
/// headers such as `APCA-API-KEY-ID`. A redirect comes back as its 3xx.
pub struct HttpTransport {
    client: reqwest::Client,
    keyed: reqwest::Client,
    user_agent: String,
}

impl HttpTransport {
    pub fn new(user_agent: &str) -> Result<Self, FetchError> {
        let build = |redirect: reqwest::redirect::Policy| {
            reqwest::Client::builder()
                .user_agent(user_agent)
                .connect_timeout(Duration::from_secs(10))
                // The rolling five-minute feed is ~7 MB gzipped; give it room.
                .timeout(Duration::from_secs(180))
                .redirect(redirect)
                .build()
                .map_err(|e| FetchError::Other(format!("could not build HTTP client: {e}")))
        };
        Ok(Self {
            client: build(reqwest::redirect::Policy::default())?,
            keyed: build(reqwest::redirect::Policy::none())?,
            user_agent: user_agent.to_owned(),
        })
    }
}

/// Refuse to send keys in the clear: secret headers go over `https`/`wss`,
/// or to this machine.
fn keys_travel_safely(req: &Request) -> Result<(), FetchError> {
    if !req.headers.iter().any(|(_, v)| v.is_secret()) {
        return Ok(());
    }
    let scheme = req.url.split_once("://").map_or("", |(s, _)| s);
    let local = matches!(req.host(), "localhost" | "127.0.0.1");
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") || local {
        Ok(())
    } else {
        Err(FetchError::Other(format!(
            "refused to send keys to {} without TLS",
            req.host()
        )))
    }
}

fn reqwest_method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Head => reqwest::Method::HEAD,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
    }
}

fn header_value(
    name: &str,
    value: &crate::HeaderValue,
) -> Result<tungstenite::http::HeaderValue, FetchError> {
    let mut v = tungstenite::http::HeaderValue::from_str(value.expose())
        .map_err(|_| FetchError::Other(format!("invalid value for header {name}")))?;
    v.set_sensitive(value.is_secret());
    Ok(v)
}

impl Transport for HttpTransport {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        Box::pin(async move {
            keys_travel_safely(req)?;
            let client = if req.headers.iter().any(|(_, v)| v.is_secret()) {
                &self.keyed
            } else {
                &self.client
            };
            let mut rb = client.request(reqwest_method(req.method), &req.url);
            for (name, value) in &req.headers {
                rb = rb.header(name.as_str(), header_value(name, value)?);
            }
            if let Some(body) = &req.body {
                rb = rb.body(body.clone());
            }
            let resp = rb
                .send()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
            let status = resp.status().as_u16();
            let headers = resp
                .headers()
                .iter()
                .filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned())))
                .collect();
            let body = resp
                .bytes()
                .await
                .map_err(|e| FetchError::Network(e.without_url().to_string()))?;
            Ok(Response {
                status,
                headers,
                body,
            })
        })
    }

    fn connect<'a>(
        &'a self,
        req: &'a Request,
    ) -> BoxFuture<'a, Result<Box<dyn StreamConn>, FetchError>> {
        Box::pin(async move {
            use tungstenite::client::IntoClientRequest;
            use tungstenite::http::HeaderName;
            keys_travel_safely(req)?;
            let mut ws_req = req
                .url
                .as_str()
                .into_client_request()
                .map_err(|e| FetchError::Other(format!("bad stream URL {}: {e}", req.url)))?;
            let headers = ws_req.headers_mut();
            if let Ok(ua) = tungstenite::http::HeaderValue::from_str(&self.user_agent) {
                headers.insert(tungstenite::http::header::USER_AGENT, ua);
            }
            for (name, value) in &req.headers {
                let n = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| FetchError::Other(format!("invalid header name {name}")))?;
                headers.insert(n, header_value(name, value)?);
            }
            let (ws, _) =
                tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(ws_req))
                    .await
                    .map_err(|_| FetchError::Network(format!("{} did not answer", req.host())))?
                    .map_err(|e| ws_error(&req.url, e))?;
            Ok(Box::new(WsConn { ws }) as Box<dyn StreamConn>)
        })
    }

    fn describe(&self) -> String {
        "MISO public data".into()
    }
}

fn ws_error(url: &str, e: tungstenite::Error) -> FetchError {
    match e {
        tungstenite::Error::Http(resp) => match resp.status().as_u16() {
            401 | 403 => FetchError::Auth(format!(
                "{} refused the connection (HTTP {})",
                crate::request::host_of(url),
                resp.status().as_u16()
            )),
            404 => FetchError::NotFound(url.to_owned()),
            status => FetchError::Status {
                status,
                url: url.to_owned(),
            },
        },
        e => FetchError::Network(e.to_string()),
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct WsConn {
    ws: WsStream,
}

impl StreamConn for WsConn {
    fn send(&mut self, text: String) -> BoxFuture<'_, Result<(), FetchError>> {
        Box::pin(async move {
            self.ws
                .send(tungstenite::Message::text(text))
                .await
                .map_err(|e| FetchError::Network(e.to_string()))
        })
    }

    fn recv(&mut self) -> BoxFuture<'_, Option<Result<Frame, FetchError>>> {
        Box::pin(async move {
            loop {
                let msg = match self.ws.next().await? {
                    Ok(m) => m,
                    Err(e) => return Some(Err(FetchError::Network(e.to_string()))),
                };
                return Some(Ok(match msg {
                    tungstenite::Message::Text(t) => Frame::Text(t.as_str().to_owned()),
                    tungstenite::Message::Binary(b) => Frame::Binary(b),
                    tungstenite::Message::Pong(_) => Frame::Pong,
                    tungstenite::Message::Close(_) => return None,
                    // tungstenite answers pings itself.
                    tungstenite::Message::Ping(_) | tungstenite::Message::Frame(_) => continue,
                }));
            }
        })
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), FetchError>> {
        Box::pin(async move {
            self.ws
                .send(tungstenite::Message::Ping(Bytes::new()))
                .await
                .map_err(|e| FetchError::Network(e.to_string()))
        })
    }

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.ws.close(None).await;
        })
    }
}

/// Serves recorded responses from a directory, for offline work and tests.
///
/// `https://host/a/b` maps to `<root>/host/a/b`, with `.json` appended when the
/// URL path has no extension. Date-stamped files (`20261001_da_expost_lmp.csv`)
/// fall back to the newest fixture with the same suffix, so any date "replays"
/// the recorded day. Requests other than GET look for the method before the
/// extension (`orders.post.json`). A query parameter can pick a variant of a
/// recording: `bars?timeframe=1Day&…` is served `bars@1Day.json` when that
/// file exists (values made of letters, digits, `-` and `_` are tried in
/// order), else `bars.json`. Streams replay `<path>.jsonl`, one server frame
/// per line, and then stay open and quiet.
pub struct FixtureTransport {
    root: PathBuf,
    /// Shown in the status bar instead of the directory.
    label: Option<String>,
}

impl FixtureTransport {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            label: None,
        }
    }

    /// Describe the replay as `label` rather than by its path (renders and
    /// screenshots should not depend on, or reveal, where the checkout is).
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn path_for(&self, url: &str) -> PathBuf {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let mut path = self.root.clone();
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            path.push(part);
        }
        if path.extension().is_none() {
            path.set_extension("json");
        }
        path
    }

    fn path_for_request(&self, req: &Request) -> PathBuf {
        let path = self.path_for(&req.url);
        match req.method {
            Method::Get | Method::Head => path,
            m => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");
                path.with_extension(format!("{}.{ext}", m.as_str().to_ascii_lowercase()))
            }
        }
    }

    /// Where a recording specific to one of the request's query values lives:
    /// `bars@1Day.json` for `bars?timeframe=1Day`, in query order.
    pub fn variant_paths(&self, req: &Request) -> Vec<PathBuf> {
        let path = self.path_for_request(req);
        let Some(query) = req.url.split_once('?').map(|(_, q)| q) else {
            return Vec::new();
        };
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|e| e.to_str()),
        ) else {
            return Vec::new();
        };
        let query = query.split('#').next().unwrap_or(query);
        query
            .split('&')
            .filter_map(|pair| pair.split_once('=').map(|(_, v)| v))
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= 32
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            .map(|v| path.with_file_name(format!("{stem}@{v}.{ext}")))
            .collect()
    }

    /// Where a recorded stream session for `url` lives.
    pub fn stream_path_for(&self, url: &str) -> PathBuf {
        self.path_for(url).with_extension("jsonl")
    }

    fn dated_fallback(path: &Path) -> Option<PathBuf> {
        let name = path.file_name()?.to_str()?;
        let (date, suffix) = name.split_once('_')?;
        if date.len() != 8 || !date.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(path.parent()?)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.split_once('_'))
                    .is_some_and(|(d, s)| d.len() == 8 && s == suffix)
            })
            .collect();
        candidates.sort();
        candidates.pop()
    }
}

impl Transport for FixtureTransport {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        Box::pin(async move {
            let path = self.path_for_request(req);
            let found = self
                .variant_paths(req)
                .into_iter()
                .find(|p| p.exists())
                .or_else(|| path.exists().then_some(path.clone()))
                .or_else(|| Self::dated_fallback(&path));
            let Some(path) = found else {
                return Ok(Response {
                    status: 404,
                    headers: Vec::new(),
                    body: Bytes::new(),
                });
            };
            let body = std::fs::read(&path)
                .map_err(|e| FetchError::Other(format!("{}: {e}", path.display())))?;
            Ok(Response::ok(if req.method == Method::Head {
                Vec::new()
            } else {
                body
            }))
        })
    }

    fn connect<'a>(
        &'a self,
        req: &'a Request,
    ) -> BoxFuture<'a, Result<Box<dyn StreamConn>, FetchError>> {
        Box::pin(async move {
            let path = self.stream_path_for(&req.url);
            let text = std::fs::read_to_string(&path)
                .map_err(|_| FetchError::NotFound(req.url.clone()))?;
            let frames = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            Ok(Box::new(ReplayConn { frames, pongs: 0 }) as Box<dyn StreamConn>)
        })
    }

    fn describe(&self) -> String {
        match &self.label {
            Some(label) => format!("replaying {label}"),
            None => format!("replaying {}", self.root.display()),
        }
    }

    fn is_live(&self) -> bool {
        false
    }
}

/// A recorded stream session: its frames, then silence (pings still get pongs,
/// so the hub's keep-alive is satisfied and nothing reconnects).
struct ReplayConn {
    frames: VecDeque<String>,
    pongs: usize,
}

impl StreamConn for ReplayConn {
    fn send(&mut self, _text: String) -> BoxFuture<'_, Result<(), FetchError>> {
        Box::pin(async { Ok(()) })
    }

    fn recv(&mut self) -> BoxFuture<'_, Option<Result<Frame, FetchError>>> {
        Box::pin(async move {
            if self.pongs > 0 {
                self.pongs -= 1;
                return Some(Ok(Frame::Pong));
            }
            match self.frames.pop_front() {
                Some(f) => Some(Ok(Frame::Text(f))),
                None => std::future::pending().await,
            }
        })
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), FetchError>> {
        self.pongs += 1;
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fixture_paths() {
        let t = FixtureTransport::new("/fx");
        assert_eq!(
            t.path_for("https://public-api.misoenergy.org/api/FuelMix"),
            Path::new("/fx/public-api.misoenergy.org/api/FuelMix.json")
        );
        assert_eq!(
            t.path_for("https://docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv"),
            Path::new("/fx/docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv")
        );
        assert_eq!(
            t.path_for_request(&Request::post("https://api.example.com/v2/orders")),
            Path::new("/fx/api.example.com/v2/orders.post.json")
        );
        assert_eq!(
            t.stream_path_for("wss://stream.example.com/v2/iex"),
            Path::new("/fx/stream.example.com/v2/iex.jsonl")
        );
        let bars = Request::get(
            "https://data.example.com/v2/stocks/bars?symbols=XLU,XEL&timeframe=1Day&start=2026-10-01T04:00:00Z&feed=iex",
        );
        assert_eq!(
            t.variant_paths(&bars),
            [
                Path::new("/fx/data.example.com/v2/stocks/bars@1Day.json"),
                Path::new("/fx/data.example.com/v2/stocks/bars@iex.json"),
            ],
            "only filename-safe values"
        );
        assert!(t.variant_paths(&Request::get("https://h/a")).is_empty());
    }

    #[tokio::test]
    async fn query_values_pick_a_variant_recording() {
        let root = std::env::temp_dir().join(format!("mt-variant-test-{}", std::process::id()));
        let dir = root.join("host").join("v2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bars.json"), "default").unwrap();
        std::fs::write(dir.join("bars@1Day.json"), "daily").unwrap();
        let t = FixtureTransport::new(&root);
        let body = |url: &str| {
            let t = &t;
            let url = url.to_owned();
            async move { t.send(&Request::get(url)).await.unwrap().body }
        };
        assert_eq!(
            body("https://host/v2/bars?timeframe=1Day").await.as_ref(),
            b"daily"
        );
        assert_eq!(
            body("https://host/v2/bars?timeframe=1Min").await.as_ref(),
            b"default"
        );
        assert_eq!(body("https://host/v2/bars").await.as_ref(), b"default");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn dated_files_fall_back_to_newest_recording() {
        let root = std::env::temp_dir().join(format!("mt-fixture-test-{}", std::process::id()));
        let dir = root.join("host").join("reports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("20260101_da.csv"), "old").unwrap();
        std::fs::write(dir.join("20260301_da.csv"), "new").unwrap();
        std::fs::write(dir.join("20260401_rt.csv"), "other").unwrap();
        let t = FixtureTransport::new(&root);
        let got = t
            .send(&Request::get("https://host/reports/20991231_da.csv"))
            .await
            .unwrap();
        assert_eq!(got.body.as_ref(), b"new");
        let missing = t
            .send(&Request::get("https://host/reports/missing.csv"))
            .await
            .unwrap();
        assert_eq!(missing.status, 404);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A server on this machine that answers every request with `answer`
    /// and counts them.
    async fn serve(answer: String) -> (u16, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let mut seen = Vec::new();
                let mut buf = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tcp.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = tcp.write_all(answer.as_bytes()).await;
            }
        });
        (port, hits)
    }

    /// Keys never follow a redirect to another host, and never go out in the
    /// clear; requests without keys still follow redirects.
    #[tokio::test]
    async fn keyed_requests_never_follow_redirects() {
        let (landing, hits) =
            serve("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into())
                .await;
        let (start, _) = serve(format!(
            "HTTP/1.1 302 Found\r\nLocation: http://localhost:{landing}/landing\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n"
        ))
        .await;
        let t = HttpTransport::new("mt-test").unwrap();
        let url = format!("http://127.0.0.1:{start}/v2/orders");

        let keyed = Request::get(&url).secret_header("APCA-API-KEY-ID", crate::Secret::new("k1"));
        let resp = t.send(&keyed).await.unwrap();
        assert_eq!(resp.status, 302, "the redirect comes back as it is");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the key never left");

        let plain = t.send(&Request::get(&url)).await.unwrap();
        assert_eq!((plain.status, plain.body.as_ref()), (200, &b"ok"[..]));
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let clear = Request::get("http://example.invalid/v2/orders")
            .secret_header("APCA-API-KEY-ID", crate::Secret::new("k1"));
        assert!(matches!(t.send(&clear).await, Err(FetchError::Other(_))));
        let clear_ws = Request::get("ws://example.invalid/stream")
            .secret_header("APCA-API-KEY-ID", crate::Secret::new("k1"));
        assert!(matches!(
            t.connect(&clear_ws).await.err(),
            Some(FetchError::Other(_))
        ));
    }

    /// The real WebSocket client against a local server: handshake headers
    /// arrive (secret ones too), text goes both ways, pings get pongs, and a
    /// refused handshake is an auth error.
    #[tokio::test]
    async fn websockets_round_trip_through_the_http_transport() {
        use tokio_tungstenite::tungstenite::handshake::server::{
            ErrorResponse, Request as Hs, Response as HsResp,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (tcp, _) = listener.accept().await.unwrap();
                // The callback's signature is tungstenite's.
                #[allow(clippy::result_large_err)]
                let check = |req: &Hs, resp: HsResp| -> Result<HsResp, ErrorResponse> {
                    if req.headers().get("x-key").and_then(|v| v.to_str().ok()) == Some("k1") {
                        Ok(resp)
                    } else {
                        let mut refuse = ErrorResponse::new(None);
                        *refuse.status_mut() =
                            tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                        Err(refuse)
                    }
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, check).await else {
                    continue;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_text() {
                        ws.send(msg).await.unwrap();
                    }
                }
            }
        });
        let t = HttpTransport::new("mt-test").unwrap();
        let url = format!("ws://{addr}/echo");
        let mut conn = t
            .connect(&Request::get(&url).secret_header("X-Key", crate::Secret::new("k1")))
            .await
            .unwrap();
        conn.send("hello".into()).await.unwrap();
        assert_eq!(
            conn.recv().await.unwrap().unwrap(),
            Frame::Text("hello".into())
        );
        conn.ping().await.unwrap();
        assert_eq!(conn.recv().await.unwrap().unwrap(), Frame::Pong);
        conn.close().await;
        let refused = t
            .connect(&Request::get(&url).secret_header("X-Key", crate::Secret::new("wrong")))
            .await;
        assert!(matches!(refused.err(), Some(FetchError::Auth(_))));
    }

    #[tokio::test]
    async fn recorded_streams_replay_then_answer_pings() {
        let root = std::env::temp_dir().join(format!("mt-stream-fx-{}", std::process::id()));
        let dir = root.join("stream.example.com").join("v2");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("iex.jsonl"),
            "[{\"T\":\"success\"}]\n\n[{\"T\":\"q\"}]\n",
        )
        .unwrap();
        let t = FixtureTransport::new(&root);
        let mut conn = t
            .connect(&Request::get("wss://stream.example.com/v2/iex"))
            .await
            .unwrap();
        assert_eq!(
            conn.recv().await.unwrap().unwrap(),
            Frame::Text("[{\"T\":\"success\"}]".into())
        );
        assert_eq!(
            conn.recv().await.unwrap().unwrap(),
            Frame::Text("[{\"T\":\"q\"}]".into())
        );
        conn.ping().await.unwrap();
        assert_eq!(conn.recv().await.unwrap().unwrap(), Frame::Pong);
        let quiet = tokio::time::timeout(Duration::from_millis(20), conn.recv()).await;
        assert!(quiet.is_err(), "a finished replay stays open and quiet");
        assert!(matches!(
            t.connect(&Request::get("wss://stream.example.com/v2/sip"))
                .await
                .err(),
            Some(FetchError::NotFound(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
