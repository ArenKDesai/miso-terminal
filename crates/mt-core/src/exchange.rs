//! Exchange time.
//!
//! US stock and options exchanges keep New York time, which observes daylight
//! saving (EDT, UTC-4, in summer). MISO's market time does not: it is EST,
//! UTC-5, all year (see [`crate::time`]). So 09:30 at the exchange is 08:30
//! MISO time from March to November and 09:30 the rest of the year. Keep the
//! two apart: equities use this module, MISO data uses `time`.

use chrono::{
    DateTime, Datelike, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;

/// The exchanges' time zone.
pub const EXCHANGE_TZ: Tz = chrono_tz::America::New_York;

/// The current instant at the exchange (follows a frozen clock, like
/// [`crate::time::now_utc`]).
pub fn now_exchange() -> DateTime<Tz> {
    to_exchange(crate::time::now_utc())
}

pub fn to_exchange(t: DateTime<Utc>) -> DateTime<Tz> {
    t.with_timezone(&EXCHANGE_TZ)
}

/// A New York wall-clock time as an instant. In the autumn hour that happens
/// twice, the first (daylight) one; in the spring hour that never happens, `None`.
pub fn exchange_to_utc(t: NaiveDateTime) -> Option<DateTime<Utc>> {
    match EXCHANGE_TZ.from_local_datetime(&t) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => Some(t.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

/// `EDT` or `EST`, for clocks.
pub fn tz_label(t: &DateTime<Tz>) -> String {
    t.format("%Z").to_string()
}

/// Which part of the trading day an instant falls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Closed,
    /// Extended hours before the open (orders need `extended_hours`).
    PreMarket,
    Regular,
    /// Extended hours after the close.
    AfterHours,
}

impl Session {
    pub fn label(self) -> &'static str {
        match self {
            Self::Closed => "Closed",
            Self::PreMarket => "Pre-market",
            Self::Regular => "Open",
            Self::AfterHours => "After hours",
        }
    }
}

/// One day's hours, in New York time. The exchange calendar (holidays, early
/// closes) supplies these; [`TradingDay::standard`] is the usual weekday.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradingDay {
    pub date: NaiveDate,
    /// Extended hours start.
    pub session_open: NaiveTime,
    pub open: NaiveTime,
    pub close: NaiveTime,
    /// Extended hours end.
    pub session_close: NaiveTime,
}

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or(NaiveTime::MIN)
}

impl TradingDay {
    /// 04:00 extended, 09:30 open, 16:00 close, 20:00 extended close, on a
    /// weekday. `None` at weekends. Holidays need the calendar.
    pub fn standard(date: NaiveDate) -> Option<Self> {
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            return None;
        }
        Some(Self {
            date,
            session_open: hm(4, 0),
            open: hm(9, 30),
            close: hm(16, 0),
            session_close: hm(20, 0),
        })
    }

    pub fn session_at(&self, t: NaiveTime) -> Session {
        if t < self.session_open || t >= self.session_close {
            Session::Closed
        } else if t < self.open {
            Session::PreMarket
        } else if t < self.close {
            Session::Regular
        } else {
            Session::AfterHours
        }
    }

    /// The regular session as instants.
    pub fn regular_utc(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        Some((
            exchange_to_utc(self.date.and_time(self.open))?,
            exchange_to_utc(self.date.and_time(self.close))?,
        ))
    }
}

/// The session at instant `t`, given the hours of `t`'s exchange date
/// (`None` when the exchange is shut that day).
pub fn session(t: DateTime<Utc>, day: Option<&TradingDay>) -> Session {
    let local = to_exchange(t).naive_local();
    match day {
        Some(d) if d.date == local.date() => d.session_at(local.time()),
        _ => Session::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time;

    fn utc(s: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
            .unwrap()
            .and_utc()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn new_york_observes_daylight_saving_and_miso_does_not() {
        // The 09:30 open is 13:30 UTC in summer and 14:30 UTC in winter...
        let summer = TradingDay::standard(day("2026-07-01")).unwrap();
        let winter = TradingDay::standard(day("2026-01-15")).unwrap();
        assert_eq!(summer.regular_utc().unwrap().0, utc("2026-07-01 13:30"));
        assert_eq!(winter.regular_utc().unwrap().0, utc("2026-01-15 14:30"));
        // ...which is 08:30 and 09:30 in MISO's market time.
        assert_eq!(
            time::to_market(utc("2026-07-01 13:30")).time(),
            NaiveTime::from_hms_opt(8, 30, 0).unwrap()
        );
        assert_eq!(
            time::to_market(utc("2026-01-15 14:30")).time(),
            NaiveTime::from_hms_opt(9, 30, 0).unwrap()
        );
        assert_eq!(tz_label(&to_exchange(utc("2026-07-01 13:30"))), "EDT");
        assert_eq!(tz_label(&to_exchange(utc("2026-01-15 14:30"))), "EST");
    }

    #[test]
    fn clock_changes() {
        // 2026-03-08: 02:00-03:00 does not exist; 2026-11-01: 01:00-02:00 happens twice.
        let gap = day("2026-03-08").and_time(hm(2, 30));
        assert_eq!(exchange_to_utc(gap), None);
        let twice = day("2026-11-01").and_time(hm(1, 30));
        assert_eq!(
            exchange_to_utc(twice),
            Some(utc("2026-11-01 05:30")),
            "the EDT one"
        );
        assert_eq!(
            to_exchange(utc("2026-11-01 06:30")).naive_local(),
            twice,
            "an hour later it is 01:30 again, in EST"
        );
    }

    #[test]
    fn sessions() {
        let d = TradingDay::standard(day("2026-10-02")).unwrap(); // a Friday, EDT
        let at = |hhmm: &str| session(utc(&format!("2026-10-02 {hhmm}")), Some(&d));
        assert_eq!(at("07:59"), Session::Closed); // 03:59 ET
        assert_eq!(at("08:00"), Session::PreMarket); // 04:00 ET
        assert_eq!(at("13:29"), Session::PreMarket);
        assert_eq!(at("13:30"), Session::Regular); // 09:30 ET
        assert_eq!(at("19:59"), Session::Regular);
        assert_eq!(at("20:00"), Session::AfterHours); // 16:00 ET
        assert_eq!(at("23:59"), Session::AfterHours);
        // 00:00 UTC on the 3rd is still the 2nd in New York (20:00, closed).
        assert_eq!(session(utc("2026-10-03 00:00"), Some(&d)), Session::Closed);
        assert_eq!(
            session(utc("2026-10-02 15:00"), None),
            Session::Closed,
            "a holiday"
        );
        assert_eq!(TradingDay::standard(day("2026-10-03")), None, "Saturday");
        // An early close from the calendar.
        let early = TradingDay {
            close: hm(13, 0),
            session_close: hm(17, 0),
            ..TradingDay::standard(day("2026-11-27")).unwrap()
        };
        assert_eq!(early.session_at(hm(13, 30)), Session::AfterHours);
    }
}
