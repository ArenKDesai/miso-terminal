//! Market time.
//!
//! MISO runs its markets on Eastern Standard Time all year: every timestamp it
//! publishes is UTC-5 with no daylight-saving shift, and every market day has
//! exactly 24 hour-ending intervals. All `NaiveDateTime`s in this workspace are in
//! market time unless a name says otherwise.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Utc};

/// UTC offset of MISO market time, in seconds.
pub const MARKET_UTC_OFFSET_SECS: i32 = -5 * 3600;

/// Label shown next to market-time clocks.
pub const MARKET_TZ_LABEL: &str = "EST";

pub fn market_offset() -> FixedOffset {
    FixedOffset::east_opt(MARKET_UTC_OFFSET_SECS).expect("constant offset is valid")
}

/// Seconds since the epoch the clock is frozen at; `i64::MIN` while it runs.
static FROZEN_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MIN);

/// Stop the clock at `at`, or let it run again with `None`. Process-wide: for
/// showing recorded data as of when it was recorded (the `render` example) and
/// for reproducible output, never in the live app.
pub fn freeze_clock(at: Option<DateTime<Utc>>) {
    let secs = at.map_or(i64::MIN, |t| t.timestamp());
    FROZEN_AT.store(secs, std::sync::atomic::Ordering::Relaxed);
}

/// The current instant: the system clock unless frozen. Everything that shows
/// or reasons about "now" should come through here.
pub fn now_utc() -> DateTime<Utc> {
    match FROZEN_AT.load(std::sync::atomic::Ordering::Relaxed) {
        i64::MIN => Utc::now(),
        secs => DateTime::from_timestamp(secs, 0).unwrap_or_else(Utc::now),
    }
}

/// Current wall-clock time in market time.
pub fn now_market() -> NaiveDateTime {
    to_market(now_utc())
}

/// Today's market day.
pub fn market_today() -> NaiveDate {
    now_market().date()
}

pub fn to_market(t: DateTime<Utc>) -> NaiveDateTime {
    t.with_timezone(&market_offset()).naive_local()
}

pub fn market_to_utc(t: NaiveDateTime) -> DateTime<Utc> {
    (t - Duration::seconds(i64::from(MARKET_UTC_OFFSET_SECS))).and_utc()
}

/// Start of hour-ending `he` (1..=24) on `day`: HE 1 covers 00:00-01:00.
pub fn hour_ending_start(day: NaiveDate, he: u8) -> NaiveDateTime {
    day.and_time(NaiveTime::MIN) + Duration::hours(i64::from(he.saturating_sub(1)))
}

/// Seconds since the Unix epoch for a market-time instant; the x axis of every chart.
pub fn chart_x(t: NaiveDateTime) -> f64 {
    market_to_utc(t).timestamp() as f64
}

/// Inverse of [`chart_x`].
pub fn from_chart_x(x: f64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp(x.round() as i64, 0).map(to_market)
}

#[derive(Debug, thiserror::Error)]
#[error("unrecognised MISO timestamp: {0:?}")]
pub struct TimeParseError(pub String);

/// Formats MISO uses across its APIs, tried in order.
const DATETIME_FORMATS: &[&str] = &[
    "%Y-%m-%dT%H:%M:%S",    // 2026-10-02T16:30:00
    "%Y-%m-%dT%H:%M",       // 2026-10-02T16:30
    "%Y-%m-%d %H:%M:%S",    // 2026-10-02 16:28:00
    "%Y-%m-%d %I:%M:%S %p", // 2026-10-02 4:30:00 PM
    "%m/%d/%Y %I:%M:%S %p", // 10/02/2026 4:30:00 PM
    "%m/%d/%Y %H:%M:%S",    // 10/02/2026 16:30:00
    "%m/%d/%Y %H:%M",       // 10/02/2026 16:30
];

/// Parse any of the datetime spellings MISO publishes. A trailing ` EST` is ignored.
pub fn parse_market_datetime(s: &str) -> Result<NaiveDateTime, TimeParseError> {
    let s = s.trim();
    let s = s.strip_suffix(MARKET_TZ_LABEL).unwrap_or(s).trim();
    DATETIME_FORMATS
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
        .ok_or_else(|| TimeParseError(s.to_owned()))
}

/// Parse a market day: `2026-10-02`, `10-02-2026` or `10/02/2026`.
pub fn parse_market_day(s: &str) -> Result<NaiveDate, TimeParseError> {
    let s = s.trim();
    ["%Y-%m-%d", "%m-%d-%Y", "%m/%d/%Y"]
        .iter()
        .find_map(|f| NaiveDate::parse_from_str(s, f).ok())
        .ok_or_else(|| TimeParseError(s.to_owned()))
}

/// Parse MISO's `RefId` stamp, e.g. `02-Oct-2026 - Interval 16:30 EST`.
pub fn parse_ref_id(s: &str) -> Option<NaiveDateTime> {
    let (day, rest) = s.split_once(" - ")?;
    let day = NaiveDate::parse_from_str(day.trim(), "%d-%b-%Y").ok()?;
    let time = rest
        .trim()
        .strip_prefix("Interval")?
        .trim()
        .trim_end_matches(MARKET_TZ_LABEL)
        .trim();
    let time = NaiveTime::parse_from_str(time, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(time, "%H:%M"))
        .ok()?;
    Some(day.and_time(time))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn parses_every_miso_spelling() {
        let want = dt("2026-10-02 16:30:00");
        for s in [
            "2026-10-02T16:30:00",
            "2026-10-02 16:30:00",
            "2026-10-02 4:30:00 PM",
            "2026-10-02 04:30:00 PM",
            "10/02/2026 4:30:00 PM EST",
            "  2026-10-02T16:30  ",
        ] {
            assert_eq!(parse_market_datetime(s).unwrap(), want, "{s}");
        }
        assert_eq!(
            parse_market_datetime("10/02/2026 12:00:00 AM EST").unwrap(),
            dt("2026-10-02 00:00:00")
        );
        assert!(parse_market_datetime("yesterday-ish").is_err());
    }

    #[test]
    fn parses_ref_ids() {
        assert_eq!(
            parse_ref_id("02-Oct-2026 - Interval 16:30 EST"),
            Some(dt("2026-10-02 16:30:00"))
        );
        assert_eq!(
            parse_ref_id("02-Oct-2026 - Interval 16:33:00 EST"),
            Some(dt("2026-10-02 16:33:00"))
        );
        assert_eq!(
            parse_ref_id("02-Oct-2026 - Total Outage Megawatts: 54,179"),
            None
        );
    }

    #[test]
    fn market_days() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        assert_eq!(parse_market_day("10-02-2026").unwrap(), d);
        assert_eq!(parse_market_day("2026-10-02").unwrap(), d);
        assert_eq!(parse_market_day("10/02/2026").unwrap(), d);
    }

    #[test]
    fn market_time_is_fixed_utc_minus_five() {
        // July (US daylight time) and January must both be UTC-5.
        for s in ["2026-07-01 12:00:00", "2026-01-15 12:00:00"] {
            let t = dt(s);
            assert_eq!(market_to_utc(t).naive_utc(), t + Duration::hours(5));
            assert_eq!(to_market(market_to_utc(t)), t);
        }
    }

    #[test]
    fn hour_ending_and_chart_axis_round_trip() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        assert_eq!(hour_ending_start(d, 1), dt("2026-10-02 00:00:00"));
        assert_eq!(hour_ending_start(d, 24), dt("2026-10-02 23:00:00"));
        let t = dt("2026-10-02 16:35:00");
        assert_eq!(from_chart_x(chart_x(t)), Some(t));
    }
}
