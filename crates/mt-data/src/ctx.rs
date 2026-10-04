use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;

use crate::budget::{BudgetStatus, Budgets};
use crate::{
    Budget, DiskCache, EventLog, FetchError, MemorySecrets, Method, Request, Response, Secret,
    SecretStore, Transport,
};

/// Bodies larger than this are not kept for conditional requests.
const MAX_VALIDATED_BODY: usize = 8 * 1024 * 1024;
/// How many URLs keep their validators (least recently used go first).
const MAX_VALIDATED: usize = 64;
/// Wait after a 429 that names no `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct FetchCtxOptions {
    /// Maximum simultaneous requests across all queries.
    pub max_concurrent: usize,
    /// Repeat requests for the same URL inside this window are answered from
    /// memory. MISO asks that each real-time link be hit at most once a minute.
    pub polite_interval: Duration,
    /// Request limits per host (or group of hosts), shared by every query.
    pub budgets: Vec<Budget>,
    /// Where API keys come from: Windows Credential Manager in the app,
    /// memory in tests and offline replay.
    pub secrets: Arc<dyn SecretStore>,
}

impl Default for FetchCtxOptions {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            polite_interval: Duration::from_secs(55),
            budgets: Vec::new(),
            secrets: Arc::new(MemorySecrets::default()),
        }
    }
}

/// Everything a [`crate::Query`] needs to fetch. Cheap to clone.
#[derive(Clone)]
pub struct FetchCtx {
    inner: Arc<Inner>,
}

struct Inner {
    transport: Arc<dyn Transport>,
    cache: Option<DiskCache>,
    limiter: Semaphore,
    opts: FetchCtxOptions,
    recent: Mutex<HashMap<String, (Instant, Bytes)>>,
    /// The last body of each URL that came with an `ETag` or `Last-Modified`,
    /// so the next request can ask "changed since?" and accept a `304`.
    validated: Mutex<HashMap<String, Validated>>,
    budgets: Budgets,
    events: EventLog,
}

struct Validated {
    etag: Option<String>,
    last_modified: Option<String>,
    body: Bytes,
    used: Instant,
}

impl FetchCtx {
    pub fn new(
        transport: Arc<dyn Transport>,
        cache: Option<DiskCache>,
        opts: FetchCtxOptions,
        events: EventLog,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                transport,
                cache,
                limiter: Semaphore::new(opts.max_concurrent.max(1)),
                budgets: Budgets::new(&opts.budgets),
                opts,
                recent: Mutex::default(),
                validated: Mutex::default(),
                events,
            }),
        }
    }

    pub fn events(&self) -> &EventLog {
        &self.inner.events
    }

    pub fn cache(&self) -> Option<&DiskCache> {
        self.inner.cache.as_ref()
    }

    pub fn transport_description(&self) -> String {
        self.inner.transport.describe()
    }

    pub fn is_live(&self) -> bool {
        self.inner.transport.is_live()
    }

    pub(crate) fn transport(&self) -> &Arc<dyn Transport> {
        &self.inner.transport
    }

    /// The secret store, for the settings panel.
    pub fn secrets(&self) -> &Arc<dyn SecretStore> {
        &self.inner.opts.secrets
    }

    /// A stored secret, or [`FetchError::Auth`] naming it when it is not set.
    pub fn secret(&self, name: &str) -> Result<Secret, FetchError> {
        match self.inner.opts.secrets.get(name)? {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(FetchError::Auth(format!("{name} is not set"))),
        }
    }

    /// Request budgets and how much of each is in use, for LOG.
    pub fn budget_status(&self) -> Vec<BudgetStatus> {
        self.inner.budgets.status()
    }

    /// One request, any method: request budget, concurrency cap and log, but
    /// no caching and no retries. Every HTTP status comes back as a
    /// [`Response`] (an order desk needs a rejection's body); only a request
    /// that got no answer is an error.
    pub async fn send(&self, req: impl Into<Request>) -> Result<Response, FetchError> {
        self.send_network(&req.into()).await
    }

    /// GET something whose content changes over time. Repeats inside the
    /// polite interval are answered from memory, and a URL whose server sent
    /// an `ETag` or `Last-Modified` is asked "changed since?", so an unchanged
    /// feed costs a `304` instead of the whole body.
    pub async fn get(&self, req: impl Into<Request>) -> Result<Bytes, FetchError> {
        let req = req.into();
        let cacheable = req.method == Method::Get;
        let url = req.url.clone();
        let polite = self.inner.opts.polite_interval;
        if cacheable
            && let Some((at, body)) = self.inner.recent.lock().get(&url)
            && at.elapsed() < polite
        {
            self.events().info(format!(
                "reuse {url} (fetched {}s ago)",
                at.elapsed().as_secs()
            ));
            return Ok(body.clone());
        }
        let mut wire = req.clone();
        if cacheable && let Some(v) = self.inner.validated.lock().get(&url) {
            if let Some(etag) = &v.etag {
                wire = wire.header("If-None-Match", etag.as_str());
            }
            if let Some(lm) = &v.last_modified {
                wire = wire.header("If-Modified-Since", lm.as_str());
            }
        }
        let resp = self.send_network(&wire).await?;
        let body = if resp.status == 304 {
            let mut validated = self.inner.validated.lock();
            match validated.get_mut(&url) {
                Some(v) => {
                    v.used = Instant::now();
                    v.body.clone()
                }
                // A 304 we did not ask for: treat as an empty answer.
                None => return Err(FetchError::Status { status: 304, url }),
            }
        } else {
            let etag = resp.header("etag").map(str::to_owned);
            let last_modified = resp.header("last-modified").map(str::to_owned);
            let body = self.check(&req, resp)?;
            if cacheable {
                self.remember_validators(&url, etag, last_modified, &body);
            }
            body
        };
        if cacheable {
            let mut recent = self.inner.recent.lock();
            recent.retain(|_, (at, _)| at.elapsed() < polite);
            recent.insert(url, (Instant::now(), body.clone()));
        }
        Ok(body)
    }

    /// GET a URL whose content never changes once it exists. Served from the
    /// disk cache when present; stored there after a successful download.
    pub async fn get_immutable(&self, req: impl Into<Request>) -> Result<Bytes, FetchError> {
        let req = req.into();
        let url = req.url.clone();
        if let Some(cache) = self.inner.cache.clone() {
            let key = url.clone();
            let hit = tokio::task::spawn_blocking(move || cache.get(&key))
                .await
                .ok()
                .flatten();
            if let Some(body) = hit {
                self.events().info(format!("cache hit {url}"));
                return Ok(body);
            }
        }
        let resp = self.send_network(&req).await?;
        let body = self.check(&req, resp)?;
        if let Some(cache) = self.inner.cache.clone() {
            let (data, events) = (body.clone(), self.events().clone());
            tokio::task::spawn_blocking(move || {
                if let Err(e) = cache.put(&url, &data) {
                    events.warn(format!("could not cache {url}: {e}"));
                }
            });
        }
        Ok(body)
    }

    /// Read something the app stored itself (e.g. `local://intraday/<day>`)
    /// from the disk cache, off the async workers. `None` without a cache.
    pub async fn local_get(&self, key: &str) -> Option<Bytes> {
        let cache = self.inner.cache.clone()?;
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || cache.get(&key))
            .await
            .ok()
            .flatten()
    }

    /// Store something under a local key. Failures are logged, not returned:
    /// the cache is an optimisation.
    pub async fn local_put(&self, key: &str, data: Vec<u8>) -> bool {
        let Some(cache) = self.inner.cache.clone() else {
            return false;
        };
        let (key, events) = (key.to_owned(), self.events().clone());
        tokio::task::spawn_blocking(move || match cache.put(&key, &data) {
            Ok(()) => true,
            Err(e) => {
                events.warn(format!("could not store {key}: {e}"));
                false
            }
        })
        .await
        .unwrap_or(false)
    }

    pub async fn get_text(&self, req: impl Into<Request>) -> Result<String, FetchError> {
        Ok(text(self.get(req).await?))
    }

    pub async fn get_text_immutable(&self, req: impl Into<Request>) -> Result<String, FetchError> {
        Ok(text(self.get_immutable(req).await?))
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        req: impl Into<Request>,
    ) -> Result<T, FetchError> {
        let req = req.into();
        let body = self.get(&req).await?;
        serde_json::from_slice(&body).map_err(|e| FetchError::parse(&req.url, e))
    }

    /// Turn a non-success status into the matching error.
    fn check(&self, req: &Request, resp: Response) -> Result<Bytes, FetchError> {
        match resp.status {
            200..=299 => Ok(resp.body),
            404 => Err(FetchError::NotFound(req.url.clone())),
            401 | 403 => Err(FetchError::Auth(format!(
                "HTTP {} from {}: {}",
                resp.status,
                req.host(),
                resp.excerpt(200)
            ))),
            429 => Err(FetchError::RateLimited {
                host: req.host().to_owned(),
                retry_after_secs: retry_after(&resp).as_secs(),
            }),
            status => Err(FetchError::Status {
                status,
                url: req.url.clone(),
            }),
        }
    }

    fn remember_validators(
        &self,
        url: &str,
        etag: Option<String>,
        last_modified: Option<String>,
        body: &Bytes,
    ) {
        let mut validated = self.inner.validated.lock();
        if (etag.is_none() && last_modified.is_none()) || body.len() > MAX_VALIDATED_BODY {
            validated.remove(url);
            return;
        }
        validated.insert(
            url.to_owned(),
            Validated {
                etag,
                last_modified,
                body: body.clone(),
                used: Instant::now(),
            },
        );
        while validated.len() > MAX_VALIDATED {
            let Some(oldest) = validated
                .iter()
                .min_by_key(|(_, v)| v.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            validated.remove(&oldest);
        }
    }

    async fn send_network(&self, req: &Request) -> Result<Response, FetchError> {
        let host = req.host().to_owned();
        let waited = self.inner.budgets.acquire(&host).await;
        if waited >= Duration::from_millis(500) {
            self.events().info(format!(
                "{} waited {:.1} s for its request budget",
                req.describe(),
                waited.as_secs_f32()
            ));
        }
        let _permit = self
            .inner
            .limiter
            .acquire()
            .await
            .map_err(|e| FetchError::Other(e.to_string()))?;
        let started = Instant::now();
        let result = self.inner.transport.send(req).await;
        let ms = started.elapsed().as_millis();
        let what = req.describe();
        match &result {
            Ok(r) if r.is_success() => self
                .events()
                .info(format!("{what} -> {} KB in {ms} ms", r.body.len() / 1024)),
            Ok(r) if r.status == 304 => self
                .events()
                .info(format!("{what} -> not modified ({ms} ms)")),
            Ok(r) if r.status == 404 => self
                .events()
                .info(format!("{what} -> not published ({ms} ms)")),
            Ok(r) if r.status == 429 => {
                let wait = retry_after(r);
                self.inner.budgets.block(&host, wait);
                self.events().warn(format!(
                    "{what} -> rate limited; holding {host} for {} s",
                    wait.as_secs()
                ));
            }
            Ok(r) => self
                .events()
                .warn(format!("{what} -> HTTP {} in {ms} ms", r.status)),
            Err(e) => self
                .events()
                .warn(format!("{what} failed after {ms} ms: {e}")),
        }
        result
    }
}

/// `Retry-After` in seconds (the HTTP-date form is rare for APIs; it gets the default).
fn retry_after(resp: &Response) -> Duration {
    resp.header("retry-after")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_AFTER, Duration::from_secs)
}

/// Decode as UTF-8, replacing invalid bytes rather than failing: a stray
/// Windows-1252 character in a report should not lose the whole report.
fn text(body: Bytes) -> String {
    match String::from_utf8(body.to_vec()) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::BoxFuture;

    /// Answers from a script and records what it was asked.
    #[derive(Default)]
    struct Scripted {
        answers: Mutex<Vec<Response>>,
        asked: Mutex<Vec<Request>>,
    }

    impl Scripted {
        fn with(answers: Vec<Response>) -> Arc<Self> {
            let mut answers = answers;
            answers.reverse();
            Arc::new(Self {
                answers: Mutex::new(answers),
                asked: Mutex::default(),
            })
        }
    }

    impl Transport for Scripted {
        fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
            self.asked.lock().push(req.clone());
            let next = self.answers.lock().pop();
            Box::pin(async move { next.ok_or_else(|| FetchError::Network("script ended".into())) })
        }
        fn describe(&self) -> String {
            "script".into()
        }
    }

    fn resp(status: u16, headers: &[(&str, &str)], body: &'static str) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: Bytes::from_static(body.as_bytes()),
        }
    }

    fn ctx(t: Arc<Scripted>, opts: FetchCtxOptions) -> FetchCtx {
        FetchCtx::new(t, None, opts, EventLog::default())
    }

    fn impatient() -> FetchCtxOptions {
        FetchCtxOptions {
            polite_interval: Duration::ZERO,
            ..FetchCtxOptions::default()
        }
    }

    #[tokio::test]
    async fn unchanged_feeds_cost_a_304() {
        let t = Scripted::with(vec![
            resp(200, &[("etag", "W/\"v1\"")], "<rss>one</rss>"),
            resp(304, &[], ""),
            resp(
                200,
                &[("last-modified", "Sun, 04 Oct 2026 12:00:00 GMT")],
                "two",
            ),
        ]);
        let c = ctx(t.clone(), impatient());
        let url = "https://feeds.example.com/markets.rss";
        assert_eq!(c.get(url).await.unwrap().as_ref(), b"<rss>one</rss>");
        assert_eq!(c.get(url).await.unwrap().as_ref(), b"<rss>one</rss>");
        assert_eq!(c.get(url).await.unwrap().as_ref(), b"two");
        let asked = t.asked.lock();
        assert!(asked[0].header_value("if-none-match").is_none());
        assert_eq!(
            asked[1].header_value("if-none-match").map(|v| v.expose()),
            Some("W/\"v1\"")
        );
        assert!(
            asked[2].header_value("if-none-match").is_some(),
            "the 304 kept the validator"
        );
        let log: Vec<String> = c
            .events()
            .recent(10)
            .into_iter()
            .map(|e| e.message)
            .collect();
        assert!(log.iter().any(|m| m.contains("not modified")), "{log:?}");
    }

    #[tokio::test]
    async fn statuses_become_errors_and_secrets_stay_out_of_the_log() {
        let t = Scripted::with(vec![
            resp(404, &[], ""),
            resp(403, &[], "{\"message\":\"forbidden.\"}"),
            resp(500, &[], "oops"),
            resp(429, &[("retry-after", "7")], ""),
        ]);
        let budget = Budget::new("Example", ["api.example.com"], 100, Duration::from_secs(60));
        let c = ctx(
            t,
            FetchCtxOptions {
                budgets: vec![budget],
                ..impatient()
            },
        );
        let authed = |path: &str| {
            Request::get(format!("https://api.example.com/{path}"))
                .secret_header("APCA-API-SECRET-KEY", Secret::new("s3cr3t"))
        };
        assert!(matches!(
            c.get(authed("a")).await,
            Err(FetchError::NotFound(_))
        ));
        match c.get(authed("b")).await {
            Err(FetchError::Auth(m)) => assert!(m.contains("forbidden."), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            c.get(authed("c")).await,
            Err(FetchError::Status { status: 500, .. })
        ));
        assert_eq!(
            c.get(authed("d")).await,
            Err(FetchError::RateLimited {
                host: "api.example.com".into(),
                retry_after_secs: 7
            })
        );
        let blocked = c.budget_status()[0].blocked_for.unwrap();
        assert!(
            blocked > Duration::from_secs(6),
            "a 429 holds the budget closed"
        );
        for e in c.events().recent(50) {
            assert!(!e.message.contains("s3cr3t"), "{}", e.message);
        }
    }

    #[tokio::test]
    async fn send_returns_every_status_and_skips_the_caches() {
        let t = Scripted::with(vec![
            resp(422, &[], "{\"message\":\"qty must be > 0\"}"),
            resp(200, &[("etag", "x")], "ok"),
            resp(200, &[], "ok again"),
        ]);
        let c = ctx(t.clone(), FetchCtxOptions::default());
        let order = Request::post("https://api.example.com/v2/orders")
            .json(&serde_json::json!({"qty": "0"}))
            .unwrap();
        let r = c.send(&order).await.unwrap();
        assert_eq!(r.status, 422);
        assert!(r.excerpt(100).contains("qty must be"));
        // `send` neither answers from the polite window nor adds validators.
        c.send("https://api.example.com/x").await.unwrap();
        c.send("https://api.example.com/x").await.unwrap();
        assert!(t.asked.lock()[2].header_value("if-none-match").is_none());
    }

    #[test]
    fn missing_secrets_are_auth_errors() {
        let c = ctx(Scripted::with(vec![]), FetchCtxOptions::default());
        assert!(matches!(
            c.secret("alpaca/paper/key-id"),
            Err(FetchError::Auth(_))
        ));
        c.secrets()
            .set("alpaca/paper/key-id", &Secret::new("PK1"))
            .unwrap();
        assert_eq!(c.secret("alpaca/paper/key-id").unwrap().expose(), "PK1");
    }
}
