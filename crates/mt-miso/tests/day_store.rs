//! The day store: daily reports the terminal downloads are kept whole and
//! served from disk afterwards, only for the day the file itself names, and
//! never from a replay.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDate;
use mt_core::{DayReportKind, Market};
use mt_data::{
    BoxFuture, DiskCache, EventLog, FetchCtx, FetchCtxOptions, FetchError, FixtureTransport, Query,
    Request, Response, Transport,
};
use mt_miso::{Miso, MisoEndpoints, day_store_dir, day_store_key, read_day_store};

/// The fixtures posing as MISO itself, so what they return is kept.
struct Live(FixtureTransport);

impl Transport for Live {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        self.0.send(req)
    }

    fn describe(&self) -> String {
        "fixtures posing as MISO".into()
    }
}

fn ctx(fixtures: &Path, cache: &DiskCache, live: bool) -> FetchCtx {
    let replay = FixtureTransport::new(fixtures);
    let transport: Arc<dyn Transport> = match live {
        true => Arc::new(Live(replay)),
        false => Arc::new(replay),
    };
    FetchCtx::new(
        transport,
        Some(cache.clone()),
        FetchCtxOptions::default(),
        EventLog::default(),
    )
}

fn date(s: &str) -> NaiveDate {
    s.parse().expect("a date")
}

#[tokio::test]
async fn reports_are_kept_and_served_from_the_store() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = std::env::temp_dir().join(format!("mt-day-store-{}", std::process::id()));
    let cache = DiskCache::new(&dir);
    let live = ctx(&fixtures, &cache, true);
    let miso = Miso::new(MisoEndpoints::default());
    let da = date("2026-10-01");

    let fetched = miso
        .day_report(DayReportKind::DaExPost, da)
        .fetch(live.clone(), None)
        .await
        .unwrap()
        .expect("the fixture");
    let kept = read_day_store(&live, Market::DayAhead, da)
        .await
        .expect("kept");
    assert_eq!(kept.kind, DayReportKind::DaExPost);
    assert_eq!(kept.rows.len(), fetched.rows.len());
    assert!(
        cache
            .path_for(&day_store_key(Market::DayAhead, da))
            .starts_with(day_store_dir(&cache))
    );

    // With nothing left to download, the stored day still answers.
    let empty = dir.join("no-fixtures");
    let offline = ctx(&empty, &cache, false);
    let again = miso
        .day_report(DayReportKind::DaExPost, da)
        .fetch(offline.clone(), None)
        .await
        .unwrap()
        .expect("from the store");
    assert_eq!(again.node("MINN.HUB"), fetched.node("MINN.HUB"));

    // A live file for another date is an error; a replay stands it in for
    // any date, and keeps nothing, not even the day its file names.
    let other = date("2026-09-15");
    let err = miso
        .day_report(DayReportKind::DaExPost, other)
        .fetch(live.clone(), None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("the file is for"), "{err}");
    let replay = ctx(&fixtures, &cache, false);
    for day in [other, date("2026-10-02")] {
        let shown = miso
            .day_report(DayReportKind::DaExPost, day)
            .fetch(replay.clone(), None)
            .await
            .unwrap();
        assert!(shown.is_some(), "{day}: the replay answers");
        assert!(
            read_day_store(&live, Market::DayAhead, day).await.is_none(),
            "{day}: not kept"
        );
    }

    // RT: a final report is kept; a preliminary one never replaces it.
    let rt = date("2026-09-26");
    miso.rt_best_day(rt)
        .fetch(live.clone(), None)
        .await
        .unwrap()
        .expect("final");
    let kept = read_day_store(&live, Market::RealTime, rt).await.unwrap();
    assert_eq!(kept.kind, DayReportKind::RtFinal);

    let prelim_day = date("2026-10-01");
    miso.day_report(DayReportKind::RtPrelim, prelim_day)
        .fetch(live.clone(), None)
        .await
        .unwrap()
        .expect("prelim");
    let kept = read_day_store(&live, Market::RealTime, prelim_day)
        .await
        .unwrap();
    assert_eq!(kept.kind, DayReportKind::RtPrelim, "kept until the final");

    let final_report = mt_core::DayLmpReport::new(DayReportKind::RtFinal, prelim_day, kept.rows);
    cache
        .put(
            &day_store_key(Market::RealTime, prelim_day),
            &final_report.to_bytes(),
        )
        .unwrap();
    miso.day_report(DayReportKind::RtPrelim, prelim_day)
        .fetch(live.clone(), None)
        .await
        .unwrap();
    let kept = read_day_store(&live, Market::RealTime, prelim_day)
        .await
        .unwrap();
    assert_eq!(kept.kind, DayReportKind::RtFinal, "not downgraded");

    // The final report is served without asking for the preliminary one.
    let best = miso
        .rt_best_day(prelim_day)
        .fetch(offline, None)
        .await
        .unwrap()
        .expect("from the store");
    assert_eq!(best.kind, DayReportKind::RtFinal);

    let _ = std::fs::remove_dir_all(&dir);
}
