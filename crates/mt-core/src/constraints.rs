//! Binding-constraint history: which transmission constraints bound in each
//! hour (DA) or five-minute interval (RT) of a market day, and at what shadow
//! price, from MISO's daily binding-constraints reports.

use std::collections::HashMap;

use chrono::{NaiveDate, NaiveDateTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Market {
    DayAhead,
    RealTime,
}

impl Market {
    pub fn label(self) -> &'static str {
        match self {
            Self::DayAhead => "DA",
            Self::RealTime => "RT",
        }
    }

    /// Length of one period: an hour in DA, five minutes in RT.
    pub fn period_hours(self) -> f64 {
        match self {
            Self::DayAhead => 1.0,
            Self::RealTime => 5.0 / 60.0,
        }
    }
}

/// One constraint binding in one period.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintRecord {
    /// MISO's constraint ID: the same constraint in DA and RT, whose names differ.
    pub id: u64,
    pub name: String,
    /// `element (type / from area / to area)`.
    pub branch: String,
    pub contingency: String,
    /// Hour-beginning (DA) or interval-beginning (RT), market time.
    pub start: NaiveDateTime,
    /// $/MWh. MISO publishes binding shadow prices as negative numbers.
    pub shadow_price: f64,
}

/// A market day's binding constraints.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintHistory {
    pub market: Market,
    pub day: NaiveDate,
    pub records: Vec<ConstraintRecord>,
}

/// One constraint's day, summed over its periods.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintSummary {
    pub id: u64,
    pub name: String,
    pub branch: String,
    pub contingency: String,
    /// Time bound, hours.
    pub hours: f64,
    /// Sum of |shadow price| x period length: $/MW over the day, the usual
    /// measure of how much a constraint cost.
    pub total: f64,
    /// Largest |shadow price|, $/MWh.
    pub max: f64,
}

impl ConstraintHistory {
    /// Per constraint, most costly first.
    pub fn summary(&self) -> Vec<ConstraintSummary> {
        let h = self.market.period_hours();
        let mut by_id: HashMap<u64, ConstraintSummary> = HashMap::new();
        for r in &self.records {
            let s = by_id.entry(r.id).or_insert_with(|| ConstraintSummary {
                id: r.id,
                name: r.name.clone(),
                branch: r.branch.clone(),
                contingency: r.contingency.clone(),
                hours: 0.0,
                total: 0.0,
                max: 0.0,
            });
            s.hours += h;
            s.total += r.shadow_price.abs() * h;
            s.max = s.max.max(r.shadow_price.abs());
        }
        let mut out: Vec<_> = by_id.into_values().collect();
        out.sort_by(|a, b| b.total.total_cmp(&a.total).then(a.id.cmp(&b.id)));
        out
    }

    /// One constraint's shadow prices through the day, in time order.
    pub fn series(&self, id: u64) -> Vec<(NaiveDateTime, f64)> {
        let mut out: Vec<_> = self
            .records
            .iter()
            .filter(|r| r.id == id)
            .map(|r| (r.start, r.shadow_price))
            .collect();
        out.sort_by_key(|p| p.0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_sum_cost_over_periods() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let rec = |id, h, m, sp| ConstraintRecord {
            id,
            name: format!("C{id}"),
            branch: String::new(),
            contingency: String::new(),
            start: day.and_hms_opt(h, m, 0).unwrap(),
            shadow_price: sp,
        };
        let rt = ConstraintHistory {
            market: Market::RealTime,
            day,
            records: vec![rec(1, 0, 0, -12.0), rec(1, 0, 5, -24.0), rec(2, 0, 0, -6.0)],
        };
        let s = rt.summary();
        assert_eq!(s[0].id, 1, "most costly first");
        assert!((s[0].hours - 10.0 / 60.0).abs() < 1e-9);
        assert!((s[0].total - 36.0 / 12.0).abs() < 1e-9);
        assert_eq!(s[0].max, 24.0);
        assert_eq!(rt.series(1).len(), 2);
    }
}
