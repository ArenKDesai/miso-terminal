//! Forecasts kept as issued: MISO's load, wind and solar forecasts, its
//! outage schedule and the NWS's temperatures, each version stored with when
//! it was issued, so FCST's models can be trained on what was known at the
//! time rather than on what happened.
//!
//! One file per kind per day the versions were issued,
//! `local://archive/issued/<kind>/<date>` (gzipped by the disk cache), as
//! tab-separated text: a `series` line naming the columns, then one line per
//! version, `issued  target  value…`, times in market time (EST). Only values
//! that changed since the last version for the same hour are stored again.
//!
//! A [`Collector`] polls [`Source`]s on their own schedules while the app is
//! live, fills a source's past from its daily reports where it has them, a
//! few seconds apart, and removes days older than the window it keeps.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::{BoxFuture, DiskCache, FetchCtx, FetchError};

const ROOT: &str = "local://archive/issued";
const TIME: &str = "%Y-%m-%dT%H:%M";

/// The key of one kind's file for one day.
pub fn key(kind: &str, day: NaiveDate) -> String {
    format!("{ROOT}/{kind}/{day}")
}

/// The folder holding every kind, for the cache's keep list.
pub fn dir(cache: &DiskCache) -> PathBuf {
    let file = cache.path_for(&key("kind", NaiveDate::default()));
    file.parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// One version of the forecast for one hour (or day).
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub issued: NaiveDateTime,
    pub target: NaiveDateTime,
    pub values: Vec<Option<f64>>,
}

/// A day's versions of one kind, or several days' merged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Issued {
    pub series: Vec<String>,
    pub versions: Vec<Version>,
}

impl Issued {
    pub fn to_text(&self) -> String {
        let mut out = format!("series\t{}\n", self.series.join("\t"));
        for v in &self.versions {
            out += &format!("{}\t{}", v.issued.format(TIME), v.target.format(TIME));
            for x in &v.values {
                out.push('\t');
                if let Some(x) = x {
                    out += &x.to_string();
                }
            }
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let series: Vec<String> = lines
            .next()?
            .strip_prefix("series")?
            .split('\t')
            .skip(1)
            .map(String::from)
            .collect();
        let mut versions = Vec::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let mut f = line.split('\t');
            let time = |s: Option<&str>| NaiveDateTime::parse_from_str(s?, TIME).ok();
            let (issued, target) = (time(f.next())?, time(f.next())?);
            let mut values: Vec<Option<f64>> = f.map(|x| x.parse().ok()).collect();
            values.resize(series.len(), None);
            versions.push(Version {
                issued,
                target,
                values,
            });
        }
        Some(Self { series, versions })
    }

    /// Add versions written with `series` (in any order, possibly new
    /// columns), keeping each value under its series' name.
    pub fn extend(&mut self, series: &[String], versions: impl IntoIterator<Item = Version>) {
        for s in series {
            if !self.series.contains(s) {
                self.series.push(s.clone());
                for v in &mut self.versions {
                    v.values.push(None);
                }
            }
        }
        let at: Vec<usize> = series
            .iter()
            .map(|s| self.series.iter().position(|x| x == s).unwrap_or_default())
            .collect();
        for v in versions {
            let mut values = vec![None; self.series.len()];
            for (i, x) in at.iter().zip(v.values) {
                values[*i] = x;
            }
            self.versions.push(Version { values, ..v });
        }
    }

    /// The column for `series`, if there is one.
    pub fn column(&self, series: &str) -> Option<usize> {
        self.series.iter().position(|s| s == series)
    }
}

pub fn read_day(cache: &DiskCache, kind: &str, day: NaiveDate) -> Option<Issued> {
    Issued::parse(std::str::from_utf8(&cache.read(&key(kind, day))?).ok()?)
}

pub fn write_day(cache: &DiskCache, kind: &str, day: NaiveDate, file: &Issued) -> io::Result<()> {
    cache.put(&key(kind, day), file.to_text().as_bytes())
}

/// The days a kind has files for (including days marked as not published).
pub fn days(cache: &DiskCache, kind: &str) -> BTreeSet<NaiveDate> {
    let folder = dir(cache).join(kind);
    let Ok(entries) = fs::read_dir(folder) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".gz")?.parse().ok()
        })
        .collect()
}

/// Every version of a kind issued from `first` to `last`, oldest first.
pub fn read(cache: &DiskCache, kind: &str, first: NaiveDate, last: NaiveDate) -> Issued {
    let mut out = Issued::default();
    for day in days(cache, kind).range(first..=last) {
        if let Some(f) = read_day(cache, kind, *day) {
            out.extend(&f.series, f.versions);
        }
    }
    out.versions.sort_by_key(|v| (v.issued, v.target));
    out
}

/// Remove every kind's days before `first`: files and bytes removed.
pub fn prune(cache: &DiskCache, first: NaiveDate) -> (usize, u64) {
    let (mut files, mut bytes) = (0, 0);
    let Ok(kinds) = fs::read_dir(dir(cache)) else {
        return (0, 0);
    };
    for kind in kinds
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
    {
        for day in days(cache, &kind).range(..first) {
            let path = cache.path_for(&key(&kind, *day));
            let size = fs::metadata(&path).map_or(0, |m| m.len());
            if fs::remove_file(&path).is_ok() {
                files += 1;
                bytes += size;
            }
        }
    }
    (files, bytes)
}

/// Files and bytes kept, every kind together.
pub fn usage(cache: &DiskCache) -> (usize, u64) {
    fn walk(p: &std::path::Path, out: &mut (usize, u64)) {
        let Ok(rd) = fs::read_dir(p) else { return };
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => walk(&e.path(), out),
                Ok(m) => {
                    out.0 += 1;
                    out.1 += m.len();
                }
                Err(_) => {}
            }
        }
    }
    let mut out = (0, 0);
    walk(&dir(cache), &mut out);
    out
}

/// What a source hands back: versions of one kind issued at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub kind: &'static str,
    pub series: Vec<String>,
    pub issued: NaiveDateTime,
    /// Target hour (or day) and the values for each series.
    pub rows: Vec<(NaiveDateTime, Vec<Option<f64>>)>,
}

/// Somewhere forecasts come from.
pub trait Source: Send + Sync + 'static {
    /// For the status lines in SET and LOG: "MISO wind and solar".
    fn name(&self) -> &'static str;

    /// How often to ask.
    fn every(&self) -> Duration;

    /// The current forecasts.
    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>>;

    /// A kind this source can also fetch for past days (before today; today's
    /// is the poll's job), one daily report at a time, and how many days back.
    fn backfill(&self) -> Option<(&'static str, u32)> {
        None
    }

    /// The report issued on `day`, or `None` if there never was one.
    fn fetch_day<'a>(
        &'a self,
        _ctx: &'a FetchCtx,
        _day: NaiveDate,
    ) -> BoxFuture<'a, Result<Option<Batch>, FetchError>> {
        Box::pin(async { Ok(None) })
    }
}

/// `[forecasts]` in config.toml.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssuedConfig {
    /// Keep MISO's and the NWS's forecasts as they are issued, for FCST's
    /// models: a few requests an hour while the terminal runs, and MISO's
    /// past daily load forecasts filled in a few seconds apart.
    pub keep_issued: bool,
    /// Days of issued forecasts kept (two years: a few tens of megabytes).
    pub keep_days: u32,
}

impl Default for IssuedConfig {
    fn default() -> Self {
        Self {
            keep_issued: true,
            keep_days: 730,
        }
    }
}

/// How one source is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceStatus {
    pub name: &'static str,
    /// When it last answered (market time).
    pub last_ok: Option<NaiveDateTime>,
    pub last_error: Option<String>,
    /// Versions stored from its last answer (0 when nothing changed).
    pub stored: usize,
}

/// What SET and LOG show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollectorStatus {
    pub keeping: bool,
    /// False in an offline replay, where nothing is collected.
    pub live: bool,
    pub sources: Vec<SourceStatus>,
    /// Past daily reports still to fetch.
    pub backfill_left: usize,
    pub files: usize,
    pub bytes: u64,
}

/// Timing, shortened in tests.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Between two past reports.
    pub pace: Duration,
    /// After a failure, before trying that source again.
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            pace: Duration::from_secs(2),
            retry: Duration::from_secs(10 * 60),
        }
    }
}

/// Collects forecasts in the background; see the module notes.
#[derive(Clone)]
pub struct Collector {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: FetchCtx,
    timing: Timing,
    wake: Notify,
    state: Mutex<State>,
    notify: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Market time now; a test can set its own clock.
    now: fn() -> NaiveDateTime,
}

#[derive(Default)]
struct State {
    config: Option<IssuedConfig>,
    sources: Vec<Arc<dyn Source>>,
    /// Bumped by `configure`, so a running pass notices.
    version: u64,
    status: CollectorStatus,
}

/// Market time (EST, no daylight saving) now.
fn market_now() -> NaiveDateTime {
    (chrono::Utc::now() - chrono::TimeDelta::hours(5)).naive_utc()
}

impl Collector {
    pub fn new(ctx: FetchCtx, runtime: &tokio::runtime::Handle) -> Self {
        Self::with_timing(ctx, runtime, Timing::default(), market_now)
    }

    pub fn with_timing(
        ctx: FetchCtx,
        runtime: &tokio::runtime::Handle,
        timing: Timing,
        now: fn() -> NaiveDateTime,
    ) -> Self {
        let inner = Arc::new(Inner {
            ctx,
            timing,
            wake: Notify::new(),
            state: Mutex::default(),
            notify: Mutex::default(),
            now,
        });
        runtime.spawn(inner.clone().work());
        Self { inner }
    }

    /// Called when the status changes (to repaint SET and LOG).
    pub fn set_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.notify.lock() = Some(Box::new(f));
    }

    /// The config and the sources to poll; nothing happens before the first
    /// call.
    pub fn configure(&self, config: &IssuedConfig, sources: Vec<Arc<dyn Source>>) {
        {
            let mut s = self.inner.state.lock();
            s.config = Some(config.clone());
            s.sources = sources;
            s.version += 1;
        }
        self.inner.wake.notify_one();
    }

    pub fn status(&self) -> CollectorStatus {
        self.inner.state.lock().status.clone()
    }
}

impl Inner {
    fn update(&self, f: impl FnOnce(&mut CollectorStatus)) {
        f(&mut self.state.lock().status);
        if let Some(n) = self.notify.lock().as_ref() {
            n();
        }
    }

    async fn work(self: Arc<Self>) {
        // When each source is next due, by name.
        let mut due: HashMap<&'static str, tokio::time::Instant> = HashMap::new();
        let mut latest: HashMap<(&'static str, NaiveDateTime), Vec<Option<f64>>> = HashMap::new();
        let mut seeded: BTreeSet<&'static str> = BTreeSet::new();
        let mut missing: Vec<(Arc<dyn Source>, &'static str, NaiveDate)> = Vec::new();
        let mut planned_for: Option<(u64, NaiveDate)> = None;
        let mut pruned_on: Option<NaiveDate> = None;
        // Measure the disk again only after something was written.
        let mut dirty = true;
        // After a failed report, the backfill waits before the next.
        let mut backfill_after = tokio::time::Instant::now();
        loop {
            let (config, sources, version) = {
                let s = self.state.lock();
                (s.config.clone(), s.sources.clone(), s.version)
            };
            let live = self.ctx.is_live();
            let Some(cache) = self.ctx.cache().cloned() else {
                self.wake.notified().await;
                continue;
            };
            let keeping = config.as_ref().is_some_and(|c| c.keep_issued);
            let usage = if dirty {
                dirty = false;
                Some(
                    tokio::task::spawn_blocking({
                        let cache = cache.clone();
                        move || usage(&cache)
                    })
                    .await
                    .unwrap_or_default(),
                )
            } else {
                None
            };
            self.update(|st| {
                st.keeping = keeping;
                st.live = live;
                if let Some(u) = usage {
                    (st.files, st.bytes) = u;
                }
                st.sources
                    .retain(|s| sources.iter().any(|src| src.name() == s.name));
                for src in &sources {
                    if !st.sources.iter().any(|s| s.name == src.name()) {
                        st.sources.push(SourceStatus {
                            name: src.name(),
                            ..SourceStatus::default()
                        });
                    }
                }
            });
            let Some(config) = config.filter(|c| c.keep_issued && live) else {
                self.wake.notified().await;
                continue;
            };
            let now = (self.now)();
            let today = now.date();

            // Days past the window go, once a day.
            if pruned_on != Some(today) {
                let first = today - chrono::TimeDelta::days(i64::from(config.keep_days.max(1)));
                let cache = cache.clone();
                let (files, bytes) = tokio::task::spawn_blocking(move || prune(&cache, first))
                    .await
                    .unwrap_or_default();
                if files > 0 {
                    dirty = true;
                    self.ctx.events().info(format!(
                        "removed {files} days of issued forecasts before {first} ({} KB)",
                        bytes / 1024
                    ));
                }
                pruned_on = Some(today);
            }

            // Past reports to fetch, newest first, planned again each day and
            // whenever the sources change.
            if planned_for != Some((version, today)) {
                missing.clear();
                for src in &sources {
                    let Some((kind, back)) = src.backfill() else {
                        continue;
                    };
                    let back = back.min(config.keep_days);
                    let have = days(&cache, kind);
                    for k in 1..=i64::from(back) {
                        let day = today - chrono::TimeDelta::days(k);
                        if !have.contains(&day) {
                            missing.push((src.clone(), kind, day));
                        }
                    }
                }
                missing.reverse(); // popped from the end: newest first
                planned_for = Some((version, today));
            }

            // Sources that are due.
            let clock = tokio::time::Instant::now();
            for src in &sources {
                if due.get(src.name()).is_some_and(|d| *d > clock) {
                    continue;
                }
                let result = src.poll(&self.ctx).await;
                let next = match &result {
                    Ok(_) => src.every(),
                    Err(_) => self.timing.retry.min(src.every()),
                };
                due.insert(src.name(), clock + next);
                match result {
                    Ok(batches) => {
                        let mut stored = 0;
                        for b in batches {
                            if seeded.insert(b.kind) {
                                seed(&cache, b.kind, today, &mut latest);
                            }
                            stored += record(&cache, &b, &mut latest).unwrap_or_else(|e| {
                                self.ctx
                                    .events()
                                    .warn(format!("could not keep {} forecasts: {e}", b.kind));
                                0
                            });
                        }
                        dirty |= stored > 0;
                        self.update(|st| {
                            if let Some(s) = st.sources.iter_mut().find(|s| s.name == src.name()) {
                                s.last_ok = Some(now);
                                s.last_error = None;
                                s.stored = stored;
                            }
                        });
                    }
                    Err(e) => self.update(|st| {
                        if let Some(s) = st.sources.iter_mut().find(|s| s.name == src.name()) {
                            s.last_error = Some(e.to_string());
                        }
                    }),
                }
            }

            // One past report per step.
            let ready = tokio::time::Instant::now() >= backfill_after;
            if let Some((src, kind, day)) = missing.pop_if(|_| ready) {
                match src.fetch_day(&self.ctx, day).await {
                    Ok(batch) => {
                        let series = batch.as_ref().map(|b| b.series.clone()).unwrap_or_default();
                        let mut file = Issued {
                            series: series.clone(),
                            versions: Vec::new(),
                        };
                        if let Some(b) = batch {
                            file.versions = b
                                .rows
                                .into_iter()
                                .map(|(target, values)| Version {
                                    issued: b.issued,
                                    target,
                                    values,
                                })
                                .collect();
                        }
                        // An empty file marks a day never published, so it is
                        // not asked for again.
                        match write_day(&cache, kind, day, &file) {
                            Ok(()) => dirty = true,
                            Err(e) => self
                                .ctx
                                .events()
                                .warn(format!("could not keep {kind} {day}: {e}")),
                        }
                    }
                    Err(e) => {
                        self.ctx
                            .events()
                            .warn(format!("{} for {day}: {e}; trying again later", src.name()));
                        missing.push((src, kind, day));
                        backfill_after = tokio::time::Instant::now() + self.timing.retry;
                    }
                }
            }
            let left = missing.len();
            self.update(|st| st.backfill_left = left);

            // Sleep until the next source is due, or a pace if reports wait.
            let until_due = due.values().min().map_or(Duration::from_secs(60), |d| {
                d.saturating_duration_since(tokio::time::Instant::now())
            });
            let nap = if left > 0 {
                let wait = backfill_after
                    .saturating_duration_since(tokio::time::Instant::now())
                    .max(self.timing.pace);
                wait.min(until_due)
            } else {
                until_due
            };
            tokio::select! {
                () = tokio::time::sleep(nap.max(Duration::from_millis(1))) => {}
                () = self.wake.notified() => {}
            }
        }
    }
}

/// The latest stored value of each hour still ahead, from the last week's
/// files, so a restart does not store the same versions again.
fn seed(
    cache: &DiskCache,
    kind: &'static str,
    today: NaiveDate,
    latest: &mut HashMap<(&'static str, NaiveDateTime), Vec<Option<f64>>>,
) {
    let f = read(cache, kind, today - chrono::TimeDelta::days(7), today);
    for v in f.versions {
        let named: Vec<Option<f64>> = v.values;
        latest.insert((kind, v.target), named_values(&f.series, &named));
    }
}

/// Values as `series=value` pairs in a canonical order, so versions written
/// with columns in another order still compare equal.
fn named_values(series: &[String], values: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut pairs: Vec<(&String, Option<f64>)> =
        series.iter().zip(values.iter().copied()).collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    pairs.into_iter().map(|p| p.1).collect()
}

/// Store the rows of a batch that differ from the latest version of their
/// hour, in the file of the day they were issued. Returns how many.
fn record(
    cache: &DiskCache,
    batch: &Batch,
    latest: &mut HashMap<(&'static str, NaiveDateTime), Vec<Option<f64>>>,
) -> io::Result<usize> {
    let mut changed = Vec::new();
    for (target, values) in &batch.rows {
        let named = named_values(&batch.series, values);
        if latest.get(&(batch.kind, *target)) != Some(&named) {
            latest.insert((batch.kind, *target), named);
            changed.push(Version {
                issued: batch.issued,
                target: *target,
                values: values.clone(),
            });
        }
    }
    if changed.is_empty() {
        return Ok(0);
    }
    let day = batch.issued.date();
    let mut file = read_day(cache, batch.kind, day).unwrap_or_default();
    let n = changed.len();
    file.extend(&batch.series, changed);
    write_day(cache, batch.kind, day, &file)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventLog, FetchCtxOptions, Request, Response, Transport};

    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn temp_cache(name: &str) -> DiskCache {
        let dir = std::env::temp_dir().join(format!("mt-issued-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        DiskCache::new(dir)
    }

    #[test]
    fn text_round_trips_and_merges_columns() {
        let f = Issued {
            series: vec!["wind".into(), "solar".into()],
            versions: vec![Version {
                issued: t("2026-10-09 10:00"),
                target: t("2026-10-10 13:00"),
                values: vec![Some(15000.5), None],
            }],
        };
        let text = f.to_text();
        assert_eq!(
            text,
            "series\twind\tsolar\n2026-10-09T10:00\t2026-10-10T13:00\t15000.5\t\n"
        );
        assert_eq!(Issued::parse(&text), Some(f.clone()));
        // A later version names its columns the other way round, plus a new one.
        let mut g = f.clone();
        g.extend(
            &["solar".into(), "wind".into(), "load".into()],
            [Version {
                issued: t("2026-10-09 11:00"),
                target: t("2026-10-10 13:00"),
                values: vec![Some(3.0), Some(14000.0), Some(70000.0)],
            }],
        );
        assert_eq!(g.series, ["wind", "solar", "load"]);
        assert_eq!(g.versions[0].values, [Some(15000.5), None, None]);
        assert_eq!(
            g.versions[1].values,
            [Some(14000.0), Some(3.0), Some(70000.0)]
        );
        assert_eq!(g.column("load"), Some(2));
        assert!(Issued::parse("nonsense").is_none());
    }

    #[test]
    fn only_changed_hours_are_stored_and_old_days_go() {
        let cache = temp_cache("record");
        let mut latest = HashMap::new();
        let batch = |issued: &str, wind: [f64; 3]| Batch {
            kind: "windsolar",
            series: vec!["wind".into()],
            issued: t(issued),
            rows: (0..3)
                .map(|h| {
                    (
                        t("2026-10-10 00:00") + chrono::TimeDelta::hours(h),
                        vec![Some(wind[h as usize])],
                    )
                })
                .collect(),
        };
        assert_eq!(
            record(
                &cache,
                &batch("2026-10-09 10:00", [1.0, 2.0, 3.0]),
                &mut latest
            )
            .unwrap(),
            3
        );
        assert_eq!(
            record(
                &cache,
                &batch("2026-10-09 11:00", [1.0, 2.0, 3.0]),
                &mut latest
            )
            .unwrap(),
            0
        );
        assert_eq!(
            record(
                &cache,
                &batch("2026-10-09 12:00", [1.0, 5.0, 3.0]),
                &mut latest
            )
            .unwrap(),
            1
        );
        // A restart seeds from the files and still stores nothing new.
        let mut fresh = HashMap::new();
        seed(
            &cache,
            "windsolar",
            t("2026-10-09 13:00").date(),
            &mut fresh,
        );
        assert_eq!(
            record(
                &cache,
                &batch("2026-10-09 13:00", [1.0, 5.0, 3.0]),
                &mut fresh
            )
            .unwrap(),
            0
        );
        let day = t("2026-10-09 00:00").date();
        let f = read(&cache, "windsolar", day, day);
        assert_eq!(f.versions.len(), 4);
        assert_eq!(f.versions[3].issued, t("2026-10-09 12:00"));
        assert_eq!(
            days(&cache, "windsolar").into_iter().collect::<Vec<_>>(),
            [day]
        );
        assert_eq!(usage(&cache).0, 1);
        assert_eq!(prune(&cache, day).0, 0);
        assert_eq!(prune(&cache, day + chrono::TimeDelta::days(1)).0, 1);
        assert!(days(&cache, "windsolar").is_empty());
        let _ = fs::remove_dir_all(cache.dir());
    }

    /// A live transport that is never asked: the fake sources answer.
    struct Live;

    impl Transport for Live {
        fn send<'a>(&'a self, _: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
            Box::pin(async { Err(FetchError::Other("not used".into())) })
        }

        fn describe(&self) -> String {
            "live".into()
        }
    }

    struct Fake {
        polls: Mutex<usize>,
        fetched: Mutex<Vec<NaiveDate>>,
    }

    impl Source for Arc<Fake> {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn every(&self) -> Duration {
            Duration::from_millis(30)
        }

        fn poll<'a>(&'a self, _: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
            Box::pin(async move {
                let n = {
                    let mut p = self.polls.lock();
                    *p += 1;
                    *p
                };
                Ok(vec![Batch {
                    kind: "fake",
                    series: vec!["x".into()],
                    issued: now(),
                    rows: vec![(t("2026-10-11 00:00"), vec![Some((n / 3) as f64)])],
                }])
            })
        }

        fn backfill(&self) -> Option<(&'static str, u32)> {
            Some(("daily", 5))
        }

        fn fetch_day<'a>(
            &'a self,
            _: &'a FetchCtx,
            day: NaiveDate,
        ) -> BoxFuture<'a, Result<Option<Batch>, FetchError>> {
            Box::pin(async move {
                self.fetched.lock().push(day);
                // Day 3 back was never published.
                Ok(
                    (day != now().date() - chrono::TimeDelta::days(3)).then(|| Batch {
                        kind: "daily",
                        series: vec!["load".into()],
                        issued: day.and_hms_opt(6, 0, 0).unwrap(),
                        rows: vec![(day.and_hms_opt(0, 0, 0).unwrap(), vec![Some(1.0)])],
                    }),
                )
            })
        }
    }

    fn now() -> NaiveDateTime {
        t("2026-10-10 12:00")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_collector_polls_backfills_newest_first_and_stops_when_told() {
        let cache = temp_cache("collector");
        let ctx = FetchCtx::new(
            Arc::new(Live),
            Some(cache.clone()),
            FetchCtxOptions::default(),
            EventLog::default(),
        );
        let collector = Collector::with_timing(
            ctx,
            &tokio::runtime::Handle::current(),
            Timing {
                pace: Duration::from_millis(5),
                retry: Duration::from_millis(30),
            },
            now,
        );
        let fake = Arc::new(Fake {
            polls: Mutex::new(0),
            fetched: Mutex::default(),
        });
        let config = IssuedConfig::default();
        collector.configure(&config, vec![Arc::new(fake.clone())]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while (fake.fetched.lock().len() < 5 || *fake.polls.lock() < 6)
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let today = now().date();
        let fetched = fake.fetched.lock().clone();
        assert_eq!(
            fetched,
            (1..=5)
                .map(|k| today - chrono::TimeDelta::days(k))
                .collect::<Vec<_>>(),
            "the days before today, newest first, each once"
        );
        // Every day has a file, the unpublished one empty.
        assert_eq!(days(&cache, "daily").len(), 5);
        assert!(
            read_day(&cache, "daily", today - chrono::TimeDelta::days(3))
                .unwrap()
                .versions
                .is_empty()
        );
        // Polled repeatedly, but only changes stored: the value moves every third poll.
        let polls = *fake.polls.lock();
        let stored = read(&cache, "fake", today, today).versions.len();
        assert!(stored < polls && stored >= polls / 3, "{stored} of {polls}");
        let status = collector.status();
        assert!(status.keeping && status.live && status.files >= 6);
        assert_eq!(status.sources[0].name, "fake");
        assert_eq!(status.sources[0].last_ok, Some(now()));
        // Turned off: no more polls.
        collector.configure(
            &IssuedConfig {
                keep_issued: false,
                ..config
            },
            vec![Arc::new(fake.clone())],
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let after = *fake.polls.lock();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(*fake.polls.lock(), after);
        assert!(!collector.status().keeping);
        let _ = fs::remove_dir_all(cache.dir());
    }
}
