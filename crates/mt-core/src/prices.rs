//! Prices: LMPs, the consolidated LMP board, intraday five-minute history, daily
//! hourly reports and ancillary-service clearing prices.

use std::collections::HashMap;

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// MISO's eight trading hubs, in display order.
pub const TRADING_HUBS: [&str; 8] = [
    "ARKANSAS.HUB",
    "ILLINOIS.HUB",
    "INDIANA.HUB",
    "LOUISIANA.HUB",
    "MICHIGAN.HUB",
    "MINN.HUB",
    "MS.HUB",
    "TEXAS.HUB",
];

/// True for the eight `*.HUB` trading hubs. MISO's report `Type` column also says
/// "Hub" for ~445 commercial pricing nodes, so never use that column for this.
pub fn is_trading_hub(node: &str) -> bool {
    node.ends_with(".HUB")
}

/// Short label for a trading hub: `MICHIGAN.HUB` -> `MICHIGAN`.
pub fn hub_short(node: &str) -> &str {
    node.strip_suffix(".HUB").unwrap_or(node)
}

/// A locational marginal price and its components, $/MWh.
/// MISO convention: `lmp = energy + mcc + mlc`.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Lmp {
    pub lmp: f64,
    /// Marginal congestion component.
    pub mcc: f64,
    /// Marginal loss component.
    pub mlc: f64,
}

impl Lmp {
    pub fn new(lmp: f64, mcc: f64, mlc: f64) -> Self {
        Self { lmp, mcc, mlc }
    }

    /// Marginal energy component (system-wide).
    pub fn energy(&self) -> f64 {
        self.lmp - self.mcc - self.mlc
    }
}

/// One pricing node on the consolidated LMP board.
#[derive(Clone, Debug, PartialEq)]
pub struct LmpBoardRow {
    pub node: String,
    /// MISO's region label: North, Midwest, South.
    pub region: String,
    /// Latest five-minute real-time ex-post price.
    pub rt_5min: Option<Lmp>,
    /// Real-time hourly integrated price for the current hour so far.
    pub rt_hourly: Option<Lmp>,
    /// Day-ahead ex-ante price for the current hour.
    pub da_exante: Option<Lmp>,
    /// Day-ahead ex-post price for the current hour.
    pub da_expost: Option<Lmp>,
}

impl LmpBoardRow {
    /// Real-time minus day-ahead (ex-post), the "DART" spread.
    pub fn dart(&self) -> Option<f64> {
        Some(self.rt_5min?.lmp - self.da_expost?.lmp)
    }
}

/// The consolidated LMP table: ~300 key nodes with RT and DA prices side by side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LmpBoard {
    /// Five-minute interval the RT prices belong to.
    pub interval: Option<NaiveDateTime>,
    /// Hour ending the RT hourly-integrated column covers.
    pub rt_hour_ending: Option<u8>,
    /// Hour ending the DA columns cover.
    pub da_hour_ending: Option<u8>,
    pub rows: Vec<LmpBoardRow>,
}

impl LmpBoard {
    pub fn row(&self, node: &str) -> Option<&LmpBoardRow> {
        self.rows.iter().find(|r| r.node == node)
    }

    pub fn hubs(&self) -> impl Iterator<Item = &LmpBoardRow> {
        TRADING_HUBS.iter().filter_map(|h| self.row(h))
    }
}

/// Real-time ex-ante prices at the hubs for the upcoming interval.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HubExAnte {
    pub interval: Option<NaiveDateTime>,
    pub hubs: Vec<(String, Lmp)>,
}

/// One row of MISO's five-minute ex-post feed.
#[derive(Clone, Debug, PartialEq)]
pub struct RtRow {
    pub interval: NaiveDateTime,
    pub node: String,
    pub lmp: f64,
    pub mcc: f64,
    pub mlc: f64,
}

/// Borrowed view of one node's intraday series. Missing intervals are `NaN`.
#[derive(Clone, Copy, Debug)]
pub struct NodeSeries<'a> {
    pub intervals: &'a [NaiveDateTime],
    pub lmp: &'a [f32],
    pub mcc: &'a [f32],
    pub mlc: &'a [f32],
}

impl NodeSeries<'_> {
    /// `(interval, value)` pairs for one component, skipping gaps.
    pub fn points(&self, values: &[f32]) -> Vec<(NaiveDateTime, f64)> {
        self.intervals
            .iter()
            .zip(values)
            .filter(|(_, v)| v.is_finite())
            .map(|(t, v)| (*t, f64::from(*v)))
            .collect()
    }
}

/// Columnar store of five-minute RT prices for every CP node, for one market day.
///
/// Seeded from the "rolling" feed (the whole day so far) and topped up from the
/// "current" feed (the latest interval), so polling stays cheap.
#[derive(Clone, Debug, Default)]
pub struct RtIntraday {
    pub market_day: Option<NaiveDate>,
    intervals: Vec<NaiveDateTime>,
    nodes: Vec<String>,
    index: HashMap<String, usize>,
    lmp: Vec<Vec<f32>>,
    mcc: Vec<Vec<f32>>,
    mlc: Vec<Vec<f32>>,
}

impl RtIntraday {
    pub fn from_rows(rows: impl IntoIterator<Item = RtRow>) -> Self {
        let mut s = Self::default();
        s.merge_rows(rows);
        s
    }

    /// Merge rows in; existing cells are overwritten. Returns how many new
    /// intervals were added.
    pub fn merge_rows(&mut self, rows: impl IntoIterator<Item = RtRow>) -> usize {
        let before = self.intervals.len();
        for r in rows {
            let t = self.interval_slot(r.interval);
            let n = self.node_slot(&r.node);
            self.lmp[n][t] = r.lmp as f32;
            self.mcc[n][t] = r.mcc as f32;
            self.mlc[n][t] = r.mlc as f32;
        }
        if self.market_day.is_none() {
            self.market_day = self.intervals.first().map(|t| t.date());
        }
        self.intervals.len() - before
    }

    fn interval_slot(&mut self, t: NaiveDateTime) -> usize {
        match self.intervals.binary_search(&t) {
            Ok(i) => i,
            Err(i) => {
                self.intervals.insert(i, t);
                for col in self
                    .lmp
                    .iter_mut()
                    .chain(&mut self.mcc)
                    .chain(&mut self.mlc)
                {
                    col.insert(i, f32::NAN);
                }
                i
            }
        }
    }

    fn node_slot(&mut self, node: &str) -> usize {
        if let Some(&i) = self.index.get(node) {
            return i;
        }
        let i = self.nodes.len();
        self.nodes.push(node.to_owned());
        self.index.insert(node.to_owned(), i);
        let empty = vec![f32::NAN; self.intervals.len()];
        self.lmp.push(empty.clone());
        self.mcc.push(empty.clone());
        self.mlc.push(empty);
        i
    }

    pub fn intervals(&self) -> &[NaiveDateTime] {
        &self.intervals
    }

    pub fn latest_interval(&self) -> Option<NaiveDateTime> {
        self.intervals.last().copied()
    }

    pub fn node_names(&self) -> &[String] {
        &self.nodes
    }

    pub fn contains(&self, node: &str) -> bool {
        self.index.contains_key(node)
    }

    pub fn series(&self, node: &str) -> Option<NodeSeries<'_>> {
        let i = *self.index.get(node)?;
        Some(NodeSeries {
            intervals: &self.intervals,
            lmp: &self.lmp[i],
            mcc: &self.mcc[i],
            mlc: &self.mlc[i],
        })
    }

    fn at(&self, node: usize, t: usize) -> Option<Lmp> {
        let lmp = self.lmp[node][t];
        lmp.is_finite().then(|| {
            Lmp::new(
                f64::from(lmp),
                f64::from(self.mcc[node][t]),
                f64::from(self.mlc[node][t]),
            )
        })
    }

    /// Latest available price for `node`, and the interval it belongs to.
    pub fn latest(&self, node: &str) -> Option<(NaiveDateTime, Lmp)> {
        let n = *self.index.get(node)?;
        (0..self.intervals.len())
            .rev()
            .find_map(|t| self.at(n, t).map(|p| (self.intervals[t], p)))
    }

    /// The price one interval before [`Self::latest`], for tick direction.
    pub fn previous(&self, node: &str) -> Option<Lmp> {
        let n = *self.index.get(node)?;
        let mut found = (0..self.intervals.len())
            .rev()
            .filter_map(|t| self.at(n, t));
        found.next()?;
        found.next()
    }

    /// Latest price at every node for the most recent interval.
    pub fn latest_all(&self) -> Vec<(&str, Lmp)> {
        let Some(t) = self.intervals.len().checked_sub(1) else {
            return Vec::new();
        };
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(n, name)| self.at(n, t).map(|p| (name.as_str(), p)))
            .collect()
    }
}

/// Which daily hourly report a [`DayLmpReport`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DayReportKind {
    DaExPost,
    DaExAnte,
    RtFinal,
    RtPrelim,
}

impl DayReportKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::DaExPost => "DA ex-post",
            Self::DaExAnte => "DA ex-ante",
            Self::RtFinal => "RT final",
            Self::RtPrelim => "RT prelim",
        }
    }
}

/// One node's 24 hourly values from a daily report. Missing hours are `NaN`.
#[derive(Clone, Debug, PartialEq)]
pub struct DayNodeRow {
    pub node: String,
    /// The report's `Type` column: Hub, Interface, Loadzone, Gennode.
    pub node_type: String,
    pub lmp: [f32; 24],
    pub mcc: [f32; 24],
    pub mlc: [f32; 24],
}

/// A whole day of hourly prices for every node, from one MISO market report.
#[derive(Clone, Debug)]
pub struct DayLmpReport {
    pub kind: DayReportKind,
    pub day: NaiveDate,
    pub rows: Vec<DayNodeRow>,
    index: HashMap<String, usize>,
}

impl DayLmpReport {
    pub fn new(kind: DayReportKind, day: NaiveDate, rows: Vec<DayNodeRow>) -> Self {
        let index = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.node.clone(), i))
            .collect();
        Self {
            kind,
            day,
            rows,
            index,
        }
    }

    pub fn node(&self, node: &str) -> Option<&DayNodeRow> {
        self.index.get(node).map(|&i| &self.rows[i])
    }
}

/// Ancillary-service market clearing prices for one reserve zone, $/MW.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneMcp {
    pub zone: String,
    pub regulation: Option<f64>,
    pub spinning: Option<f64>,
    pub supplemental: Option<f64>,
    pub short_term: Option<f64>,
    pub ramp_up: Option<f64>,
    pub ramp_down: Option<f64>,
}

/// Real-time ancillary-service MCPs by reserve zone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AncillaryMcps {
    pub interval: Option<NaiveDateTime>,
    pub zones: Vec<ZoneMcp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 2)
            .unwrap()
            .and_hms_opt(16, min, 0)
            .unwrap()
    }

    fn row(min: u32, node: &str, lmp: f64) -> RtRow {
        RtRow {
            interval: t(min),
            node: node.into(),
            lmp,
            mcc: 1.0,
            mlc: 0.5,
        }
    }

    #[test]
    fn intraday_merges_and_tracks_latest() {
        let mut s =
            RtIntraday::from_rows([row(0, "A", 10.0), row(0, "B", 20.0), row(5, "A", 11.0)]);
        assert_eq!(s.intervals().len(), 2);
        assert_eq!(s.market_day, Some(t(0).date()));
        assert_eq!(s.latest("A").unwrap(), (t(5), Lmp::new(11.0, 1.0, 0.5)));
        // B has no value at 16:05, so its latest is 16:00.
        assert_eq!(s.latest("B").unwrap().0, t(0));
        assert_eq!(s.previous("A").unwrap().lmp, 10.0);
        assert!(s.previous("B").is_none());

        // A new interval, a correction to an old one, and a new node.
        let added = s.merge_rows([row(10, "A", 12.0), row(0, "A", 9.5), row(10, "C", 30.0)]);
        assert_eq!(added, 1);
        assert_eq!(
            s.series("A").unwrap().points(s.series("A").unwrap().lmp)[0].1,
            9.5
        );
        assert_eq!(s.series("C").unwrap().lmp.len(), 3);
        assert!(s.series("C").unwrap().lmp[0].is_nan());
        assert_eq!(s.latest_all().len(), 2); // A and C at 16:10
    }

    #[test]
    fn intraday_handles_out_of_order_intervals() {
        let s = RtIntraday::from_rows([row(10, "A", 3.0), row(0, "A", 1.0), row(5, "A", 2.0)]);
        assert_eq!(s.intervals(), &[t(0), t(5), t(10)]);
        let a = s.series("A").unwrap();
        assert_eq!(a.lmp, &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn lmp_components() {
        let p = Lmp::new(40.0, 3.0, 1.0);
        assert_eq!(p.energy(), 36.0);
        assert!(is_trading_hub("MINN.HUB"));
        assert!(!is_trading_hub("ALTE.ALTE"));
        assert_eq!(hub_short("MINN.HUB"), "MINN");
    }
}
