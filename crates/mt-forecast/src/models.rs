//! Statistical models of a day's 24 hourly prices: the bars every other model
//! must clear. Each sees only what was known when the forecast was made (the
//! backtest hands it a series cut off there).

use augurs_core::prelude::*;
use augurs_ets::AutoETS;
use augurs_mstl::MSTLModel;
use chrono::{Duration, NaiveDate};

use crate::calendar::DayKind;
use crate::hourly::{self, Day, Hourly};

/// A model that forecasts the 24 hours of a day from the series as known.
pub trait DayModel: Send + Sync {
    /// The name the Skill tab shows.
    fn name(&self) -> &'static str;

    /// The 24 hours of `day`, from `known` (nothing at or after the moment
    /// the forecast is made). `None` when there is not enough to go on.
    fn forecast(&self, known: &Hourly, day: NaiveDate) -> Option<Day>;
}

/// Each hour as it was on the last full day known before `day`: for DA
/// tomorrow, today; for RT tomorrow, yesterday.
pub struct LastDay;

impl DayModel for LastDay {
    fn name(&self) -> &'static str {
        "Same hour, last day"
    }

    fn forecast(&self, known: &Hourly, day: NaiveDate) -> Option<Day> {
        (1..=7).find_map(|k| hourly::full_day(known, day - Duration::days(k)))
    }
}

/// Each hour as it was a week earlier, the same weekday.
pub struct LastWeek;

impl DayModel for LastWeek {
    fn name(&self) -> &'static str {
        "Same hour, last week"
    }

    fn forecast(&self, known: &Hourly, day: NaiveDate) -> Option<Day> {
        hourly::full_day(known, day - Duration::days(7))
    }
}

/// Each hour's median over the last four known days of the same kind
/// (weekday, Saturday, Sunday or holiday), looking back up to eight weeks.
pub struct Profile;

/// Days of the same kind the profile takes.
const PROFILE_DAYS: usize = 4;

impl DayModel for Profile {
    fn name(&self) -> &'static str {
        "Hour-by-day profile"
    }

    fn forecast(&self, known: &Hourly, day: NaiveDate) -> Option<Day> {
        let kind = DayKind::of(day);
        let days: Vec<Day> = (1..=56)
            .map(|k| day - Duration::days(k))
            .filter(|d| DayKind::of(*d) == kind)
            .filter_map(|d| hourly::full_day(known, d))
            .take(PROFILE_DAYS)
            .collect();
        if days.len() < 2 {
            return None;
        }
        let mut out = [0.0; 24];
        for (h, v) in out.iter_mut().enumerate() {
            let values: Vec<f64> = days.iter().map(|d| d[h]).collect();
            *v = hourly::quantile(&values, 0.5)?;
        }
        Some(out)
    }
}

/// MSTL: the series less its daily and weekly seasonal shapes, smoothed by
/// automatic exponential smoothing (augurs), then the shapes added back. Fit
/// to the last eight weeks known, with the top and bottom 1% clamped.
pub struct Smoothing;

/// Hours of history the smoothing model fits.
const SMOOTHING_HOURS: usize = 8 * 168;

impl DayModel for Smoothing {
    fn name(&self) -> &'static str {
        "Exponential smoothing (MSTL)"
    }

    fn forecast(&self, known: &Hourly, day: NaiveDate) -> Option<Day> {
        let last = *known.keys().next_back()?;
        let until = last + Duration::hours(1);
        let target_end = hourly::hour_start(day, 24);
        let horizon = (target_end - until).num_hours();
        if !(24..=24 * 7).contains(&horizon) {
            return None;
        }
        let mut y = hourly::contiguous(known, until, SMOOTHING_HOURS)?;
        hourly::winsorize(&mut y, 0.01);
        let trend = AutoETS::non_seasonal().into_trend_model();
        let fitted = MSTLModel::new(vec![24, 168], trend).fit(&y).ok()?;
        let points = fitted.predict(horizon as usize, None).ok()?.point;
        let tail = points.get(points.len().checked_sub(24)?..)?;
        let mut out = [0.0; 24];
        out.copy_from_slice(tail);
        out.iter().all(|v| v.is_finite()).then_some(out)
    }
}

/// The statistical models, in the order the Skill tab lists them.
pub fn statistical() -> Vec<Box<dyn DayModel>> {
    vec![
        Box::new(LastDay),
        Box::new(LastWeek),
        Box::new(Profile),
        Box::new(Smoothing),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hourly::hour_start;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    /// Hours numbered from the start of `from`, valued by `f(day index, hour)`.
    fn series(from: &str, days: i64, f: impl Fn(i64, usize) -> f64) -> Hourly {
        let start = day(from);
        let mut s = Hourly::new();
        for d in 0..days {
            for h in 0..24 {
                s.insert(hour_start(start + Duration::days(d), h), f(d, h));
            }
        }
        s
    }

    #[test]
    fn naive_models_repeat_a_known_day() {
        // Day d, hour h is worth 100·d + h. Thursday Oct 1 to Wednesday Oct 7.
        let s = series("2026-10-01", 7, |d, h| (100 * d) as f64 + h as f64);
        let target = day("2026-10-08");
        assert_eq!(LastDay.forecast(&s, target).unwrap()[5], 605.0);
        // A missing hour makes the last full day the one before.
        let mut gap = s.clone();
        gap.remove(&hour_start(day("2026-10-07"), 3));
        assert_eq!(LastDay.forecast(&gap, target).unwrap()[5], 505.0);
        // Last week's Thursday is Oct 1, day 0.
        assert_eq!(LastWeek.forecast(&s, target).unwrap()[23], 23.0);
        assert_eq!(
            LastWeek.forecast(&s, day("2026-10-15")),
            None,
            "Oct 8 is not known"
        );
    }

    #[test]
    fn profile_takes_the_median_of_the_same_kind_of_day() {
        // Four weeks from Thursday Sep 3; weekdays are worth their day index,
        // weekends 1000. Labor Day (Sep 7, a holiday) counts as a Sunday.
        let s = series("2026-09-03", 28, |d, _| {
            let date = day("2026-09-03") + Duration::days(d);
            if DayKind::of(date) == DayKind::Weekday {
                d as f64
            } else {
                1000.0
            }
        });
        // Thursday Oct 1: the last four weekdays are Sep 30, 29, 28, 25 (days
        // 27, 26, 25, 22): median 25.5.
        assert_eq!(Profile.forecast(&s, day("2026-10-01")).unwrap()[0], 25.5);
        // A Sunday takes Sundays and Labor Day.
        assert_eq!(Profile.forecast(&s, day("2026-10-04")).unwrap()[7], 1000.0);
        assert_eq!(Profile.forecast(&Hourly::new(), day("2026-10-04")), None);
    }

    #[test]
    fn smoothing_follows_daily_and_weekly_shapes() {
        // Ten weeks of a daily sine with a weekly swing and no noise.
        let s = series("2026-07-01", 70, |d, h| {
            let t = (d * 24 + h as i64) as f64;
            40.0 + 10.0 * (t / 24.0 * std::f64::consts::TAU).sin()
                + 5.0 * (t / 168.0 * std::f64::consts::TAU).cos()
        });
        let target = day("2026-09-09");
        let got = Smoothing.forecast(&s, target).unwrap();
        let truth = series("2026-07-01", 71, |d, h| {
            let t = (d * 24 + h as i64) as f64;
            40.0 + 10.0 * (t / 24.0 * std::f64::consts::TAU).sin()
                + 5.0 * (t / 168.0 * std::f64::consts::TAU).cos()
        });
        let want = hourly::full_day(&truth, target).unwrap();
        let worst = got
            .iter()
            .zip(want)
            .map(|(g, w)| (g - w).abs())
            .fold(0.0, f64::max);
        assert!(worst < 1.0, "off by {worst}: {got:?}");
        // Too short a history, or a target too far ahead, gives nothing.
        let short = series("2026-08-26", 14, |_, h| h as f64);
        assert_eq!(Smoothing.forecast(&short, day("2026-09-09")), None);
        assert_eq!(Smoothing.forecast(&s, day("2026-09-30")), None);
    }
}
