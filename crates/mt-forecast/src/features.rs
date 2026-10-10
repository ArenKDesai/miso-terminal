//! What the learned models see for each hour of a target day, built only from
//! what was known when the forecast would have been made.
//!
//! - **DA tomorrow** is forecast in the morning, before MISO posts the DA
//!   results: DA is known through today, RT through yesterday (the last
//!   complete day), forecasts as issued before 09:00.
//! - **RT tomorrow** is forecast in the afternoon, once tomorrow's DA is
//!   posted, so tomorrow's DA is a feature (and the model learns the spread
//!   between the two): RT is known through yesterday, forecasts as issued
//!   before 15:00.
//!
//! Henry Hub gas is lagged a week (EIA publishes the daily spot weekly).
//! Each input is read through an accessor that refuses anything past its
//! cut-off, so a feature cannot see the future by mistake.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::calendar::{self, DayKind};
use crate::hourly::{Hourly, hour_start};

/// Which price a forecast is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    DayAhead,
    RealTime,
}

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Self::DayAhead => "DA",
            Self::RealTime => "RT",
        }
    }

    /// What was known when the forecast for `day` would have been made.
    pub fn known(self, day: NaiveDate) -> Known {
        let eve = day - Duration::days(1);
        Known {
            da_until: match self {
                Self::DayAhead => hour_start(day, 0),
                Self::RealTime => hour_start(day, 24),
            },
            rt_until: hour_start(eve, 0),
            gas_until: day - Duration::days(8),
            issued_before: match self {
                Self::DayAhead => hour_start(eve, 9),
                Self::RealTime => hour_start(eve, 15),
            },
        }
    }

    /// The series it forecasts, and the moment up to which that series is
    /// known for `day` (what the statistical models are given).
    pub fn series(self, inputs: &NodeInputs) -> &Hourly {
        match self {
            Self::DayAhead => &inputs.da,
            Self::RealTime => &inputs.rt,
        }
    }

    pub fn known_until(self, day: NaiveDate) -> NaiveDateTime {
        let k = self.known(day);
        match self {
            Self::DayAhead => k.da_until,
            Self::RealTime => k.rt_until,
        }
    }

    /// The last target day whose outcome is known when `day` is forecast.
    pub fn last_outcome(self, day: NaiveDate) -> NaiveDate {
        match self {
            Self::DayAhead => day - Duration::days(1),
            Self::RealTime => day - Duration::days(2),
        }
    }
}

/// Cut-offs for each input: values at or after these are unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    pub da_until: NaiveDateTime,
    pub rt_until: NaiveDateTime,
    pub gas_until: NaiveDate,
    pub issued_before: NaiveDateTime,
}

/// A forecast kept as issued (MISO's load forecast, wind and solar, NWS
/// temperatures): for each hour it covered, every version, with when it was
/// fetched.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Issued {
    pub name: String,
    /// Target hour → (fetched at, value), oldest first.
    pub versions: BTreeMap<NaiveDateTime, Vec<(NaiveDateTime, f64)>>,
}

impl Issued {
    /// The latest value for `hour` fetched before `before`.
    pub fn as_of(&self, hour: NaiveDateTime, before: NaiveDateTime) -> Option<f64> {
        self.versions
            .get(&hour)?
            .iter()
            .rev()
            .find(|(at, _)| *at < before)
            .map(|v| v.1)
    }
}

/// Everything known about a node and the system, all times market time.
#[derive(Clone, Debug, Default)]
pub struct NodeInputs {
    pub da: Hourly,
    pub rt: Hourly,
    /// The DA congestion component at the node.
    pub congestion: Hourly,
    /// Henry Hub spot, $/MMBtu, by day.
    pub gas: BTreeMap<NaiveDate, f64>,
    pub issued: Vec<Issued>,
}

/// Rows whose issued forecasts must exist before that forecast becomes a
/// feature: 60 days of hours, and four rows in five.
const ISSUED_MIN_ROWS: usize = 60 * 24;
const ISSUED_MIN_SHARE: f64 = 0.8;

/// Feature columns, in order. The issued forecasts that have earned a place
/// follow these.
pub const BASE_COLUMNS: [&str; 16] = [
    "Hour",
    "Weekday",
    "Kind of day",
    "Holiday",
    "Month",
    "Anchor",
    "DA day average",
    "DA a day earlier",
    "DA a week earlier",
    "RT last day known",
    "RT last day average",
    "RT a week earlier",
    "RT − DA, last week",
    "Congestion",
    "Congestion day average",
    "Henry Hub gas",
];

/// The columns a model uses: the base ones plus the issued forecasts (by
/// index into [`NodeInputs::issued`]) with enough history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Columns {
    pub issued: Vec<usize>,
}

impl Columns {
    /// Every column's name.
    pub fn names(&self, inputs: &NodeInputs) -> Vec<String> {
        BASE_COLUMNS
            .iter()
            .map(|s| (*s).to_owned())
            .chain(self.issued.iter().map(|&i| inputs.issued[i].name.clone()))
            .collect()
    }

    pub fn count(&self) -> usize {
        BASE_COLUMNS.len() + self.issued.len()
    }

    /// The issued forecasts known for at least four rows in five, and at
    /// least sixty days of them, over the target days `days`.
    pub fn choose(inputs: &NodeInputs, target: Target, days: &[NaiveDate]) -> Self {
        let issued = (0..inputs.issued.len())
            .filter(|&i| {
                let found = days
                    .iter()
                    .flat_map(|&d| {
                        let before = target.known(d).issued_before;
                        (0..24).map(move |h| (hour_start(d, h), before))
                    })
                    .filter(|&(t, b)| inputs.issued[i].as_of(t, b).is_some())
                    .count();
                found >= ISSUED_MIN_ROWS
                    && found as f64 >= ISSUED_MIN_SHARE * (days.len() * 24) as f64
            })
            .collect();
        Self { issued }
    }
}

/// A day's 24 rows of features, and the anchor each prediction is relative
/// to (DA tomorrow: today's DA at that hour; RT tomorrow: tomorrow's DA).
/// Models learn the target less its anchor, which keeps them on the right
/// level when prices shift. `None` for hours without an anchor.
pub fn day_rows(
    inputs: &NodeInputs,
    target: Target,
    columns: &Columns,
    day: NaiveDate,
) -> [Option<(Vec<f64>, f64)>; 24] {
    let k = target.known(day);
    let da = |t: NaiveDateTime| {
        (t < k.da_until)
            .then(|| inputs.da.get(&t).copied())
            .flatten()
    };
    let rt = |t: NaiveDateTime| {
        (t < k.rt_until)
            .then(|| inputs.rt.get(&t).copied())
            .flatten()
    };
    let mcc = |t: NaiveDateTime| {
        (t < k.da_until)
            .then(|| inputs.congestion.get(&t).copied())
            .flatten()
    };
    let nan = |v: Option<f64>| v.unwrap_or(f64::NAN);
    let day_mean = |f: &dyn Fn(NaiveDateTime) -> Option<f64>, d: NaiveDate| {
        let v: Vec<f64> = (0..24).filter_map(|h| f(hour_start(d, h))).collect();
        (v.len() >= 20).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    let before = |n: i64| day - Duration::days(n);
    // The DA day the anchor comes from: today for DA, tomorrow itself for RT.
    let anchor_day = match target {
        Target::DayAhead => before(1),
        Target::RealTime => day,
    };
    let last_rt_day = before(2);
    let gas = inputs
        .gas
        .range(..=k.gas_until)
        .next_back()
        .filter(|(d, _)| (k.gas_until - **d).num_days() <= 14)
        .map(|(_, v)| *v);
    let kind = DayKind::of(day);
    let da_mean = day_mean(&da, anchor_day);
    let rt_mean = day_mean(&rt, last_rt_day);
    let mcc_mean = day_mean(&mcc, anchor_day);
    std::array::from_fn(|h| {
        let at = |d: NaiveDate| hour_start(d, h);
        let anchor = da(at(anchor_day))?;
        let spreads: Vec<f64> = (2..=8)
            .filter_map(|n| Some(rt(at(before(n)))? - da(at(before(n)))?))
            .collect();
        let mut row = vec![
            h as f64,
            f64::from(day.weekday().num_days_from_monday()),
            match kind {
                DayKind::Weekday => 0.0,
                DayKind::Saturday => 1.0,
                DayKind::SundayOrHoliday => 2.0,
            },
            f64::from(u8::from(calendar::is_holiday(day))),
            f64::from(day.month()),
            anchor,
            nan(da_mean),
            nan(da(at(anchor_day - Duration::days(1)))),
            nan(da(at(before(7)))),
            nan(rt(at(last_rt_day))),
            nan(rt_mean),
            nan(rt(at(before(7)))),
            if spreads.len() >= 3 {
                spreads.iter().sum::<f64>() / spreads.len() as f64
            } else {
                f64::NAN
            },
            nan(mcc(at(anchor_day))),
            nan(mcc_mean),
            nan(gas),
        ];
        row.extend(
            columns
                .issued
                .iter()
                .map(|&i| nan(inputs.issued[i].as_of(at(day), k.issued_before))),
        );
        Some((row, anchor))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    /// DA worth 100 + day index, RT 200 + day index, congestion 1, gas 3.
    fn inputs() -> NodeInputs {
        let start = day("2026-09-01");
        let mut i = NodeInputs::default();
        for d in 0..40 {
            let date = start + Duration::days(d);
            for h in 0..24 {
                let t = hour_start(date, h);
                i.da.insert(t, 100.0 + d as f64);
                i.rt.insert(t, 200.0 + d as f64);
                i.congestion.insert(t, 1.0);
            }
            i.gas.insert(date, 3.0);
        }
        i
    }

    #[test]
    fn features_see_only_what_was_known() {
        let inputs = inputs();
        let columns = Columns { issued: vec![] };
        // Forecasting DA for Oct 1 (day index 30): today's DA (29) anchors.
        let rows = day_rows(&inputs, Target::DayAhead, &columns, day("2026-10-01"));
        let (row, anchor) = rows[5].clone().unwrap();
        assert_eq!(row.len(), columns.count());
        assert_eq!(anchor, 129.0);
        assert_eq!(row[0], 5.0, "hour");
        assert_eq!(row[1], 3.0, "Thursday");
        assert_eq!(row[7], 128.0, "DA a day before the anchor");
        // RT only through Sep 29 (index 28).
        assert_eq!(row[9], 228.0);
        assert_eq!(row[10], 228.0);
        // RT − DA over Sep 23 to 29 is 100 every day.
        assert_eq!(row[12], 100.0);
        assert_eq!(row[15], 3.0, "gas a week back");
        // RT for Oct 1 is forecast with Oct 1's own DA, once posted.
        let rows = day_rows(&inputs, Target::RealTime, &columns, day("2026-10-01"));
        assert_eq!(rows[0].as_ref().unwrap().1, 130.0);
        assert_eq!(
            rows[0].as_ref().unwrap().0[9],
            228.0,
            "still yesterday's RT"
        );
        // No anchor, no row: DA for a day whose eve has no DA.
        assert!(day_rows(&inputs, Target::DayAhead, &columns, day("2026-10-20"))[0].is_none());
    }

    #[test]
    fn known_cut_offs() {
        let k = Target::DayAhead.known(day("2026-10-01"));
        assert_eq!(k.da_until, hour_start(day("2026-10-01"), 0));
        assert_eq!(k.rt_until, hour_start(day("2026-09-30"), 0));
        assert_eq!(k.issued_before, hour_start(day("2026-09-30"), 9));
        let k = Target::RealTime.known(day("2026-10-01"));
        assert_eq!(k.da_until, hour_start(day("2026-10-02"), 0));
        assert_eq!(k.issued_before, hour_start(day("2026-09-30"), 15));
        assert_eq!(
            Target::RealTime.last_outcome(day("2026-10-01")),
            day("2026-09-29")
        );
    }

    #[test]
    fn issued_forecasts_join_with_enough_history() {
        let mut inputs = inputs();
        // A load forecast issued at 06:00 the day before each day, from Sep 1.
        let mut load = Issued {
            name: "Load forecast".into(),
            ..Issued::default()
        };
        for d in 0..40 {
            let date = day("2026-09-01") + Duration::days(d);
            for h in 0..24 {
                let fetched = hour_start(date - Duration::days(1), 6);
                load.versions
                    .entry(hour_start(date, h))
                    .or_default()
                    .push((fetched, 70_000.0 + d as f64));
            }
        }
        // A later version, fetched after the DA forecast's cut-off.
        load.versions
            .get_mut(&hour_start(day("2026-10-01"), 0))
            .unwrap()
            .push((hour_start(day("2026-09-30"), 12), 1.0));
        inputs.issued.push(load);
        let days: Vec<NaiveDate> = (2..40)
            .map(|d| day("2026-09-01") + Duration::days(d))
            .collect();
        // 38 days is short of 60: not yet a feature.
        assert!(
            Columns::choose(&inputs, Target::DayAhead, &days)
                .issued
                .is_empty()
        );
        let columns = Columns { issued: vec![0] };
        let row = day_rows(&inputs, Target::DayAhead, &columns, day("2026-10-01"))[0]
            .clone()
            .unwrap()
            .0;
        assert_eq!(
            row[16], 70_030.0,
            "as issued before 09:00, not the later version"
        );
        let rt_row = day_rows(&inputs, Target::RealTime, &columns, day("2026-10-01"))[0]
            .clone()
            .unwrap()
            .0;
        assert_eq!(rt_row[16], 1.0, "the RT forecast is made after it");
        assert_eq!(columns.names(&inputs)[16], "Load forecast");
    }
}
