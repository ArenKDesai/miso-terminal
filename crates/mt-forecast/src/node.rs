//! A node's forecast for one day: every model's 24 hours, its record over a
//! backtest of recent days, and conformal bands from that record. The learned
//! models (gradient-boosted trees and ridge regression) are refitted every
//! week of the backtest, each time on the days whose outcome was known then.

use chrono::{Duration, NaiveDate};

use crate::evaluate::{self, Accuracy, Bands, Outcome, Record};
use crate::features::{self, Columns, NodeInputs, Target};
use crate::gbm::{self, Gbm, Matrix};
use crate::hourly::{self, Day};
use crate::models;
use crate::ridge::Ridge;

/// The learned models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Learner {
    Trees,
    Ridge,
}

impl Learner {
    pub const ALL: [Self; 2] = [Self::Trees, Self::Ridge];

    pub fn name(self) -> &'static str {
        match self {
            Self::Trees => "Gradient-boosted trees",
            Self::Ridge => "Ridge regression",
        }
    }
}

/// How much history the learned models train on and the backtest covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Target days a learned model trains on, at most.
    pub train_days: usize,
    /// Recent days forecast in the backtest.
    pub backtest_days: usize,
    /// Days between refits of a learned model in the backtest.
    pub refit_days: usize,
    /// Target days needed before a learned model is fitted at all.
    pub min_train_days: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            train_days: 365,
            backtest_days: 56,
            refit_days: 7,
            min_train_days: 28,
        }
    }
}

enum Fitted {
    Trees(Gbm),
    Ridge(Ridge),
}

/// A learned model fitted on target days before a cut-off.
struct Trained {
    model: Fitted,
    columns: Columns,
}

impl Trained {
    /// Train on the target days up to `last` (the latest outcome known), at
    /// most `settings.train_days` of them.
    fn fit(
        learner: Learner,
        inputs: &NodeInputs,
        target: Target,
        last: NaiveDate,
        settings: &Settings,
    ) -> Option<Self> {
        let series = target.series(inputs);
        let days: Vec<NaiveDate> = (0..settings.train_days as i64)
            .rev()
            .map(|k| last - Duration::days(k))
            .filter(|d| hourly::full_day(series, *d).is_some())
            .collect();
        if days.len() < settings.min_train_days {
            return None;
        }
        let columns = Columns::choose(inputs, target, &days);
        let (mut x, mut y) = (Vec::new(), Vec::new());
        for &d in &days {
            let actual = hourly::full_day(series, d)?;
            for (h, row) in features::day_rows(inputs, target, &columns, d)
                .into_iter()
                .enumerate()
            {
                if let Some((row, anchor)) = row {
                    x.extend(row);
                    y.push(actual[h] - anchor);
                }
            }
        }
        // One scarcity hour should not set the model's course.
        hourly::winsorize(&mut y, 0.005);
        let m = Matrix {
            values: &x,
            features: columns.count(),
        };
        let model = match learner {
            Learner::Trees => Fitted::Trees(Gbm::fit(&m, &y, &gbm::Params::default())?),
            Learner::Ridge => Fitted::Ridge(Ridge::fit(&m, &y)?),
        };
        Some(Self { model, columns })
    }

    fn forecast(&self, inputs: &NodeInputs, target: Target, day: NaiveDate) -> Option<Day> {
        let rows = features::day_rows(inputs, target, &self.columns, day);
        let mut out = [0.0; 24];
        for (o, row) in out.iter_mut().zip(rows) {
            let (row, anchor) = row?;
            *o = anchor
                + match &self.model {
                    Fitted::Trees(m) => m.predict(&row),
                    Fitted::Ridge(m) => m.predict(&row),
                };
        }
        out.iter().all(|v| v.is_finite()).then_some(out)
    }
}

/// One model's part in a forecast.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelForecast {
    pub name: &'static str,
    /// The target day's 24 hours, if the model could forecast it.
    pub hours: Option<Day>,
    pub record: Record,
    pub accuracy: Option<Accuracy>,
    /// From the record's recent errors.
    pub bands: Option<Bands>,
    /// The learned model's features, by name.
    pub features: Vec<String>,
}

/// Every model's forecast of one node's price for `day`.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeForecast {
    pub target: Target,
    pub day: NaiveDate,
    pub models: Vec<ModelForecast>,
    /// The model with the lowest error over the days all models forecast.
    pub best: Option<usize>,
}

impl NodeForecast {
    /// The best model's forecast, if any.
    pub fn chosen(&self) -> Option<&ModelForecast> {
        self.best.map(|i| &self.models[i])
    }

    /// Whether the best model beats the seasonal naive ones (the first two)
    /// on mean absolute error over the backtest.
    pub fn beats_naive(&self) -> Option<bool> {
        let best = self.chosen()?.accuracy?.mae;
        let naive = self.models[..2]
            .iter()
            .filter_map(|m| m.accuracy.map(|a| a.mae))
            .fold(f64::INFINITY, f64::min);
        naive.is_finite().then_some(best < naive)
    }
}

/// Forecast `target` at a node for `day`: the statistical models, then the
/// learned ones, each with a backtest over the `settings.backtest_days`
/// before the last outcome known.
pub fn forecast(
    inputs: &NodeInputs,
    target: Target,
    day: NaiveDate,
    settings: &Settings,
) -> NodeForecast {
    let series = target.series(inputs);
    let last_known = target.last_outcome(day);
    let backtest_days: Vec<NaiveDate> = (0..settings.backtest_days as i64)
        .rev()
        .map(|k| last_known - Duration::days(k))
        .collect();
    let mut out: Vec<ModelForecast> = Vec::new();

    let stats = models::statistical();
    let records = evaluate::backtest(&stats, series, &backtest_days, |d| target.known_until(d));
    let cut = target.known_until(day);
    let known: hourly::Hourly = series.range(..cut).map(|(t, v)| (*t, *v)).collect();
    for (model, record) in stats.iter().zip(records) {
        out.push(finish(
            model.name(),
            model.forecast(&known, day),
            record,
            Vec::new(),
        ));
    }

    for learner in Learner::ALL {
        let record = backtest_learned(learner, inputs, target, &backtest_days, settings);
        let trained = Trained::fit(learner, inputs, target, last_known, settings);
        let hours = trained
            .as_ref()
            .and_then(|t| t.forecast(inputs, target, day));
        let features = trained
            .as_ref()
            .map(|t| t.columns.names(inputs))
            .unwrap_or_default();
        out.push(finish(learner.name(), hours, record, features));
    }

    let candidates: Vec<Record> = out
        .iter()
        .filter(|m| m.hours.is_some())
        .map(|m| m.record.clone())
        .collect();
    let best = evaluate::best(&candidates).and_then(|r| out.iter().position(|m| m.name == r.model));
    NodeForecast {
        target,
        day,
        models: out,
        best,
    }
}

fn finish(
    name: &'static str,
    hours: Option<Day>,
    record: Record,
    features: Vec<String>,
) -> ModelForecast {
    ModelForecast {
        name,
        hours,
        accuracy: evaluate::accuracy(&record),
        bands: Bands::from_record(&record),
        record,
        features,
    }
}

/// Backtest a learned model, refitting every `settings.refit_days` on the
/// outcomes known at the time.
fn backtest_learned(
    learner: Learner,
    inputs: &NodeInputs,
    target: Target,
    days: &[NaiveDate],
    settings: &Settings,
) -> Record {
    let series = target.series(inputs);
    let mut record = Record {
        model: learner.name(),
        days: Vec::new(),
    };
    for block in days.chunks(settings.refit_days.max(1)) {
        let Some(trained) = Trained::fit(
            learner,
            inputs,
            target,
            target.last_outcome(block[0]),
            settings,
        ) else {
            continue;
        };
        for &day in block {
            if let (Some(forecast), Some(actual)) = (
                trained.forecast(inputs, target, day),
                hourly::full_day(series, day),
            ) {
                record.days.push(Outcome {
                    day,
                    forecast,
                    actual,
                });
            }
        }
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hourly::hour_start;

    /// Prices with a daily shape, a weekly swing, a slow trend in gas and
    /// RT a steady premium over DA in the evening.
    pub(super) fn inputs(days: i64) -> NodeInputs {
        let start: NaiveDate = "2026-03-01".parse().unwrap();
        let mut i = NodeInputs::default();
        for d in 0..days {
            let date = start + Duration::days(d);
            let gas = 3.0 + d as f64 / 100.0;
            i.gas.insert(date, gas);
            let weekday = matches!(
                crate::calendar::DayKind::of(date),
                crate::calendar::DayKind::Weekday
            );
            for h in 0..24 {
                let t = hour_start(date, h);
                let shape = 10.0 * ((h as f64 - 6.0) / 24.0 * std::f64::consts::TAU).sin();
                let level = 8.0 * gas + if weekday { 5.0 } else { -5.0 };
                // A deterministic wobble so nothing is exactly repeatable.
                let wobble = ((d * 24 + h as i64) as f64 * 1.7).sin() * 2.0;
                let da = level + shape + wobble;
                i.da.insert(t, da);
                i.rt.insert(t, da + if h >= 17 { 6.0 } else { 0.0 } + wobble);
                i.congestion.insert(t, wobble / 4.0);
            }
        }
        i
    }

    #[test]
    fn every_model_forecasts_and_is_scored() {
        let inputs = inputs(150);
        let settings = Settings {
            train_days: 90,
            backtest_days: 21,
            refit_days: 7,
            min_train_days: 28,
        };
        let day: NaiveDate = "2026-07-28".parse().unwrap();
        for target in [Target::DayAhead, Target::RealTime] {
            let f = forecast(&inputs, target, day, &settings);
            let names: Vec<&str> = f.models.iter().map(|m| m.name).collect();
            assert_eq!(
                names,
                [
                    "Same hour, last day",
                    "Same hour, last week",
                    "Hour-by-day profile",
                    "Exponential smoothing (MSTL)",
                    "Gradient-boosted trees",
                    "Ridge regression",
                ]
            );
            for m in &f.models {
                assert!(m.hours.is_some(), "{} {}", target.label(), m.name);
                assert!(m.record.days.len() >= 14, "{} {}", target.label(), m.name);
                assert!(m.accuracy.is_some(), "{}", m.name);
            }
            let trees = &f.models[4];
            assert_eq!(trees.features.len(), features::BASE_COLUMNS.len());
            assert!(trees.bands.is_some(), "a backtest of three weeks has bands");
            // The learned models know RT's evening premium over tomorrow's DA,
            // and the gas trend; they should beat repeating a past day.
            assert_eq!(f.beats_naive(), Some(true), "{}", target.label());
            let chosen = f.chosen().unwrap().name;
            assert!(
                chosen == "Gradient-boosted trees" || chosen == "Ridge regression",
                "{} {chosen}",
                target.label()
            );
        }
    }

    #[test]
    fn too_little_history_leaves_learned_models_out() {
        let inputs = inputs(20);
        let f = forecast(
            &inputs,
            Target::DayAhead,
            "2026-03-21".parse().unwrap(),
            &Settings::default(),
        );
        assert!(f.models[0].hours.is_some(), "the naive day is there");
        assert!(f.models[4].hours.is_none() && f.models[5].hours.is_none());
        assert!(f.models[4].record.days.is_empty());
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    /// `cargo test -p mt-forecast --release -- --ignored --nocapture timing`
    #[test]
    #[ignore = "a timing, not a check"]
    fn a_year_of_history() {
        let inputs = tests_inputs(460);
        let day: NaiveDate = "2027-06-01".parse().unwrap();
        for target in [Target::DayAhead, Target::RealTime] {
            let t = std::time::Instant::now();
            let f = forecast(&inputs, target, day, &Settings::default());
            eprintln!(
                "{}: {:?}, best {:?}, trees {:?}",
                target.label(),
                t.elapsed(),
                f.chosen().map(|m| m.name),
                f.models
                    .iter()
                    .map(|m| m
                        .accuracy
                        .map(|a| (m.name, (a.mae * 100.0).round() / 100.0)))
                    .collect::<Vec<_>>()
            );
        }
    }

    fn tests_inputs(days: i64) -> NodeInputs {
        super::tests::inputs(days)
    }
}
