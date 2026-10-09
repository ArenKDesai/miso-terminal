//! How good a model is, and how wide its bands should be.
//!
//! A rolling-origin backtest forecasts each past day as if it were tomorrow,
//! from what was known then, and keeps the forecast beside what happened.
//! Bands are split conformal: the quantiles of past errors (actual less
//! forecast) added to the forecast, so an 80% band holds about 80% of
//! outcomes by construction. To score the bands honestly, each backtest day's
//! band comes only from the errors of the days before it.

use chrono::{NaiveDate, NaiveDateTime};

use crate::hourly::{self, Day, Hourly};
use crate::models::DayModel;

/// The quantiles a forecast carries: the 80% band, the 50% band and the
/// median.
pub const QUANTILES: [f64; 5] = [0.1, 0.25, 0.5, 0.75, 0.9];

/// Past errors needed before bands are drawn: a week of hours.
pub const MIN_ERRORS: usize = 7 * 24;

/// Days of recent errors a band is drawn from: about two months, so it
/// follows the season.
pub const BAND_DAYS: usize = 60;

/// One model's backtest: its forecast and the outcome for each day it could
/// forecast, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub model: &'static str,
    pub days: Vec<Outcome>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub day: NaiveDate,
    pub forecast: Day,
    pub actual: Day,
}

/// Forecast each of `days` (oldest first) with every model, from `series`
/// cut off at `known_until(day)`, and keep the days whose outcome is whole.
pub fn backtest(
    models: &[Box<dyn DayModel>],
    series: &Hourly,
    days: &[NaiveDate],
    known_until: impl Fn(NaiveDate) -> NaiveDateTime,
) -> Vec<Record> {
    let mut records: Vec<Record> = models
        .iter()
        .map(|m| Record {
            model: m.name(),
            days: Vec::new(),
        })
        .collect();
    for &day in days {
        let Some(actual) = hourly::full_day(series, day) else {
            continue;
        };
        let cut = known_until(day);
        let known: Hourly = series.range(..cut).map(|(t, v)| (*t, *v)).collect();
        for (model, record) in models.iter().zip(&mut records) {
            if let Some(forecast) = model.forecast(&known, day) {
                record.days.push(Outcome {
                    day,
                    forecast,
                    actual,
                });
            }
        }
    }
    records
}

/// Errors (actual less forecast), hour by hour, over the given outcomes.
pub fn errors(outcomes: &[Outcome]) -> Vec<f64> {
    outcomes
        .iter()
        .flat_map(|o| o.actual.iter().zip(&o.forecast).map(|(a, f)| a - f))
        .collect()
}

/// Conformal bands: what to add to a forecast for each of [`QUANTILES`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bands {
    pub offsets: [f64; 5],
}

impl Bands {
    /// From past errors; `None` with fewer than [`MIN_ERRORS`].
    pub fn from_errors(errors: &[f64]) -> Option<Self> {
        if errors.len() < MIN_ERRORS {
            return None;
        }
        let mut offsets = [0.0; 5];
        for (o, q) in offsets.iter_mut().zip(QUANTILES) {
            *o = hourly::quantile(errors, q)?;
        }
        Some(Self { offsets })
    }

    /// From a record's last [`BAND_DAYS`] days.
    pub fn from_record(record: &Record) -> Option<Self> {
        let recent = record.days.len().saturating_sub(BAND_DAYS);
        Self::from_errors(&errors(&record.days[recent..]))
    }

    /// The five quantiles around a point forecast.
    pub fn around(&self, point: f64) -> [f64; 5] {
        self.offsets.map(|o| point + o)
    }
}

/// A model's accuracy over a backtest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accuracy {
    /// Days forecast.
    pub days: usize,
    /// Mean absolute error, $/MWh.
    pub mae: f64,
    /// Root mean squared error, $/MWh.
    pub rmse: f64,
    /// Mean pinball loss over [`QUANTILES`], on the days with bands.
    pub pinball: Option<f64>,
    /// Share of outcomes inside the 50% and the 80% bands, on the days with
    /// bands.
    pub coverage50: Option<f64>,
    pub coverage80: Option<f64>,
}

/// Score a record. Bands for each day come from the errors of the
/// [`BAND_DAYS`] days before it, so the coverage is out of sample.
pub fn accuracy(record: &Record) -> Option<Accuracy> {
    let all = errors(&record.days);
    if all.is_empty() {
        return None;
    }
    let n = all.len() as f64;
    let mae = all.iter().map(|e| e.abs()).sum::<f64>() / n;
    let rmse = (all.iter().map(|e| e * e).sum::<f64>() / n).sqrt();
    let (mut pinball, mut in50, mut in80, mut scored) = (0.0, 0, 0, 0usize);
    for (i, day) in record.days.iter().enumerate() {
        let Some(bands) = Bands::from_errors(&errors(&record.days[i.saturating_sub(BAND_DAYS)..i]))
        else {
            continue;
        };
        for (a, f) in day.actual.iter().zip(&day.forecast) {
            let q = bands.around(*f);
            in50 += usize::from((q[1]..=q[3]).contains(a));
            in80 += usize::from((q[0]..=q[4]).contains(a));
            pinball += QUANTILES
                .iter()
                .zip(q)
                .map(|(level, at)| pinball_loss(*a, at, *level))
                .sum::<f64>()
                / QUANTILES.len() as f64;
            scored += 1;
        }
    }
    let share = |k: usize| (scored > 0).then(|| k as f64 / scored as f64);
    Some(Accuracy {
        days: record.days.len(),
        mae,
        rmse,
        pinball: (scored > 0).then(|| pinball / scored as f64),
        coverage50: share(in50),
        coverage80: share(in80),
    })
}

/// The pinball (quantile) loss of predicting `at` for quantile `level` when
/// `actual` happened.
pub fn pinball_loss(actual: f64, at: f64, level: f64) -> f64 {
    if actual >= at {
        level * (actual - at)
    } else {
        (1.0 - level) * (at - actual)
    }
}

/// The record with the lowest mean absolute error over the days every
/// listed record forecast, so models are compared on the same days.
pub fn best(records: &[Record]) -> Option<&Record> {
    let shared: Vec<NaiveDate> = records
        .iter()
        .filter(|r| !r.days.is_empty())
        .map(|r| {
            r.days
                .iter()
                .map(|o| o.day)
                .collect::<std::collections::BTreeSet<_>>()
        })
        .reduce(|a, b| a.intersection(&b).copied().collect())?
        .into_iter()
        .collect();
    let mae = |r: &Record| {
        let on: Vec<Outcome> = r
            .days
            .iter()
            .filter(|o| shared.binary_search(&o.day).is_ok())
            .cloned()
            .collect();
        let e = errors(&on);
        (!e.is_empty()).then(|| e.iter().map(|x| x.abs()).sum::<f64>() / e.len() as f64)
    };
    records
        .iter()
        .filter_map(|r| Some((r, mae(r)?)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hourly::hour_start;
    use crate::models::{LastDay, LastWeek};
    use chrono::Duration;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn outcome(d: &str, forecast: f64, actual: f64) -> Outcome {
        Outcome {
            day: day(d),
            forecast: [forecast; 24],
            actual: [actual; 24],
        }
    }

    #[test]
    fn backtest_sees_only_what_was_known() {
        // Each day is worth its index; the models repeat a known day.
        let start = day("2026-09-01");
        let mut s = Hourly::new();
        for d in 0..30 {
            for h in 0..24 {
                s.insert(hour_start(start + Duration::days(d), h), d as f64);
            }
        }
        let days: Vec<NaiveDate> = (20..30).map(|d| start + Duration::days(d)).collect();
        let models: Vec<Box<dyn DayModel>> = vec![Box::new(LastDay), Box::new(LastWeek)];
        // Known up to the day itself: LastDay is always one behind.
        let r = backtest(&models, &s, &days, |d| hour_start(d, 0));
        assert_eq!(r[0].days.len(), 10);
        assert!(r[0].days.iter().all(|o| o.actual[0] - o.forecast[0] == 1.0));
        assert!(r[1].days.iter().all(|o| o.actual[0] - o.forecast[0] == 7.0));
        // Known only to the day before: two behind.
        let r = backtest(&models, &s, &days, |d| hour_start(d - Duration::days(1), 0));
        assert!(r[0].days.iter().all(|o| o.actual[0] - o.forecast[0] == 2.0));
        // A day with no outcome is skipped.
        let r = backtest(&models, &s, &[day("2026-10-05")], |d| hour_start(d, 0));
        assert!(r[0].days.is_empty());
        assert_eq!(
            best(&backtest(&models, &s, &days, |d| hour_start(d, 0))).map(|r| r.model),
            Some("Same hour, last day")
        );
    }

    #[test]
    fn accuracy_by_hand() {
        // Errors of +2 and −4 on alternate days, no bands yet (two days).
        let r = Record {
            model: "m",
            days: vec![
                outcome("2026-10-01", 10.0, 12.0),
                outcome("2026-10-02", 10.0, 6.0),
            ],
        };
        let a = accuracy(&r).unwrap();
        assert_eq!(a.days, 2);
        assert_eq!(a.mae, 3.0);
        // √((4 + 16) / 2) = √10.
        assert!((a.rmse - 10f64.sqrt()).abs() < 1e-12);
        assert_eq!((a.pinball, a.coverage80), (None, None));
        assert!(
            accuracy(&Record {
                model: "m",
                days: vec![]
            })
            .is_none()
        );
    }

    #[test]
    fn bands_come_from_earlier_errors_only() {
        // Errors cycle 0, +1, +2, ... +9 over ten days, repeated: after a week
        // of errors the bands exist, and roughly 80% land inside them.
        let days: Vec<Outcome> = (0..40)
            .map(|i| {
                let d = day("2026-08-01") + Duration::days(i);
                Outcome {
                    day: d,
                    forecast: [0.0; 24],
                    actual: [(i % 10) as f64; 24],
                }
            })
            .collect();
        let r = Record { model: "m", days };
        let a = accuracy(&r).unwrap();
        let c80 = a.coverage80.unwrap();
        assert!((0.7..=0.9).contains(&c80), "{c80}");
        assert!(a.coverage50.unwrap() < c80);
        let b = Bands::from_record(&r).unwrap();
        // Errors 0..9 evenly: the 10th percentile 0.9, the median 4.5.
        assert!((b.offsets[0] - 0.9).abs() < 1e-9 && (b.offsets[2] - 4.5).abs() < 1e-9);
        assert_eq!(b.around(10.0)[2], 14.5);
        assert!(Bands::from_errors(&[1.0; 10]).is_none());
    }

    #[test]
    fn pinball_by_hand() {
        // Under-forecasting the 90th percentile by 10 costs 9; over, 1.
        assert!((pinball_loss(20.0, 10.0, 0.9) - 9.0).abs() < 1e-12);
        assert!((pinball_loss(0.0, 10.0, 0.9) - 1.0).abs() < 1e-12);
        assert_eq!(pinball_loss(5.0, 5.0, 0.5), 0.0);
    }

    #[test]
    fn best_compares_on_shared_days() {
        // A is perfect on one easy day; B misses by 1 on every day; on the
        // days both forecast, B is better.
        let a = Record {
            model: "A",
            days: vec![
                outcome("2026-10-01", 0.0, 5.0),
                outcome("2026-10-02", 1.0, 1.0),
            ],
        };
        let b = Record {
            model: "B",
            days: vec![
                outcome("2026-10-01", 4.0, 5.0),
                outcome("2026-10-03", 9.0, 0.0),
            ],
        };
        assert_eq!(best(&[a, b]).map(|r| r.model), Some("B"));
        assert!(best(&[]).is_none());
    }
}
