//! The freezable clock. Its own test binary: freezing is process-wide, so it
//! must not run alongside tests that read the real time.

use chrono::{NaiveDate, TimeZone, Utc};
use mt_core::time::{freeze_clock, market_today, now_market, now_utc};

#[test]
fn freezing_stops_market_time_and_thawing_restarts_it() {
    let at = Utc.with_ymd_and_hms(2026, 10, 2, 21, 55, 0).unwrap(); // 16:55 EST
    freeze_clock(Some(at));
    assert_eq!(now_utc(), at);
    assert_eq!(
        market_today(),
        NaiveDate::from_ymd_opt(2026, 10, 2).unwrap()
    );
    assert_eq!(now_market().format("%H:%M").to_string(), "16:55");
    freeze_clock(None);
    assert!(now_utc() > at, "the real clock is running again");
}
