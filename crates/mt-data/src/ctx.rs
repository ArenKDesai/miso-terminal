use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;

use crate::{DiskCache, EventLog, FetchError, Transport};

#[derive(Clone, Debug)]
pub struct FetchCtxOptions {
    /// Maximum simultaneous requests across all queries.
    pub max_concurrent: usize,
    /// Repeat requests for the same URL inside this window are answered from
    /// memory. MISO asks that each real-time link be hit at most once a minute.
    pub polite_interval: Duration,
}

impl Default for FetchCtxOptions {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            polite_interval: Duration::from_secs(55),
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
    events: EventLog,
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
                opts,
                recent: Mutex::default(),
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

    /// GET a URL whose content changes over time.
    pub async fn get(&self, url: &str) -> Result<Bytes, FetchError> {
        let polite = self.inner.opts.polite_interval;
        if let Some((at, body)) = self.inner.recent.lock().get(url)
            && at.elapsed() < polite
        {
            self.events().info(format!(
                "reuse {url} (fetched {}s ago)",
                at.elapsed().as_secs()
            ));
            return Ok(body.clone());
        }
        let body = self.fetch_network(url).await?;
        let mut recent = self.inner.recent.lock();
        recent.retain(|_, (at, _)| at.elapsed() < polite);
        recent.insert(url.to_owned(), (Instant::now(), body.clone()));
        Ok(body)
    }

    /// GET a URL whose content never changes once it exists. Served from the
    /// disk cache when present; stored there after a successful download.
    pub async fn get_immutable(&self, url: &str) -> Result<Bytes, FetchError> {
        if let Some(cache) = self.inner.cache.clone() {
            let key = url.to_owned();
            let hit = tokio::task::spawn_blocking(move || cache.get(&key))
                .await
                .ok()
                .flatten();
            if let Some(body) = hit {
                self.events().info(format!("cache hit {url}"));
                return Ok(body);
            }
        }
        let body = self.fetch_network(url).await?;
        if let Some(cache) = self.inner.cache.clone() {
            let (key, data, events) = (url.to_owned(), body.clone(), self.events().clone());
            tokio::task::spawn_blocking(move || {
                if let Err(e) = cache.put(&key, &data) {
                    events.warn(format!("could not cache {key}: {e}"));
                }
            });
        }
        Ok(body)
    }

    pub async fn get_text(&self, url: &str) -> Result<String, FetchError> {
        Ok(text(self.get(url).await?))
    }

    pub async fn get_text_immutable(&self, url: &str) -> Result<String, FetchError> {
        Ok(text(self.get_immutable(url).await?))
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, FetchError> {
        let body = self.get(url).await?;
        serde_json::from_slice(&body).map_err(|e| FetchError::parse(url, e))
    }

    async fn fetch_network(&self, url: &str) -> Result<Bytes, FetchError> {
        let _permit = self
            .inner
            .limiter
            .acquire()
            .await
            .map_err(|e| FetchError::Other(e.to_string()))?;
        let started = Instant::now();
        let result = self.inner.transport.get(url).await;
        let ms = started.elapsed().as_millis();
        match &result {
            Ok(body) => self
                .events()
                .info(format!("GET {url} -> {} KB in {ms} ms", body.len() / 1024)),
            Err(FetchError::NotFound(_)) => self
                .events()
                .info(format!("GET {url} -> not published ({ms} ms)")),
            Err(e) => self
                .events()
                .warn(format!("GET {url} failed after {ms} ms: {e}")),
        }
        result
    }
}

/// Decode as UTF-8, replacing invalid bytes rather than failing: a stray
/// Windows-1252 character in a report should not lose the whole report.
fn text(body: Bytes) -> String {
    match String::from_utf8(body.to_vec()) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}
