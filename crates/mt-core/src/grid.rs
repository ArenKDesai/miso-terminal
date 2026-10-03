//! System conditions: load, generation mix, interchange, renewables, transmission
//! constraints, outages and capacity.

use chrono::{NaiveDate, NaiveDateTime};

/// A `(time, value)` point in market time.
pub type TimePoint = (NaiveDateTime, f64);

/// Real-time system load and the two forecasts it is judged against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemLoad {
    pub market_day: Option<NaiveDate>,
    /// When MISO stamped the payload.
    pub as_of: Option<NaiveDateTime>,
    /// Five-minute actual load, MW.
    pub actual_5min: Vec<TimePoint>,
    /// Day-ahead cleared load by hour ending (1..=24), MW.
    pub da_cleared: Vec<(u8, f64)>,
    /// Medium-term load forecast by hour ending (1..=24), MW.
    pub forecast: Vec<(u8, f64)>,
}

impl SystemLoad {
    pub fn latest(&self) -> Option<TimePoint> {
        self.actual_5min.last().copied()
    }

    pub fn forecast_peak(&self) -> Option<(u8, f64)> {
        self.forecast
            .iter()
            .copied()
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// Canonical fuel keys. Themes colour fuels by these keys, so a new MISO
/// category only needs a line in [`fuel_key`] to pick up a colour.
pub const FUEL_KEYS: [&str; 9] = [
    "coal", "gas", "nuclear", "wind", "solar", "hydro", "storage", "imports", "other",
];

/// Map a MISO fuel category ("Natural Gas", "Battery Storage", ...) to a canonical key.
pub fn fuel_key(category: &str) -> &'static str {
    let c = category.to_ascii_lowercase();
    if c.contains("coal") {
        "coal"
    } else if c.contains("gas") {
        "gas"
    } else if c.contains("nuclear") {
        "nuclear"
    } else if c.contains("wind") {
        "wind"
    } else if c.contains("solar") {
        "solar"
    } else if c.contains("hydro") {
        "hydro"
    } else if c.contains("storage") || c.contains("battery") {
        "storage"
    } else if c.contains("import") {
        "imports"
    } else {
        "other"
    }
}

/// Generation by fuel for one five-minute interval, MW.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FuelMix {
    pub interval: Option<NaiveDateTime>,
    pub total_mw: Option<f64>,
    /// `(MISO category, MW)` in MISO's order.
    pub fuels: Vec<(String, f64)>,
}

impl FuelMix {
    /// Total generation: MISO's reported total, else the sum of categories.
    pub fn total(&self) -> f64 {
        self.total_mw
            .filter(|t| *t > 0.0)
            .unwrap_or_else(|| self.fuels.iter().map(|(_, mw)| mw.max(0.0)).sum())
    }
}

/// Fuel mix for every interval of the day so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FuelMixHistory {
    pub intervals: Vec<FuelMix>,
}

impl FuelMixHistory {
    /// Every category that appears, in first-seen order.
    pub fn categories(&self) -> Vec<String> {
        let mut seen = Vec::<String>::new();
        for mix in &self.intervals {
            for (c, _) in &mix.fuels {
                if !seen.contains(c) {
                    seen.push(c.clone());
                }
            }
        }
        seen
    }

    pub fn series(&self, category: &str) -> Vec<TimePoint> {
        self.intervals
            .iter()
            .filter_map(|m| {
                let t = m.interval?;
                let mw = m.fuels.iter().find(|(c, _)| c == category)?.1;
                Some((t, mw))
            })
            .collect()
    }
}

/// Net scheduled interchange with each neighbouring balancing authority, MW.
/// Positive is export from MISO, negative is import.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interchange {
    pub time: Option<NaiveDateTime>,
    /// `(BA, MW)`; MISO's own net total is `MISO`.
    pub by_ba: Vec<(String, f64)>,
}

impl Interchange {
    pub fn net(&self) -> Option<f64> {
        self.by_ba
            .iter()
            .find(|(ba, _)| ba == "MISO")
            .map(|(_, v)| *v)
    }

    pub fn neighbours(&self) -> impl Iterator<Item = &(String, f64)> {
        self.by_ba.iter().filter(|(ba, _)| ba != "MISO")
    }
}

/// Five-minute interchange history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InterchangeHistory {
    pub points: Vec<Interchange>,
}

/// Hourly wind and solar, forecast against actual, MW.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenewableHour {
    /// Start of the hour (market time).
    pub start: NaiveDateTime,
    pub hour_ending: u8,
    pub wind_forecast: Option<f64>,
    pub solar_forecast: Option<f64>,
    pub wind_actual: Option<f64>,
    pub solar_actual: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Renewables {
    pub market_day: Option<NaiveDate>,
    pub hours: Vec<RenewableHour>,
}

/// A binding transmission constraint in the real-time market.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BindingConstraint {
    pub name: String,
    pub period: Option<NaiveDateTime>,
    /// Shadow price, $/MWh. Negative by MISO convention when binding.
    pub shadow_price: Option<f64>,
    /// Whether the constraint's demand curve was overridden.
    pub overridden: Option<bool>,
    pub curve_type: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BindingConstraints {
    pub interval: Option<NaiveDateTime>,
    pub constraints: Vec<BindingConstraint>,
}

/// Generation outages for one day, MW.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutageDay {
    pub day: NaiveDate,
    pub planned: f64,
    pub unplanned: f64,
    pub forced: f64,
    pub derated: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outages {
    /// MISO's headline, e.g. "Total Outage Megawatts: 54,179".
    pub headline: String,
    pub days: Vec<OutageDay>,
}

/// Committed capacity against demand (MISO's CSAT display), MW.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CapacityPoint {
    pub time: NaiveDateTime,
    pub demand: Option<f64>,
    pub committed: Option<f64>,
    pub demand_forecast: Option<f64>,
    pub committed_forecast: Option<f64>,
    pub available: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capacity {
    pub points: Vec<CapacityPoint>,
}

/// North-South regional directional transfer (the MISO Midwest/South
/// interface), five-minute. Positive flow is South to North.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransferPoint {
    pub time: NaiveDateTime,
    /// Flow as seen by the dispatch engine (UDS), MW.
    pub flow: Option<f64>,
    /// Raw measured flow, MW.
    pub raw: Option<f64>,
    /// Limit for North-to-South flow (negative), MW.
    pub north_south_limit: Option<f64>,
    /// Limit for South-to-North flow (positive), MW.
    pub south_north_limit: Option<f64>,
}

impl TransferPoint {
    /// Share of the limit in the direction of flow, 0-1 (or above, if violated).
    pub fn utilization(&self) -> Option<f64> {
        let flow = self.flow?;
        let limit = if flow >= 0.0 {
            self.south_north_limit?
        } else {
            self.north_south_limit?
        };
        (limit.abs() > 0.0).then(|| flow.abs() / limit.abs())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionalTransfer {
    pub points: Vec<TransferPoint>,
}

/// Area control error: MISO's real-time generation/load imbalance, MW.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ace {
    pub points: Vec<TimePoint>,
}

/// One headline figure from MISO's real-time snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotItem {
    pub title: String,
    pub value: Option<f64>,
    pub raw: String,
    pub time: Option<NaiveDateTime>,
}

/// MISO's headline numbers: demand, forecast peak, marginal energy cost, NSI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridSnapshot {
    pub items: Vec<SnapshotItem>,
}

impl GridSnapshot {
    /// Find an item whose title contains `needle` (case-insensitive).
    pub fn find(&self, needle: &str) -> Option<&SnapshotItem> {
        let needle = needle.to_ascii_lowercase();
        self.items
            .iter()
            .find(|i| i.title.to_ascii_lowercase().contains(&needle))
    }

    pub fn current_demand(&self) -> Option<&SnapshotItem> {
        self.find("current demand")
    }

    pub fn forecast_peak(&self) -> Option<&SnapshotItem> {
        self.find("peak demand")
    }

    pub fn marginal_energy_cost(&self) -> Option<&SnapshotItem> {
        self.find("marginal energy")
    }

    pub fn scheduled_interchange(&self) -> Option<&SnapshotItem> {
        self.find("imports")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuel_keys_cover_miso_categories() {
        assert_eq!(fuel_key("Natural Gas"), "gas");
        assert_eq!(fuel_key("Battery Storage"), "storage");
        assert_eq!(fuel_key("Imports"), "imports");
        assert_eq!(fuel_key("Hydro"), "hydro");
        assert_eq!(fuel_key("Something New"), "other");
        assert!(FUEL_KEYS.iter().all(|k| fuel_key(k) == *k || *k == "other"));
    }

    #[test]
    fn transfer_utilization_uses_the_limit_in_the_flow_direction() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 2)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let p = |flow| TransferPoint {
            time: t,
            flow: Some(flow),
            raw: None,
            north_south_limit: Some(-3000.0),
            south_north_limit: Some(2500.0),
        };
        assert_eq!(p(1250.0).utilization(), Some(0.5));
        assert_eq!(p(-1500.0).utilization(), Some(0.5));
    }

    #[test]
    fn fuel_mix_total_falls_back_to_sum() {
        let mix = FuelMix {
            interval: None,
            total_mw: Some(0.0),
            fuels: vec![
                ("Coal".into(), 10.0),
                ("Battery Storage".into(), -2.0),
                ("Wind".into(), 5.0),
            ],
        };
        assert_eq!(mix.total(), 15.0);
    }
}
