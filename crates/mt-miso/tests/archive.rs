//! The five-minute archive: fetching MISO's previous-day feed files the day
//! away in the disk cache, and the archive query reads it back.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Duration;
use mt_data::{DiskCache, EventLog, FetchCtx, FetchCtxOptions, FixtureTransport, Query};
use mt_miso::{Miso, MisoEndpoints, intraday_archive_key};

#[tokio::test]
async fn previous_day_fills_the_archive() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let dir = std::env::temp_dir().join(format!("mt-archive-test-{}", std::process::id()));
    let cache = DiskCache::new(&dir);
    let ctx = FetchCtx::new(
        Arc::new(FixtureTransport::new(fixtures)),
        Some(cache.clone()),
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let miso = Miso::new(MisoEndpoints::default());

    let previous = miso
        .rt_previous_day()
        .fetch(ctx.clone(), None)
        .await
        .unwrap();
    let day = previous.market_day.expect("the fixture has a market day");
    assert!(cache.get(&intraday_archive_key(day)).is_some());

    let archived = miso
        .rt_archive_day(day)
        .fetch(ctx.clone(), None)
        .await
        .unwrap()
        .expect("archived");
    assert_eq!(archived.intervals(), previous.intervals());
    assert_eq!(archived.node_names(), previous.node_names());

    let never = miso
        .rt_archive_day(day - Duration::days(30))
        .fetch(ctx, None)
        .await
        .unwrap();
    assert!(never.is_none(), "a day that was never saved");

    // Retention: days before the window go, the rest stay.
    let old = day - Duration::days(120);
    cache
        .put(&intraday_archive_key(old), &previous.to_bytes())
        .unwrap();
    assert_eq!(mt_miso::prune_archive(&cache, 90, day), 1);
    assert!(cache.get(&intraday_archive_key(old)).is_none());
    assert!(cache.get(&intraday_archive_key(day)).is_some());
    assert_eq!(
        mt_miso::prune_archive(&cache, 0, day),
        0,
        "0 keeps everything"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
