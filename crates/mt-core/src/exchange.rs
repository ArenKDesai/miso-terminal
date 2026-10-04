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

/// Where the market is and what happens next, for status lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarketStatus {
    pub session: Session,
    /// The next change and what it is: `opens`, `closes`, `extended hours end`.
    pub next: Option<(DateTime<Utc>, &'static str)>,
}

/// A date's hours from the exchange calendar. A date the calendar covers but
/// does not list is a holiday; outside its range, standard weekday hours apply.
pub fn trading_day(date: NaiveDate, calendar: &[TradingDay]) -> Option<TradingDay> {
    if let Some(d) = calendar.iter().find(|d| d.date == date) {
        return Some(*d);
    }
    let covered = calendar
        .iter()
        .map(|d| d.date)
        .min()
        .is_some_and(|lo| lo <= date)
        && calendar
            .iter()
            .map(|d| d.date)
            .max()
            .is_some_and(|hi| date <= hi);
    if covered {
        None
    } else {
        TradingDay::standard(date)
    }
}

/// The market's status at `now`, from the exchange calendar's days (holidays,
/// early closes); days it does not cover get standard weekday hours.
pub fn market_status(now: DateTime<Utc>, calendar: &[TradingDay]) -> MarketStatus {
    let local = to_exchange(now).naive_local();
    let today = trading_day(local.date(), calendar);
    let session = today.map_or(Session::Closed, |d| d.session_at(local.time()));
    let at = |d: &TradingDay, t: NaiveTime| exchange_to_utc(d.date.and_time(t));
    let next = match (session, today) {
        (Session::PreMarket, Some(d)) => at(&d, d.open).map(|t| (t, "opens")),
        (Session::Regular, Some(d)) => at(&d, d.close).map(|t| (t, "closes")),
        (Session::AfterHours, Some(d)) => {
            at(&d, d.session_close).map(|t| (t, "extended hours end"))
        }
        _ => (0..14)
            .filter_map(|i| trading_day(local.date() + chrono::Duration::days(i), calendar))
            .filter_map(|d| at(&d, d.open))
            .find(|open| *open > now)
            .map(|t| (t, "opens")),
    };
    MarketStatus { session, next }
}

impl MarketStatus {
    /// Let the broker's clock overrule the calendar about the regular session
    /// (an unscheduled closure), if it answered in the last ten minutes.
    pub fn with_clock(self, clock: &crate::equity::MarketClock, now: DateTime<Utc>) -> Self {
        if (now - clock.at).num_minutes().abs() > 10 {
            return self;
        }
        match (clock.is_open, self.session) {
            (true, s) if s != Session::Regular => Self {
                session: Session::Regular,
                next: Some((clock.next_close, "closes")),
            },
            (false, Session::Regular) => Self {
                session: Session::Closed,
                next: Some((clock.next_open, "opens")),
            },
            _ => self,
        }
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

    #[test]
    fn status_says_what_happens_next() {
        // Friday 2026-10-02, EDT; the calendar lists the 2nd and the 5th to
        // the 9th, but not Thanksgiving-style gaps.
        let cal: Vec<TradingDay> = ["2026-10-02", "2026-10-05", "2026-10-06"]
            .iter()
            .filter_map(|d| TradingDay::standard(day(d)))
            .collect();
        let at = |s: &str| market_status(utc(s), &cal);
        let s = at("2026-10-02 12:00"); // 08:00 ET
        assert_eq!(s.session, Session::PreMarket);
        assert_eq!(s.next, Some((utc("2026-10-02 13:30"), "opens")));
        let s = at("2026-10-02 15:00");
        assert_eq!(s.next, Some((utc("2026-10-02 20:00"), "closes")));
        let s = at("2026-10-02 21:55"); // 17:55 ET, after hours
        assert_eq!(s.session, Session::AfterHours);
        assert_eq!(s.next.unwrap().1, "extended hours end");
        // The weekend: next open is Monday 09:30 ET.
        let s = at("2026-10-03 15:00");
        assert_eq!(s.session, Session::Closed);
        assert_eq!(s.next, Some((utc("2026-10-05 13:30"), "opens")));
        // A weekday the calendar covers but leaves out is a holiday.
        let holiday: Vec<TradingDay> = cal
            .iter()
            .filter(|d| d.date != day("2026-10-05"))
            .copied()
            .collect();
        let s = market_status(utc("2026-10-05 15:00"), &holiday);
        assert_eq!(s.session, Session::Closed);
        assert_eq!(s.next, Some((utc("2026-10-06 13:30"), "opens")));
        // Without a calendar, standard hours.
        assert_eq!(
            market_status(utc("2026-10-07 15:00"), &[]).session,
            Session::Regular
        );
        // A recent clock overrules the calendar; a stale one does not.
        let clock = crate::equity::MarketClock {
            at: utc("2026-10-02 15:00"),
            is_open: false,
            next_open: utc("2026-10-05 13:30"),
            next_close: utc("2026-10-05 20:00"),
        };
        let s = at("2026-10-02 15:00").with_clock(&clock, utc("2026-10-02 15:01"));
        assert_eq!(s.session, Session::Closed);
        let s = at("2026-10-02 15:00").with_clock(&clock, utc("2026-10-02 16:00"));
        assert_eq!(s.session, Session::Regular);
    }
}
