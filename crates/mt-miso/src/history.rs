//! The price history: the day store held to a window of days, and the
//! backfill that downloads the days the window is missing.
//!
//! The store is its own record of progress. Each pass lists what is on disk
//! ([`Inventory`]), removes the days before the window, plans what is missing
//! ([`plan`], newest first) and fetches it through the same path the panels
//! use, one report at a time and [`PACE`] apart. A backfill that is paused, or
//! cut short by closing the terminal, therefore carries on where it stopped,
//! and a day a panel downloaded in the meantime is not downloaded again. Once
//! the window is full the backfill keeps it so, looking again every hour for
//! a new day and for final RT reports to replace preliminary ones.
//!
//! Charts read the store through [`StoredPricesQuery`]: only their nodes'
//! rows, from each day's file, re-read only when a file changes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use chrono::{NaiveDate, TimeDelta};
use mt_core::time::market_today;
use mt_core::{DayLmpReport, DayNodeRow, DayReportKind, Market};
use mt_data::{DiskCache, FetchCtx, FetchError, Freshness, Query};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::endpoints::MisoEndpoints;
use crate::queries::{day_store_dir, day_store_key, fetch_report};

/// MISO's first day of daily LMP reports: earlier days answer 404.
pub const FIRST_DAY: NaiveDate = NaiveDate::from_ymd_opt(2023, 1, 1).expect("a date");

/// RT final reports come five to six days after the market day (and MISO
/// then removes the preliminary one). Younger days only have a preliminary
/// report, so the final is not asked for.
pub const FINAL_LAG_DAYS: i64 = 5;

/// Time between two reports: everything since 2023, about 2,800 reports,
/// takes about an hour and a half.
pub const PACE: Duration = Duration::from_secs(2);

/// How often a full window is looked at again: a new day, a final report.
const RECHECK: Duration = Duration::from_secs(60 * 60);

/// The wait before the first download after launch, while the panels fetch
/// what they show.
const STARTUP_GRACE: Duration = Duration::from_secs(15);

/// Waits after a failed download, doubling from the first to the last.
const RETRY_FIRST: Duration = Duration::from_secs(10);
const RETRY_MAX: Duration = Duration::from_secs(10 * 60);

/// About what one stored report takes on disk (every node, gzipped) and what
/// MISO sends for it (the CSV, not compressed): for estimates before anything
/// is downloaded.
pub const STORED_BYTES_PER_REPORT: u64 = 170 * 1024;
pub const DOWNLOAD_BYTES_PER_REPORT: u64 = 1_300_000;

/// `[price_history]` in config.toml.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// Days of MISO's daily DA and RT reports to keep, counting back from
    /// today (92 is about three months); 0 keeps every day back to MISO's
    /// first, 2023-01-01. Days before the window are removed.
    pub keep_days: u32,
    /// Download the days the window is missing, newest first, and keep it
    /// filled. Off until it is started (SET's *Download now*).
    pub backfill: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            keep_days: 92,
            backfill: false,
        }
    }
}

impl HistoryConfig {
    /// The window's first day.
    pub fn first_day(&self, today: NaiveDate) -> NaiveDate {
        match self.keep_days {
            0 => FIRST_DAY,
            n => (today - TimeDelta::days(i64::from(n) - 1)).max(FIRST_DAY),
        }
    }
}

/// One stored day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub kind: DayReportKind,
    /// Size on disk.
    pub bytes: u64,
}

/// What the day store holds, by market and day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub da: BTreeMap<NaiveDate, Stored>,
    pub rt: BTreeMap<NaiveDate, Stored>,
}

impl Inventory {
    /// List the store: one directory walk and, for RT days, the first bytes
    /// of each file (final or preliminary). Blocking.
    pub fn scan(cache: &DiskCache) -> Self {
        let mut inv = Self::default();
        for market in [Market::DayAhead, Market::RealTime] {
            for (day, stamp) in stored_files(cache, market) {
                let bytes = stamp.len;
                let kind = match market {
                    // Only ex-post DA reports are kept.
                    Market::DayAhead => Some(DayReportKind::DaExPost),
                    Market::RealTime => cache
                        .head(&day_store_key(market, day), DayLmpReport::HEADER_LEN)
                        .and_then(|h| DayLmpReport::peek(&h))
                        .filter(|(_, named)| *named == day)
                        .map(|(kind, _)| kind),
                };
                // An unreadable file counts as missing and is fetched again.
                if let Some(kind) = kind {
                    inv.days_mut(market).insert(day, Stored { kind, bytes });
                }
            }
        }
        inv
    }

    pub fn days(&self, market: Market) -> &BTreeMap<NaiveDate, Stored> {
        match market {
            Market::DayAhead => &self.da,
            Market::RealTime => &self.rt,
        }
    }

    fn days_mut(&mut self, market: Market) -> &mut BTreeMap<NaiveDate, Stored> {
        match market {
            Market::DayAhead => &mut self.da,
            Market::RealTime => &mut self.rt,
        }
    }

    /// How much of the window from `first` the store holds on `today`.
    pub fn coverage(&self, first: NaiveDate, today: NaiveDate) -> Coverage {
        let span = |last: NaiveDate| usize::try_from((last - first).num_days() + 1).unwrap_or(0);
        let yesterday = today - TimeDelta::days(1);
        let da = self.da.range(first..=today);
        let rt = || self.rt.range(first..=yesterday);
        Coverage {
            da_days: span(today),
            rt_days: span(yesterday),
            da_stored: da.count(),
            rt_stored: rt().count(),
            rt_prelim: rt()
                .filter(|(_, s)| s.kind == DayReportKind::RtPrelim)
                .count(),
            bytes: self
                .da
                .values()
                .chain(self.rt.values())
                .map(|s| s.bytes)
                .sum(),
        }
    }
}

/// How much of the window the store holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Days in the window: DA through today, RT through yesterday.
    pub da_days: usize,
    pub rt_days: usize,
    /// Of those, the days stored.
    pub da_stored: usize,
    pub rt_stored: usize,
    /// Stored RT days with only the preliminary report so far.
    pub rt_prelim: usize,
    /// The whole store on disk.
    pub bytes: u64,
}

impl Coverage {
    /// Reports the window is missing.
    pub fn missing(&self) -> usize {
        (self.da_days - self.da_stored.min(self.da_days))
            + (self.rt_days - self.rt_stored.min(self.rt_days))
    }
}

/// A stored day's file as the directory lists it: a change of either means
/// the day was written again (a final report over a preliminary one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

/// Each stored day's file in one market's directory.
fn stored_files(cache: &DiskCache, market: Market) -> Vec<(NaiveDate, Stamp)> {
    let dir = day_store_dir(cache).join(match market {
        Market::DayAhead => "da",
        Market::RealTime => "rt",
    });
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let day = e
                .file_name()
                .to_str()?
                .strip_suffix(".gz")?
                .parse::<NaiveDate>()
                .ok()?;
            let meta = e.metadata().ok();
            let stamp = Stamp {
                len: meta.as_ref().map_or(0, std::fs::Metadata::len),
                modified: meta.and_then(|m| m.modified().ok()),
            };
            Some((day, stamp))
        })
        .collect()
}

/// Remove the stored days before `first`. Returns the days removed and the
/// bytes freed. Blocking.
pub fn prune_day_store(cache: &DiskCache, first: NaiveDate) -> (usize, u64) {
    let (mut days, mut bytes) = (0, 0);
    for market in [Market::DayAhead, Market::RealTime] {
        for (day, stamp) in stored_files(cache, market) {
            if day < first
                && std::fs::remove_file(cache.path_for(&day_store_key(market, day))).is_ok()
            {
                days += 1;
                bytes += stamp.len;
            }
        }
    }
    (days, bytes)
}

/// How often the stored prices behind a chart look for days written since:
/// a directory listing, plus reading only the files that changed.
const STORED_REFRESH: Duration = Duration::from_secs(60);

/// Some nodes' hourly prices from the day store, from a first day on: what
/// GP, SPRD, CMP and HUBS read for any day the store holds, instead of whole
/// days of every node.
#[derive(Clone, Debug, Default)]
pub struct StoredPrices {
    nodes: Arc<[String]>,
    da: BTreeMap<NaiveDate, StoredDay>,
    rt: BTreeMap<NaiveDate, StoredDay>,
}

/// One stored day of a [`StoredPrices`].
#[derive(Clone, Debug)]
struct StoredDay {
    kind: DayReportKind,
    stamp: Stamp,
    /// One per node asked for; `None` where the day has no such node.
    rows: Vec<Option<DayNodeRow>>,
}

impl StoredPrices {
    fn days(&self, market: Market) -> &BTreeMap<NaiveDate, StoredDay> {
        match market {
            Market::DayAhead => &self.da,
            Market::RealTime => &self.rt,
        }
    }

    /// Which report the store holds for a day, if any.
    pub fn stored(&self, market: Market, day: NaiveDate) -> Option<DayReportKind> {
        self.days(market).get(&day).map(|d| d.kind)
    }

    /// A node's row for a stored day (`None` if the day is not stored, or
    /// has no such node).
    pub fn row(&self, node: &str, market: Market, day: NaiveDate) -> Option<&DayNodeRow> {
        let i = self.nodes.iter().position(|n| n == node)?;
        self.days(market).get(&day)?.rows.get(i)?.as_ref()
    }

    /// Stored days, both markets.
    pub fn len(&self) -> usize {
        self.da.len() + self.rt.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// List the store from `first` on and read each day for `nodes`, reusing
    /// what `prev` read from files that have not changed since. Blocking.
    pub fn read(
        cache: &DiskCache,
        nodes: &Arc<[String]>,
        first: NaiveDate,
        prev: Option<&Self>,
    ) -> Self {
        let names: Vec<&str> = nodes.iter().map(String::as_str).collect();
        let mut out = Self {
            nodes: nodes.clone(),
            ..Self::default()
        };
        for market in [Market::DayAhead, Market::RealTime] {
            for (day, stamp) in stored_files(cache, market) {
                if day < first {
                    continue;
                }
                let kept = prev
                    .and_then(|p| p.days(market).get(&day))
                    .filter(|d| d.stamp == stamp);
                let read = || {
                    let bytes = cache.read(&day_store_key(market, day))?;
                    let (kind, named, rows) = DayLmpReport::rows_from_bytes(&bytes, &names)?;
                    (named == day && kind.market() == market).then_some(StoredDay {
                        kind,
                        stamp,
                        rows,
                    })
                };
                // An unreadable file is left out: the panels download that day.
                if let Some(d) = kept.cloned().or_else(read) {
                    match market {
                        Market::DayAhead => out.da.insert(day, d),
                        Market::RealTime => out.rt.insert(day, d),
                    };
                }
            }
        }
        out
    }
}

/// [`StoredPrices`] for some nodes from a first day on, kept fresh as the
/// store grows. Nothing is downloaded.
#[derive(Clone, Debug)]
pub struct StoredPricesQuery {
    nodes: Arc<[String]>,
    first: NaiveDate,
}

impl StoredPricesQuery {
    pub fn new(nodes: &[&str], first: NaiveDate) -> Self {
        Self {
            nodes: nodes.iter().map(|n| (*n).to_owned()).collect(),
            first,
        }
    }
}

impl Query for StoredPricesQuery {
    type Output = StoredPrices;

    fn key(&self) -> String {
        format!("miso/stored/{}/{}", self.first, self.nodes.join(","))
    }

    fn label(&self) -> String {
        let nodes = match &*self.nodes {
            [one] => one.clone(),
            many => format!("{} nodes", many.len()),
        };
        format!("Price history, {nodes} from {}", self.first)
    }

    fn freshness(&self, _: &StoredPrices) -> Freshness {
        Freshness::Every(STORED_REFRESH)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<StoredPrices>>,
    ) -> Result<StoredPrices, FetchError> {
        // A replay has no store: every day comes from the reports.
        let Some(cache) = ctx.cache().cloned() else {
            return Ok(StoredPrices::default());
        };
        let (nodes, first) = (self.nodes.clone(), self.first);
        tokio::task::spawn_blocking(move || {
            StoredPrices::read(&cache, &nodes, first, prev.as_deref())
        })
        .await
        .map_err(|e| FetchError::Other(format!("reading the price history: {e}")))
    }
}

/// One day's report to fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Job {
    pub market: Market,
    pub day: NaiveDate,
}

impl std::fmt::Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.market.label(), self.day)
    }
}

/// What the window from `first` is missing on `today`, newest first: DA
/// ex-post through today, RT through yesterday, and preliminary RT days old
/// enough for their final report.
pub fn plan(inv: &Inventory, first: NaiveDate, today: NaiveDate) -> Vec<Job> {
    let settled = today - TimeDelta::days(FINAL_LAG_DAYS);
    let mut jobs = Vec::new();
    for day in first.iter_days().take_while(|d| *d <= today) {
        if day < today {
            match inv.rt.get(&day).map(|s| s.kind) {
                Some(DayReportKind::RtFinal) => {}
                Some(_) if day > settled => {}
                _ => jobs.push(Job {
                    market: Market::RealTime,
                    day,
                }),
            }
        }
        if !inv.da.contains_key(&day) {
            jobs.push(Job {
                market: Market::DayAhead,
                day,
            });
        }
    }
    jobs.reverse();
    jobs
}

/// Fetch one job's report into the store. `Some`: what the store holds for
/// the day now; `None`: MISO has not published it (yet).
async fn fetch_job(
    ctx: &FetchCtx,
    endpoints: &MisoEndpoints,
    job: Job,
    stored: Option<DayReportKind>,
    today: NaiveDate,
) -> Result<Option<DayReportKind>, FetchError> {
    let fetch = |kind| async move {
        fetch_report(ctx, endpoints, kind, job.day)
            .await
            .map(|r| r.map(|r| r.kind))
    };
    match job.market {
        Market::DayAhead => fetch(DayReportKind::DaExPost).await,
        Market::RealTime => {
            if job.day <= today - TimeDelta::days(FINAL_LAG_DAYS) {
                if let Some(kind) = fetch(DayReportKind::RtFinal).await? {
                    return Ok(Some(kind));
                }
                // Not settled yet: the preliminary report stands meanwhile.
                if stored == Some(DayReportKind::RtPrelim) {
                    return Ok(stored);
                }
            }
            fetch(DayReportKind::RtPrelim).await
        }
    }
}

/// What the backfill is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    /// Waiting for its settings.
    #[default]
    Starting,
    /// Nothing to keep: an offline replay, or no disk cache.
    Unavailable,
    /// Listing the store and removing the days before the window.
    Scanning,
    /// Not downloading: the store keeps only what panels download.
    Off,
    /// Off while fetching is paused (LOG's *Pause fetching*).
    Held,
    Downloading(Job),
    /// A download failed; trying again at `at`.
    Retrying {
        job: Job,
        error: String,
        at: Instant,
    },
    /// The window is full, or as full as MISO allows; looking again hourly.
    UpToDate,
}

/// The backfill's progress, for SET, LOG and the status bar.
#[derive(Clone, Debug, Default)]
pub struct BackfillStatus {
    pub phase: Phase,
    /// The window's first day, as of the last pass.
    pub first_day: Option<NaiveDate>,
    pub coverage: Coverage,
    /// Reports this pass set out to fetch, and how many are done.
    pub planned: usize,
    pub done: usize,
    /// Reports MISO has not published (yet): asked for again next pass.
    pub not_published: Vec<Job>,
    /// Reports that could not be read, and why: skipped until the next pass.
    pub unreadable: Vec<(Job, String)>,
    /// When this pass's first download started.
    pub started: Option<Instant>,
    /// Days removed from before the window since launch.
    pub pruned: usize,
}

impl BackfillStatus {
    pub fn remaining(&self) -> usize {
        self.planned.saturating_sub(self.done)
    }

    /// Downloading, or waiting to try a download again.
    pub fn is_working(&self) -> bool {
        matches!(self.phase, Phase::Downloading(_) | Phase::Retrying { .. })
    }

    /// About how long the rest takes, at this pass's pace so far.
    pub fn eta(&self) -> Option<Duration> {
        let per = match (self.started, self.done) {
            (Some(t), done @ 3..) => t.elapsed() / u32::try_from(done).ok()?,
            _ => PACE + Duration::from_secs(1),
        };
        per.checked_mul(u32::try_from(self.remaining()).ok()?)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    endpoints: MisoEndpoints,
    config: HistoryConfig,
    held: bool,
}

type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Downloads the days the price history's window is missing, on the data
/// runtime, one report at a time. Cheap to clone. See the module docs.
#[derive(Clone)]
pub struct Backfill {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: FetchCtx,
    pace: Duration,
    /// The first wait after a failed download; it doubles to [`RETRY_MAX`].
    retry: Duration,
    /// How long the first pass after launch waits before downloading.
    grace: Duration,
    /// `None` until [`Backfill::configure`] is first called: nothing is
    /// listed, removed or fetched before the real settings are known.
    settings: Mutex<Option<Settings>>,
    /// Moves on every change of settings; a pass that sees it move starts over.
    version: AtomicU64,
    wake: Notify,
    status: Mutex<BackfillStatus>,
    notify: Mutex<Option<Notifier>>,
}

enum Flow {
    Next,
    /// The settings changed: plan again.
    Restart,
}

impl Backfill {
    /// A backfill for `ctx`'s day store, working on `runtime`. It waits for
    /// [`Self::configure`]; with no disk cache, or a replay, it does nothing.
    pub fn new(ctx: FetchCtx, runtime: &tokio::runtime::Handle) -> Self {
        Self::build(ctx, runtime, PACE, RETRY_FIRST, STARTUP_GRACE)
    }

    /// Another pace and first wait after a failure, and no grace at launch,
    /// for tests.
    pub fn with_timing(
        ctx: FetchCtx,
        runtime: &tokio::runtime::Handle,
        pace: Duration,
        retry: Duration,
    ) -> Self {
        Self::build(ctx, runtime, pace, retry, Duration::ZERO)
    }

    fn build(
        ctx: FetchCtx,
        runtime: &tokio::runtime::Handle,
        pace: Duration,
        retry: Duration,
        grace: Duration,
    ) -> Self {
        let usable = ctx.is_live() && ctx.cache().is_some();
        let inner = Arc::new(Inner {
            ctx,
            pace,
            retry,
            grace,
            settings: Mutex::default(),
            version: AtomicU64::new(0),
            wake: Notify::new(),
            status: Mutex::new(BackfillStatus {
                phase: if usable {
                    Phase::Starting
                } else {
                    Phase::Unavailable
                },
                ..BackfillStatus::default()
            }),
            notify: Mutex::default(),
        });
        if usable {
            runtime.spawn(inner.clone().work());
        }
        Self { inner }
    }

    /// Called (from a background thread) whenever the status changes.
    pub fn set_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.notify.lock() = Some(Arc::new(f));
    }

    /// The window, whether to download, and where from. `held` stops
    /// downloads while fetching is paused, without changing the setting.
    /// Only a change starts a new pass, so calling it again is cheap.
    pub fn configure(&self, endpoints: &MisoEndpoints, config: &HistoryConfig, held: bool) {
        let next = Settings {
            endpoints: endpoints.clone(),
            config: config.clone(),
            held,
        };
        let mut settings = self.inner.settings.lock();
        if settings.as_ref() == Some(&next) {
            return;
        }
        *settings = Some(next);
        drop(settings);
        self.inner.version.fetch_add(1, Ordering::SeqCst);
        self.inner.wake.notify_one();
    }

    pub fn status(&self) -> BackfillStatus {
        self.inner.status.lock().clone()
    }
}

impl Inner {
    fn update(&self, f: impl FnOnce(&mut BackfillStatus)) {
        f(&mut self.status.lock());
        let notify = self.notify.lock().clone();
        if let Some(n) = notify {
            n();
        }
    }

    fn changed(&self, version: u64) -> bool {
        self.version.load(Ordering::SeqCst) != version
    }

    /// Wait for `d`, or less if the settings change. Returns whether they did.
    async fn sleep(&self, d: Duration, version: u64) -> bool {
        let deadline = tokio::time::Instant::now() + d;
        while !self.changed(version) {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => break,
                () = self.wake.notified() => {}
            }
        }
        self.changed(version)
    }

    async fn work(self: Arc<Self>) {
        let Some(cache) = self.ctx.cache().cloned() else {
            return;
        };
        let mut grace = self.grace;
        loop {
            let version = self.version.load(Ordering::SeqCst);
            let Some(settings) = self.settings.lock().clone() else {
                self.wake.notified().await;
                continue;
            };
            // At launch the panels' own fetches go first, so a day they are
            // downloading is found in the store rather than downloaded twice.
            let wait = std::mem::take(&mut grace);
            if settings.config.backfill && !settings.held && self.sleep(wait, version).await {
                continue;
            }
            let today = market_today();
            let first = settings.config.first_day(today);
            self.update(|s| s.phase = Phase::Scanning);
            let scan = cache.clone();
            let (inventory, (pruned, freed)) = tokio::task::spawn_blocking(move || {
                let pruned = prune_day_store(&scan, first);
                (Inventory::scan(&scan), pruned)
            })
            .await
            .unwrap_or_default();
            if pruned > 0 {
                self.ctx.events().info(format!(
                    "price history: removed {pruned} reports before {first} ({} MB)",
                    freed / 1_048_576
                ));
            }
            let on = settings.config.backfill && !settings.held;
            let jobs = match on {
                true => plan(&inventory, first, today),
                false => Vec::new(),
            };
            match jobs.len() {
                0 => {}
                1 => self
                    .ctx
                    .events()
                    .info(format!("price history: 1 report to fetch for {}", jobs[0])),
                n => self.ctx.events().info(format!(
                    "price history: {n} reports to fetch from {first}, newest first"
                )),
            }
            self.update(|s| {
                *s = BackfillStatus {
                    phase: match (settings.config.backfill, settings.held, jobs.first()) {
                        (false, _, _) => Phase::Off,
                        (true, true, _) => Phase::Held,
                        (true, false, Some(job)) => Phase::Downloading(*job),
                        (true, false, None) => Phase::UpToDate,
                    },
                    first_day: Some(first),
                    coverage: inventory.coverage(first, today),
                    planned: jobs.len(),
                    pruned: s.pruned + pruned,
                    ..BackfillStatus::default()
                };
            });
            let mut inventory = inventory;
            let mut restart = false;
            for job in jobs {
                let flow = self
                    .run(
                        &settings.endpoints,
                        job,
                        &mut inventory,
                        first,
                        today,
                        version,
                    )
                    .await;
                if matches!(flow, Flow::Restart) || self.sleep(self.pace, version).await {
                    restart = true;
                    break;
                }
            }
            if restart {
                continue;
            }
            if on {
                self.update(|s| s.phase = Phase::UpToDate);
            }
            self.sleep(RECHECK, version).await;
        }
    }

    /// Fetch one report, trying again after failures until it is in, MISO
    /// says it is not published, or it cannot be read.
    async fn run(
        &self,
        endpoints: &MisoEndpoints,
        job: Job,
        inventory: &mut Inventory,
        first: NaiveDate,
        today: NaiveDate,
        version: u64,
    ) -> Flow {
        let mut wait = self.retry;
        loop {
            if self.changed(version) {
                return Flow::Restart;
            }
            self.update(|s| {
                s.phase = Phase::Downloading(job);
                s.started.get_or_insert_with(Instant::now);
            });
            let stored = inventory.days(job.market).get(&job.day).map(|s| s.kind);
            match fetch_job(&self.ctx, endpoints, job, stored, today).await {
                Ok(Some(kind)) => {
                    let bytes = self
                        .ctx
                        .cache()
                        .map(|c| c.path_for(&day_store_key(job.market, job.day)))
                        .and_then(|p| std::fs::metadata(p).ok())
                        .map_or(0, |m| m.len());
                    inventory
                        .days_mut(job.market)
                        .insert(job.day, Stored { kind, bytes });
                    self.update(|s| {
                        s.done += 1;
                        s.coverage = inventory.coverage(first, today);
                    });
                    return Flow::Next;
                }
                Ok(None) => {
                    self.update(|s| {
                        s.done += 1;
                        s.not_published.push(job);
                    });
                    return Flow::Next;
                }
                Err(e @ FetchError::Parse { .. }) => {
                    self.ctx
                        .events()
                        .warn(format!("price history: skipped {job}: {e}"));
                    self.update(|s| {
                        s.done += 1;
                        s.unreadable.push((job, e.to_string()));
                    });
                    return Flow::Next;
                }
                Err(e) => {
                    let error = e.to_string();
                    self.update(|s| {
                        s.phase = Phase::Retrying {
                            job,
                            error,
                            at: Instant::now() + wait,
                        };
                    });
                    if self.sleep(wait, version).await {
                        return Flow::Restart;
                    }
                    wait = (wait * 2).min(RETRY_MAX);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn stored(kind: DayReportKind) -> Stored {
        Stored { kind, bytes: 100 }
    }

    #[test]
    fn the_window_counts_back_from_today_and_stops_at_misos_first_day() {
        let today = d("2026-10-08");
        let three_months = HistoryConfig::default();
        assert_eq!(three_months.first_day(today), d("2026-07-09"));
        let everything = HistoryConfig {
            keep_days: 0,
            ..HistoryConfig::default()
        };
        assert_eq!(everything.first_day(today), FIRST_DAY);
        let ten_years = HistoryConfig {
            keep_days: 3650,
            ..HistoryConfig::default()
        };
        assert_eq!(ten_years.first_day(today), FIRST_DAY);
        let one = HistoryConfig {
            keep_days: 1,
            ..HistoryConfig::default()
        };
        assert_eq!(one.first_day(today), today);
    }

    #[test]
    fn plans_what_is_missing_newest_first() {
        let today = d("2026-10-08");
        let first = d("2026-09-30");
        let mut inv = Inventory::default();
        inv.da
            .insert(d("2026-10-08"), stored(DayReportKind::DaExPost));
        inv.da
            .insert(d("2026-10-01"), stored(DayReportKind::DaExPost));
        // Settled: nothing to do.
        inv.rt
            .insert(d("2026-10-01"), stored(DayReportKind::RtFinal));
        // Preliminary and old enough for its final: asked again.
        inv.rt
            .insert(d("2026-10-02"), stored(DayReportKind::RtPrelim));
        inv.rt
            .insert(d("2026-10-03"), stored(DayReportKind::RtPrelim));
        // Preliminary and too young for a final: left alone.
        inv.rt
            .insert(d("2026-10-04"), stored(DayReportKind::RtPrelim));
        // Before the window: not planned, but counted on disk.
        inv.da
            .insert(d("2026-09-29"), stored(DayReportKind::DaExPost));

        let jobs: Vec<String> = plan(&inv, first, today)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            jobs,
            [
                "DA 2026-10-07",
                "RT 2026-10-07",
                "DA 2026-10-06",
                "RT 2026-10-06",
                "DA 2026-10-05",
                "RT 2026-10-05",
                "DA 2026-10-04",
                "DA 2026-10-03",
                "RT 2026-10-03",
                "DA 2026-10-02",
                "RT 2026-10-02",
                "DA 2026-09-30",
                "RT 2026-09-30",
            ]
        );

        let c = inv.coverage(first, today);
        assert_eq!((c.da_days, c.rt_days), (9, 8));
        assert_eq!((c.da_stored, c.rt_stored, c.rt_prelim), (2, 4, 3));
        assert_eq!(c.missing(), 7 + 4);
        assert_eq!(c.bytes, 700, "the whole store");
    }

    #[test]
    fn eta_follows_the_pace_so_far() {
        let mut s = BackfillStatus {
            planned: 10,
            ..BackfillStatus::default()
        };
        assert_eq!(
            s.eta(),
            Some(Duration::from_secs(30)),
            "before any download"
        );
        s.done = 4;
        s.started = Instant::now().checked_sub(Duration::from_secs(8));
        let eta = s.eta().unwrap();
        assert!(
            eta >= Duration::from_secs(12) && eta < Duration::from_secs(13),
            "{eta:?}"
        );
    }
}
