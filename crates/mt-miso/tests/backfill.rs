//! The backfill against a fake MISO: it fills the window newest first, one
//! report at a time and a pace apart, never downloads a day twice, carries on
//! where it stopped after a pause, replaces a preliminary RT day with its
//! final report, retries failures, and removes days before the window.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, TimeDelta};
use mt_core::time::market_today;
use mt_core::{DayLmpReport, DayReportKind, Market};
use mt_data::{
    BoxFuture, DiskCache, EventLog, FetchCtx, FetchCtxOptions, FetchError, FixtureTransport,
    Request, Response, Transport,
};
use mt_miso::history::{Inventory, Phase};
use mt_miso::{Backfill, BackfillStatus, HistoryConfig, MisoEndpoints, day_store_key};
use parking_lot::Mutex;

const PACE: Duration = Duration::from_millis(40);

/// MISO's report site: the recorded reports, re-dated to whatever day is
/// asked for, for the days it has "published"; 404 for the rest.
struct FakeMiso {
    da: String,
    rt: String,
    published: Mutex<HashSet<(&'static str, NaiveDate)>>,
    /// Requests to drop (a network error) before answering again.
    fail: Mutex<usize>,
    asked: Mutex<Vec<(Instant, String)>>,
}

impl FakeMiso {
    fn new() -> Arc<Self> {
        let dir = fixtures().join("docs.misoenergy.org/marketreports");
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).expect("a recorded report");
        Arc::new(Self {
            da: read("20261001_da_expost_lmp.csv"),
            rt: read("20260926_rt_lmp_final.csv"),
            published: Mutex::default(),
            fail: Mutex::new(0),
            asked: Mutex::default(),
        })
    }

    fn publish(&self, suffix: &'static str, days: impl IntoIterator<Item = NaiveDate>) {
        self.published
            .lock()
            .extend(days.into_iter().map(|d| (suffix, d)));
    }

    /// The answered downloads, as `(suffix, day)`, in order.
    fn downloaded(&self) -> Vec<(String, NaiveDate)> {
        let published = self.published.lock();
        self.asked
            .lock()
            .iter()
            .filter_map(|(_, url)| report(url))
            .filter(|(s, d)| published.contains(&(s.as_str(), *d)))
            .collect()
    }

    fn requests(&self) -> usize {
        self.asked.lock().len()
    }
}

/// `…/20261001_da_expost_lmp.csv` -> `("da_expost_lmp", 2026-10-01)`.
fn report(url: &str) -> Option<(String, NaiveDate)> {
    let file = url.rsplit('/').next()?.strip_suffix(".csv")?;
    let (day, suffix) = file.split_once('_')?;
    Some((
        suffix.to_owned(),
        NaiveDate::parse_from_str(day, "%Y%m%d").ok()?,
    ))
}

impl Transport for FakeMiso {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        self.asked.lock().push((Instant::now(), req.url.clone()));
        let failed = {
            let mut fail = self.fail.lock();
            let failed = *fail > 0;
            *fail = fail.saturating_sub(1);
            failed
        };
        let answer = match report(&req.url) {
            _ if failed => Err(FetchError::Network("connection reset".into())),
            Some((suffix, day))
                if self
                    .published
                    .lock()
                    .iter()
                    .any(|p| *p == (suffix.as_str(), day)) =>
            {
                let template = if suffix.starts_with("da") {
                    &self.da
                } else {
                    &self.rt
                };
                let mut lines: Vec<&str> = template.lines().collect();
                let date = day.format("%m/%d/%Y").to_string();
                lines[1] = &date;
                Ok(Response {
                    status: 200,
                    headers: Default::default(),
                    body: lines.join("\n").into(),
                })
            }
            _ => Ok(Response {
                status: 404,
                headers: Default::default(),
                body: Default::default(),
            }),
        };
        Box::pin(async move { answer })
    }

    fn describe(&self) -> String {
        "fake MISO".into()
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

struct Rig {
    /// The backfill works on it; kept alive for the test.
    _rt: tokio::runtime::Runtime,
    dir: PathBuf,
    cache: DiskCache,
    miso: Arc<FakeMiso>,
    backfill: Backfill,
}

impl Rig {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mt-backfill-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = DiskCache::new(&dir);
        let miso = FakeMiso::new();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let ctx = FetchCtx::new(
            miso.clone(),
            Some(cache.clone()),
            FetchCtxOptions::default(),
            EventLog::default(),
        );
        let backfill = Backfill::with_timing(ctx, rt.handle(), PACE, Duration::from_millis(20));
        Self {
            _rt: rt,
            dir,
            cache,
            miso,
            backfill,
        }
    }

    fn configure(&self, keep_days: u32, backfill: bool) {
        self.backfill.configure(
            &MisoEndpoints::default(),
            &HistoryConfig {
                keep_days,
                backfill,
            },
            false,
        );
    }

    fn wait_for(&self, what: &str, done: impl Fn(&BackfillStatus) -> bool) -> BackfillStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let s = self.backfill.status();
            if done(&s) {
                return s;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {s:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn put(&self, kind: DayReportKind, day: NaiveDate) {
        let report = DayLmpReport::new(kind, day, Vec::new());
        self.cache
            .put(&day_store_key(kind.market(), day), &report.to_bytes())
            .expect("stored");
    }

    fn inventory(&self) -> Inventory {
        Inventory::scan(&self.cache)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn days_back(today: NaiveDate, from: i64, to: i64) -> impl Iterator<Item = NaiveDate> {
    (from..=to).map(move |n| today - TimeDelta::days(n))
}

fn up_to_date(s: &BackfillStatus) -> bool {
    s.phase == Phase::UpToDate
}

#[test]
fn fills_the_window_newest_first_one_report_a_pace_apart() {
    let rig = Rig::new("fill");
    let today = market_today();
    // Ten days: DA through today; RT final up to six days back, preliminary
    // after that. MISO never published the first day's DA.
    rig.miso.publish("da_expost_lmp", days_back(today, 0, 8));
    rig.miso.publish("rt_lmp_final", days_back(today, 6, 9));
    rig.miso.publish("rt_lmp_prelim", days_back(today, 1, 5));
    // A preliminary day the panels kept earlier, now final.
    rig.put(DayReportKind::RtPrelim, today - TimeDelta::days(7));

    rig.configure(10, true);
    let s = rig.wait_for("the window to fill", up_to_date);

    let inv = rig.inventory();
    assert_eq!(inv.da.len(), 9);
    for (day, stored) in &inv.rt {
        let age = (today - *day).num_days();
        let want = if age >= 6 {
            DayReportKind::RtFinal
        } else {
            DayReportKind::RtPrelim
        };
        assert_eq!(stored.kind, want, "{day}");
    }
    assert_eq!(inv.rt.len(), 9, "yesterday back to the window's first day");
    assert_eq!(s.not_published.len(), 1, "{s:?}");
    assert_eq!(s.not_published[0].market, Market::DayAhead);
    assert_eq!(s.coverage.missing(), 1);
    assert_eq!(s.coverage.rt_prelim, 5);

    // Newest first, and every report downloaded once.
    let downloaded = rig.miso.downloaded();
    assert_eq!(downloaded[0], ("da_expost_lmp".into(), today));
    assert!(
        downloaded.windows(2).all(|w| w[0].1 >= w[1].1),
        "{downloaded:?}"
    );
    let unique: HashSet<_> = downloaded.iter().collect();
    assert_eq!(unique.len(), downloaded.len(), "{downloaded:?}");
    assert_eq!(downloaded.len(), 9 + 9);

    // A pace apart: requests for different days never come closer than that.
    let asked = rig.miso.asked.lock().clone();
    for w in asked.windows(2) {
        let (a, b) = (report(&w[0].1).unwrap(), report(&w[1].1).unwrap());
        let same_job = a.1 == b.1 && a.0.starts_with("rt") && b.0.starts_with("rt");
        if !same_job {
            let gap = w[1].0 - w[0].0;
            assert!(
                gap >= PACE - Duration::from_millis(5),
                "{a:?} then {b:?} after {gap:?}"
            );
        }
    }
}

#[test]
fn a_pause_stops_it_and_it_carries_on_where_it_stopped() {
    let rig = Rig::new("pause");
    let today = market_today();
    rig.miso.publish("da_expost_lmp", days_back(today, 0, 19));
    rig.miso.publish("rt_lmp_final", days_back(today, 6, 19));
    rig.miso.publish("rt_lmp_prelim", days_back(today, 1, 5));

    rig.configure(20, true);
    rig.wait_for("a few downloads", |s| s.done >= 4);
    rig.configure(20, false);
    let s = rig.wait_for("the pause", |s| s.phase == Phase::Off);
    let asked = rig.miso.requests();
    std::thread::sleep(PACE * 4);
    assert_eq!(rig.miso.requests(), asked, "nothing while paused");
    assert!(
        s.coverage.missing() > 0 && s.coverage.missing() < 39,
        "{s:?}"
    );

    rig.configure(20, true);
    let s = rig.wait_for("the rest", up_to_date);
    assert_eq!(s.coverage.missing(), 0, "{s:?}");
    let downloaded = rig.miso.downloaded();
    let unique: HashSet<_> = downloaded.iter().collect();
    assert_eq!(
        unique.len(),
        downloaded.len(),
        "nothing twice: {downloaded:?}"
    );
    assert_eq!(downloaded.len(), 20 + 19);
}

#[test]
fn failures_are_tried_again() {
    let rig = Rig::new("retry");
    let today = market_today();
    rig.miso.publish("da_expost_lmp", days_back(today, 0, 1));
    rig.miso.publish("rt_lmp_prelim", days_back(today, 1, 1));
    *rig.miso.fail.lock() = 2;

    rig.configure(2, true);
    rig.wait_for("a retry", |s| matches!(s.phase, Phase::Retrying { .. }));
    let s = rig.wait_for("the window to fill", up_to_date);
    assert_eq!(s.coverage.missing(), 0, "{s:?}");
    assert!(s.not_published.is_empty() && s.unreadable.is_empty());
}

#[test]
fn days_before_the_window_are_removed_even_when_off() {
    let rig = Rig::new("prune");
    let today = market_today();
    let old = today - TimeDelta::days(30);
    rig.put(DayReportKind::DaExPost, old);
    rig.put(DayReportKind::RtFinal, old);
    rig.put(DayReportKind::DaExPost, today);

    // Nothing happens before the settings are known.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.inventory().da.len(), 2);
    assert_eq!(rig.backfill.status().phase, Phase::Starting);

    rig.configure(10, false);
    let s = rig.wait_for("the pass", |s| s.phase == Phase::Off);
    assert_eq!(s.pruned, 2);
    let inv = rig.inventory();
    assert_eq!(inv.da.keys().copied().collect::<Vec<_>>(), [today]);
    assert!(inv.rt.is_empty());
    assert_eq!(rig.miso.requests(), 0, "off: nothing downloaded");
    assert_eq!(s.coverage.da_stored, 1);
}

#[test]
fn a_replay_has_nothing_to_keep() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let replay = FetchCtx::new(
        Arc::new(FixtureTransport::new(fixtures())),
        None,
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let backfill = Backfill::new(replay, rt.handle());
    backfill.configure(
        &MisoEndpoints::default(),
        &HistoryConfig {
            keep_days: 10,
            backfill: true,
        },
        false,
    );
    assert_eq!(backfill.status().phase, Phase::Unavailable);
}
