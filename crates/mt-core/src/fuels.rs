//! Fuel prices: daily spot series such as Henry Hub natural gas.

use chrono::{Duration, NaiveDate};

/// A daily price series.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpotPrices {
    pub name: String,
    /// e.g. `$/MMBtu`.
    pub unit: String,
    /// Ascending by date; trading days only.
    pub points: Vec<(NaiveDate, f64)>,
}

impl SpotPrices {
    pub fn latest(&self) -> Option<(NaiveDate, f64)> {
        self.points.last().copied()
    }

    /// The points from `days` before the latest date onwards.
    pub fn recent(&self, days: i64) -> &[(NaiveDate, f64)] {
        let Some((last, _)) = self.latest() else {
            return &[];
        };
        let from = last - Duration::days(days);
        let i = self.points.partition_point(|p| p.0 < from);
        &self.points[i..]
    }

    /// The price on `day`, or the last one before it (weekends, holidays).
    pub fn on_or_before(&self, day: NaiveDate) -> Option<f64> {
        let i = self.points.partition_point(|p| p.0 <= day);
        i.checked_sub(1).map(|i| self.points[i].1)
    }
}

/// Implied market heat rate, MMBtu/MWh: how many MMBtu of gas the power price
/// buys. Near a gas unit's real heat rate, gas is on the margin.
pub fn implied_heat_rate(power: f64, gas: f64) -> Option<f64> {
    (gas > 0.05).then(|| power / gas)
}

/// Spark spread, $/MWh: the power price less the fuel cost of a unit with the
/// given heat rate.
pub fn spark_spread(power: f64, gas: f64, heat_rate: f64) -> f64 {
    power - gas * heat_rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_lookups_and_spreads() {
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        let s = SpotPrices {
            name: "HH".into(),
            unit: "$/MMBtu".into(),
            points: vec![(d(9, 25), 3.0), (d(9, 26), 3.2), (d(9, 29), 3.1)],
        };
        assert_eq!(s.latest(), Some((d(9, 29), 3.1)));
        assert_eq!(s.on_or_before(d(9, 27)), Some(3.2), "weekend uses Friday");
        assert_eq!(s.on_or_before(d(9, 1)), None);
        assert_eq!(s.recent(3).len(), 2);
        assert_eq!(implied_heat_rate(31.0, 3.1), Some(10.0));
        assert!((spark_spread(31.0, 3.1, 7.0) - 9.3).abs() < 1e-9);
    }
}
