//! The calendar prices follow: NERC holidays (when MISO's load looks like a
//! Sunday's) and the kind of day.

use chrono::{Datelike, NaiveDate, Weekday};

/// The six NERC holidays, on the day they are observed: New Year's Day,
/// Memorial Day, Independence Day, Labor Day, Thanksgiving and Christmas. A
/// fixed-date holiday on a Sunday is observed on the Monday; on a Saturday it
/// is not moved.
pub fn is_holiday(date: NaiveDate) -> bool {
    let fixed = |month: u32, day: u32| {
        NaiveDate::from_ymd_opt(date.year(), month, day).is_some_and(|d| {
            d == date || (d.weekday() == Weekday::Sun && d.succ_opt() == Some(date))
        })
    };
    let nth = |month: u32, weekday: Weekday, n: u8| {
        NaiveDate::from_weekday_of_month_opt(date.year(), month, weekday, n) == Some(date)
    };
    let last_monday_of_may = || {
        (25..=31).any(|d| {
            NaiveDate::from_ymd_opt(date.year(), 5, d)
                .is_some_and(|m| m.weekday() == Weekday::Mon && m == date)
        })
    };
    fixed(1, 1)
        || last_monday_of_may()
        || fixed(7, 4)
        || nth(9, Weekday::Mon, 1)
        || nth(11, Weekday::Thu, 4)
        || fixed(12, 25)
}

/// Weekdays, Saturdays, and Sundays with holidays: load, and so prices,
/// follow these three shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DayKind {
    Weekday,
    Saturday,
    SundayOrHoliday,
}

impl DayKind {
    pub fn of(date: NaiveDate) -> Self {
        if is_holiday(date) || date.weekday() == Weekday::Sun {
            Self::SundayOrHoliday
        } else if date.weekday() == Weekday::Sat {
            Self::Saturday
        } else {
            Self::Weekday
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn nerc_holidays() {
        for d in [
            "2026-01-01", // Thursday
            "2026-05-25", // last Monday of May
            "2026-07-03", // no: see below
            "2026-09-07", // first Monday of September
            "2026-11-26", // fourth Thursday of November
            "2026-12-25",
            "2022-12-26", // Christmas on a Sunday: the Monday
            "2027-05-31",
        ] {
            let expected = d != "2026-07-03";
            assert_eq!(is_holiday(day(d)), expected, "{d}");
        }
        // Independence Day 2026 is a Saturday: not moved.
        assert!(is_holiday(day("2026-07-04")));
        assert!(!is_holiday(day("2026-05-18")) && !is_holiday(day("2026-11-19")));
        assert!(!is_holiday(day("2022-12-27")));
    }

    #[test]
    fn kinds_of_day() {
        assert_eq!(DayKind::of(day("2026-10-09")), DayKind::Weekday);
        assert_eq!(DayKind::of(day("2026-10-10")), DayKind::Saturday);
        assert_eq!(DayKind::of(day("2026-10-11")), DayKind::SundayOrHoliday);
        assert_eq!(DayKind::of(day("2026-11-26")), DayKind::SundayOrHoliday);
    }
}
