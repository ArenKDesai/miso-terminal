use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use crate::{FetchCtx, FetchError, Freshness, Query};

type AnyValue = Arc<dyn Any + Send + Sync>;
type Refetch = Arc<dyn Fn(&DataHub) + Send + Sync>;
type Notify = Arc<dyn Fn() + Send + Sync>;

/// First retry delay after a failure; doubles per consecutive failure.
const BASE_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// An entry counts as "watched" if a panel asked for it this recently.
const WATCH_WINDOW: Duration = Duration::from_secs(5);

/// What a panel gets back from [`DataHub::watch`]: the latest value (possibly
/// stale) plus enough status to render loading and error states honestly.
pub struct Snapshot<T> {
    pub data: Option<Arc<T>>,
    pub updated: Option<DateTime<Utc>>,
    pub loading: bool,
    pub error: Option<FetchError>,
    /// The value has missed at least one scheduled refresh.
    pub stale: bool,
    /// Increments every time a new value lands; cheap change detection.
    pub generation: u64,
}

impl<T> Default for Snapshot<T> {
    fn default() -> Self {
        Self {
            data: None,
            updated: None,
            loading: false,
            error: None,
            stale: false,
            generation: 0,
        }
    }
}

impl<T> Snapshot<T> {
    pub fn data(&self) -> Option<&T> {
        self.data.as_deref()
    }

    /// The same status without the data, for code that only renders freshness.
    pub fn status(&self) -> Snapshot<()> {
        Snapshot {
            data: self.data.as_ref().map(|_| Arc::new(())),
            updated: self.updated,
            loading: self.loading,
            error: self.error.clone(),
            stale: self.stale,
            generation: self.generation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryState {
    Empty,
    Loading,
    Fresh,
    Stale,
    Error,
}

/// One row of the data-feed log.
#[derive(Clone, Debug)]
pub struct EntryStatus {
    pub key: String,
    pub label: String,
    pub state: EntryState,
    pub updated: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub failures: u32,
    pub last_duration: Option<Duration>,
    pub fetches: u64,
    pub watched: bool,
}

struct Entry {
    label: String,
    value: Option<AnyValue>,
    updated: Option<(Instant, DateTime<Utc>)>,
    fresh_for: Option<Duration>,
    in_flight: bool,
    error: Option<(FetchError, Instant)>,
    failures: u32,
    last_watched: Instant,
    last_duration: Option<Duration>,
    fetches: u64,
    generation: u64,
    refetch: Refetch,
}

impl Entry {
    fn new<Q: Query>(q: &Q) -> Self {
        let q2 = q.clone();
        Self {
            label: q.label(),
            value: None,
            updated: None,
            fresh_for: None,
            in_flight: false,
            error: None,
            failures: 0,
            last_watched: Instant::now(),
            last_duration: None,
            fetches: 0,
            generation: 0,
            refetch: Arc::new(move |hub: &DataHub| hub.refresh(&q2)),
        }
    }

    fn backoff_elapsed(&self, now: Instant) -> bool {
        match &self.error {
            Some((_, at)) if self.failures > 0 => {
                let wait = BASE_BACKOFF
                    .saturating_mul(1 << (self.failures - 1).min(10))
                    .min(MAX_BACKOFF);
                now.duration_since(*at) >= wait
            }
            _ => true,
        }
    }

    fn age(&self, now: Instant) -> Option<Duration> {
        self.updated.map(|(at, _)| now.duration_since(at))
    }

    fn is_due(&self, now: Instant) -> bool {
        if self.in_flight || !self.backoff_elapsed(now) {
            return false;
        }
        match (self.age(now), self.fresh_for) {
            (None, _) => true,
            (Some(age), Some(fresh)) => age >= fresh,
            (Some(_), None) => false,
        }
    }

    fn is_stale(&self, now: Instant) -> bool {
        match (self.age(now), self.fresh_for) {
            (Some(age), Some(fresh)) => age > fresh + fresh.max(Duration::from_secs(30)),
            _ => false,
        }
    }

    fn state(&self, now: Instant) -> EntryState {
        if self.failures > 0 {
            EntryState::Error
        } else if self.in_flight {
            EntryState::Loading
        } else if self.value.is_none() {
            EntryState::Empty
        } else if self.is_stale(now) {
            EntryState::Stale
        } else {
            EntryState::Fresh
        }
    }

    fn snapshot<T: Send + Sync + 'static>(&self, now: Instant) -> Snapshot<T> {
        Snapshot {
            data: self.value.clone().and_then(|v| v.downcast::<T>().ok()),
            updated: self.updated.map(|(_, wall)| wall),
            loading: self.in_flight,
            error: self.error.as_ref().map(|(e, _)| e.clone()),
            stale: self.is_stale(now),
            generation: self.generation,
        }
    }

    fn store(&mut self, value: AnyValue, fresh: Freshness) {
        self.fresh_for = match fresh {
            Freshness::Every(d) => Some(d),
            Freshness::Forever => None,
        };
        self.value = Some(value);
        self.updated = Some((Instant::now(), Utc::now()));
        self.error = None;
        self.failures = 0;
        self.generation += 1;
    }
}

/// Deduplicating, caching, self-refreshing store of query results.
///
/// Call [`DataHub::watch`] from UI code every frame: it returns immediately with
/// whatever is cached and, if the value is missing or due for a refresh, starts
/// a background fetch. When the fetch lands the hub calls the notify callback
/// (the app wires this to `request_repaint`). Queries nobody watches stop
/// refreshing, and [`DataHub::gc`] eventually drops them.
#[derive(Clone)]
pub struct DataHub {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: tokio::runtime::Handle,
    ctx: FetchCtx,
    entries: Mutex<HashMap<String, Entry>>,
    notify: Mutex<Option<Notify>>,
    paused: AtomicBool,
}

impl DataHub {
    pub fn new(runtime: tokio::runtime::Handle, ctx: FetchCtx) -> Self {
        Self {
            inner: Arc::new(Inner {
                runtime,
                ctx,
                entries: Mutex::default(),
                notify: Mutex::default(),
                paused: AtomicBool::new(false),
            }),
        }
    }

    pub fn ctx(&self) -> &FetchCtx {
        &self.inner.ctx
    }

    /// Called (from a background thread) whenever a fetch completes.
    pub fn set_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.notify.lock() = Some(Arc::new(f));
    }

    /// While paused no new fetches start; cached values are still served.
    pub fn set_paused(&self, paused: bool) {
        self.inner.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Relaxed)
    }

    /// Current value of `q`, fetching in the background if missing or due.
    pub fn watch<Q: Query>(&self, q: &Q) -> Snapshot<Q::Output> {
        let key = q.key();
        let now = Instant::now();
        let mut entries = self.inner.entries.lock();
        let entry = entries.entry(key.clone()).or_insert_with(|| Entry::new(q));
        entry.last_watched = now;
        let start = !self.is_paused() && entry.is_due(now);
        let prev = if start { self.begin(entry) } else { None };
        let snap = entry.snapshot(now);
        drop(entries);
        if start {
            self.spawn(q.clone(), key, prev);
        }
        snap
    }

    /// Current value of `q` without registering interest or fetching.
    pub fn peek<Q: Query>(&self, q: &Q) -> Snapshot<Q::Output> {
        self.inner
            .entries
            .lock()
            .get(&q.key())
            .map(|e| e.snapshot(Instant::now()))
            .unwrap_or_default()
    }

    /// Fetch now, ignoring freshness and backoff (but not an in-flight fetch).
    pub fn refresh<Q: Query>(&self, q: &Q) {
        let key = q.key();
        let mut entries = self.inner.entries.lock();
        let entry = entries.entry(key.clone()).or_insert_with(|| Entry::new(q));
        if entry.in_flight {
            return;
        }
        let prev = self.begin(entry);
        drop(entries);
        self.spawn(q.clone(), key, prev);
    }

    /// Refresh a query by key (from the data-feed log).
    pub fn refresh_key(&self, key: &str) {
        let refetch = self
            .inner
            .entries
            .lock()
            .get(key)
            .map(|e| e.refetch.clone());
        if let Some(f) = refetch {
            f(self);
        }
    }

    /// Refresh everything a panel is currently watching.
    pub fn refresh_watched(&self) {
        let now = Instant::now();
        let fns: Vec<Refetch> = self
            .inner
            .entries
            .lock()
            .values()
            .filter(|e| now.duration_since(e.last_watched) < WATCH_WINDOW)
            .map(|e| e.refetch.clone())
            .collect();
        for f in fns {
            f(self);
        }
    }

    /// Put a value in directly: offline demos, tests, values derived elsewhere.
    pub fn seed<Q: Query>(&self, q: &Q, value: Q::Output) {
        let fresh = q.freshness(&value);
        let mut entries = self.inner.entries.lock();
        let entry = entries.entry(q.key()).or_insert_with(|| Entry::new(q));
        entry.store(Arc::new(value), fresh);
    }

    /// Drop entries nobody has watched for `idle`. Returns how many were dropped.
    pub fn gc(&self, idle: Duration) -> usize {
        let now = Instant::now();
        let mut entries = self.inner.entries.lock();
        let before = entries.len();
        entries.retain(|_, e| e.in_flight || now.duration_since(e.last_watched) < idle);
        before - entries.len()
    }

    pub fn in_flight(&self) -> usize {
        self.inner
            .entries
            .lock()
            .values()
            .filter(|e| e.in_flight)
            .count()
    }

    pub fn status(&self) -> Vec<EntryStatus> {
        let now = Instant::now();
        let mut out: Vec<EntryStatus> = self
            .inner
            .entries
            .lock()
            .iter()
            .map(|(key, e)| EntryStatus {
                key: key.clone(),
                label: e.label.clone(),
                state: e.state(now),
                updated: e.updated.map(|(_, wall)| wall),
                error: e.error.as_ref().map(|(err, _)| err.to_string()),
                failures: e.failures,
                last_duration: e.last_duration,
                fetches: e.fetches,
                watched: now.duration_since(e.last_watched) < WATCH_WINDOW,
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Mark an entry in flight and hand back its current value for the fetch.
    fn begin<T: Send + Sync + 'static>(&self, entry: &mut Entry) -> Option<Arc<T>> {
        entry.in_flight = true;
        entry.fetches += 1;
        entry.value.clone().and_then(|v| v.downcast::<T>().ok())
    }

    fn spawn<Q: Query>(&self, q: Q, key: String, prev: Option<Arc<Q::Output>>) {
        let hub = self.clone();
        self.inner.runtime.spawn(async move {
            let started = Instant::now();
            // Run the fetch as its own task so a panicking parser is reported as
            // an error instead of wedging the entry "in flight" forever.
            let (fetcher, ctx) = (q.clone(), hub.inner.ctx.clone());
            let result = match tokio::spawn(async move { fetcher.fetch(ctx, prev).await }).await {
                Ok(r) => r,
                Err(e) => Err(FetchError::Other(format!("fetch task failed: {e}"))),
            };
            hub.complete(&q, &key, result, started.elapsed());
        });
    }

    fn complete<Q: Query>(
        &self,
        q: &Q,
        key: &str,
        result: Result<Q::Output, FetchError>,
        took: Duration,
    ) {
        {
            let mut entries = self.inner.entries.lock();
            let Some(entry) = entries.get_mut(key) else {
                return;
            };
            entry.in_flight = false;
            entry.last_duration = Some(took);
            match result {
                Ok(value) => {
                    let fresh = q.freshness(&value);
                    entry.store(Arc::new(value), fresh);
                }
                Err(err) => {
                    self.inner
                        .ctx
                        .events()
                        .warn(format!("{} failed: {err}", entry.label));
                    entry.failures = entry.failures.saturating_add(1);
                    entry.error = Some((err, Instant::now()));
                }
            }
        }
        let notify = self.inner.notify.lock().clone();
        if let Some(f) = notify {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::{EventLog, FetchCtxOptions, FixtureTransport};

    #[derive(Clone)]
    struct Counter {
        key: &'static str,
        calls: Arc<AtomicUsize>,
        fail: Arc<AtomicBool>,
        fresh: Freshness,
    }

    impl Counter {
        fn new(key: &'static str, fresh: Freshness) -> Self {
            Self {
                key,
                calls: Arc::default(),
                fail: Arc::default(),
                fresh,
            }
        }
    }

    impl Query for Counter {
        type Output = usize;

        fn key(&self) -> String {
            self.key.into()
        }

        fn freshness(&self, _: &usize) -> Freshness {
            self.fresh
        }

        async fn fetch(
            &self,
            _ctx: FetchCtx,
            prev: Option<Arc<usize>>,
        ) -> Result<usize, FetchError> {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail.load(Ordering::SeqCst) {
                return Err(FetchError::Other("boom".into()));
            }
            // Encode the previous value so tests can see `prev` is threaded through.
            Ok(prev.map_or(0, |p| *p) * 10 + n)
        }
    }

    fn hub() -> (tokio::runtime::Runtime, DataHub) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let ctx = FetchCtx::new(
            Arc::new(FixtureTransport::new("unused")),
            None,
            FetchCtxOptions::default(),
            EventLog::default(),
        );
        let hub = DataHub::new(rt.handle().clone(), ctx);
        (rt, hub)
    }

    fn wait_for(mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn watch_fetches_once_and_dedupes() {
        let (_rt, hub) = hub();
        let q = Counter::new("t/a", Freshness::Forever);
        let first = hub.watch(&q);
        assert!(first.data.is_none() && first.loading);
        hub.watch(&q); // in flight: must not start a second fetch
        wait_for(|| hub.peek(&q).data.is_some());
        let snap = hub.watch(&q);
        assert_eq!(snap.data(), Some(&1));
        assert_eq!(snap.generation, 1);
        std::thread::sleep(Duration::from_millis(50));
        hub.watch(&q);
        assert_eq!(
            q.calls.load(Ordering::SeqCst),
            1,
            "Forever must not refetch"
        );
    }

    #[test]
    fn refreshes_when_due_and_threads_prev() {
        let (_rt, hub) = hub();
        let q = Counter::new("t/b", Freshness::Every(Duration::from_millis(30)));
        hub.watch(&q);
        wait_for(|| hub.peek(&q).data.is_some());
        std::thread::sleep(Duration::from_millis(40));
        hub.watch(&q);
        wait_for(|| hub.peek(&q).generation == 2);
        assert_eq!(hub.peek(&q).data(), Some(&12)); // prev 1, call 2
    }

    #[test]
    fn errors_keep_last_value_and_back_off() {
        let (_rt, hub) = hub();
        let q = Counter::new("t/c", Freshness::Every(Duration::ZERO));
        hub.watch(&q);
        wait_for(|| hub.peek(&q).data.is_some());
        q.fail.store(true, Ordering::SeqCst);
        hub.watch(&q);
        wait_for(|| hub.peek(&q).error.is_some());
        let snap = hub.peek(&q);
        assert_eq!(snap.data(), Some(&1), "last good value survives a failure");
        // Backoff: an immediate re-watch must not hammer the source.
        let calls = q.calls.load(Ordering::SeqCst);
        hub.watch(&q);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(q.calls.load(Ordering::SeqCst), calls);
        assert_eq!(hub.status()[0].state, EntryState::Error);
    }

    #[test]
    fn refresh_seed_pause_and_gc() {
        let (_rt, hub) = hub();
        let q = Counter::new("t/d", Freshness::Forever);
        hub.seed(&q, 7);
        assert_eq!(hub.watch(&q).data(), Some(&7));
        assert_eq!(q.calls.load(Ordering::SeqCst), 0);

        hub.refresh_key("t/d");
        wait_for(|| hub.peek(&q).generation == 2);
        assert_eq!(hub.peek(&q).data(), Some(&71));

        let p = Counter::new("t/e", Freshness::Forever);
        hub.set_paused(true);
        assert!(!hub.watch(&p).loading);
        hub.set_paused(false);

        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(hub.gc(Duration::from_millis(10)), 2);
        assert!(hub.status().is_empty());
    }

    #[test]
    fn notify_fires_on_completion() {
        let (_rt, hub) = hub();
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        hub.set_notify(move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        hub.watch(&Counter::new("t/f", Freshness::Forever));
        wait_for(|| fired.load(Ordering::SeqCst) == 1);
    }
}
