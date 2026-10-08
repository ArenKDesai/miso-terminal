//! Charts read the price history through `StoredPrices`: only the nodes
//! asked for, only from the first day on, and a day's file again only once
//! it changes.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use mt_core::{DayLmpReport, DayNodeRow, DayReportKind, Market};
use mt_data::DiskCache;
use mt_miso::{StoredPrices, day_store_key};

fn d(s: &str) -> NaiveDate {
    s.parse().expect("a date")
}

fn row(node: &str, lmp: f32) -> DayNodeRow {
    DayNodeRow {
        node: node.into(),
        node_type: "Hub".into(),
        lmp: [lmp; 24],
        mcc: [1.0; 24],
        mlc: [0.5; 24],
    }
}

fn put(cache: &DiskCache, kind: DayReportKind, day: NaiveDate, rows: Vec<DayNodeRow>) {
    let report = DayLmpReport::new(kind, day, rows);
    cache
        .put(&day_store_key(kind.market(), day), &report.to_bytes())
        .expect("stored");
}

#[test]
fn reads_only_the_nodes_and_days_asked_for_and_changed_files_again() {
    let dir = std::env::temp_dir().join(format!("mt-stored-prices-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = DiskCache::new(&dir);
    let both = || vec![row("MINN.HUB", 20.0), row("ALTE.ALTE", 30.0)];
    put(&cache, DayReportKind::DaExPost, d("2026-09-01"), both());
    put(&cache, DayReportKind::DaExPost, d("2026-09-02"), both());
    put(&cache, DayReportKind::RtPrelim, d("2026-09-02"), both());
    // A day this node did not exist on, and a day before the window.
    put(
        &cache,
        DayReportKind::DaExPost,
        d("2026-09-03"),
        vec![row("ALTE.ALTE", 31.0)],
    );
    put(&cache, DayReportKind::DaExPost, d("2026-08-31"), both());

    let nodes: Arc<[String]> = Arc::from(vec!["MINN.HUB".to_owned()]);
    let first = d("2026-09-01");
    let a = StoredPrices::read(&cache, &nodes, first, None);
    assert_eq!(a.len(), 4, "three DA days and one RT day from the first on");
    assert_eq!(
        a.stored(Market::DayAhead, d("2026-09-03")),
        Some(DayReportKind::DaExPost)
    );
    assert!(
        a.row("MINN.HUB", Market::DayAhead, d("2026-09-03"))
            .is_none(),
        "stored, without this node"
    );
    assert!(a.stored(Market::DayAhead, d("2026-08-31")).is_none());
    assert!(
        a.row("ALTE.ALTE", Market::DayAhead, d("2026-09-01"))
            .is_none(),
        "not asked for"
    );
    let minn = a
        .row("MINN.HUB", Market::DayAhead, d("2026-09-01"))
        .unwrap();
    assert_eq!((minn.lmp[0], minn.mcc[5], minn.mlc[23]), (20.0, 1.0, 0.5));
    assert_eq!(
        a.stored(Market::RealTime, d("2026-09-02")),
        Some(DayReportKind::RtPrelim)
    );

    // An unchanged file is not read again: here one is overwritten with
    // other prices of the same length and given back its time, and the
    // earlier read stands.
    let path = cache.path_for(&day_store_key(Market::DayAhead, d("2026-09-01")));
    let written = std::fs::metadata(&path).unwrap().modified().unwrap();
    let len = std::fs::metadata(&path).unwrap().len();
    put(
        &cache,
        DayReportKind::DaExPost,
        d("2026-09-01"),
        vec![row("MINN.HUB", 21.0), row("ALTE.ALTE", 30.0)],
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(written)
        .unwrap();
    // The final RT report replaces the preliminary one: a new file.
    std::thread::sleep(Duration::from_millis(20));
    put(
        &cache,
        DayReportKind::RtFinal,
        d("2026-09-02"),
        vec![row("MINN.HUB", 25.5)],
    );

    let b = StoredPrices::read(&cache, &nodes, first, Some(&a));
    assert_eq!(
        b.row("MINN.HUB", Market::DayAhead, d("2026-09-01"))
            .unwrap()
            .lmp[0],
        20.0,
        "reused, not read again"
    );
    assert_eq!(
        b.stored(Market::RealTime, d("2026-09-02")),
        Some(DayReportKind::RtFinal)
    );
    assert_eq!(
        b.row("MINN.HUB", Market::RealTime, d("2026-09-02"))
            .unwrap()
            .lmp[0],
        25.5
    );

    // A removed day goes too.
    std::fs::remove_file(cache.path_for(&day_store_key(Market::DayAhead, d("2026-09-02"))))
        .unwrap();
    let c = StoredPrices::read(&cache, &nodes, first, Some(&b));
    assert!(c.stored(Market::DayAhead, d("2026-09-02")).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
