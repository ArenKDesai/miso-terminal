//! Prices: LMPs, the consolidated LMP board, intraday five-minute history, daily
//! hourly reports and ancillary-service clearing prices.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::constraints::Market;

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
            self.market_day = self.intervals.first().map(chrono::NaiveDateTime::date);
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

    /// Compact binary form, for keeping today's store across restarts.
    /// Layout (little-endian): magic, market day, interval count, node count,
    /// interval timestamps, node names, then LMP, MCC and MLC node-major.
    pub fn to_bytes(&self) -> Vec<u8> {
        let (nt, nn) = (self.intervals.len(), self.nodes.len());
        let mut out = Vec::with_capacity(32 + nt * 8 + nn * 24 + nn * nt * 12);
        out.extend_from_slice(INTRADAY_MAGIC);
        let day = self.market_day.map_or(i32::MIN, |d| d.num_days_from_ce());
        out.extend_from_slice(&day.to_le_bytes());
        out.extend_from_slice(&(nt as u32).to_le_bytes());
        out.extend_from_slice(&(nn as u32).to_le_bytes());
        for t in &self.intervals {
            out.extend_from_slice(&t.and_utc().timestamp().to_le_bytes());
        }
        for n in &self.nodes {
            let b = n.as_bytes();
            out.extend_from_slice(&(b.len() as u16).to_le_bytes());
            out.extend_from_slice(b);
        }
        for column in [&self.lmp, &self.mcc, &self.mlc] {
            for node in column {
                for v in node {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        out
    }

    /// Inverse of [`Self::to_bytes`]; `None` for anything malformed.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(INTRADAY_MAGIC.len())? != INTRADAY_MAGIC {
            return None;
        }
        let day = i32::from_le_bytes(r.array()?);
        let nt = u32::from_le_bytes(r.array()?) as usize;
        let nn = u32::from_le_bytes(r.array()?) as usize;
        let mut s = Self {
            market_day: NaiveDate::from_num_days_from_ce_opt(day),
            ..Self::default()
        };
        for _ in 0..nt {
            let ts = i64::from_le_bytes(r.array()?);
            s.intervals
                .push(chrono::DateTime::from_timestamp(ts, 0)?.naive_utc());
        }
        for i in 0..nn {
            let len = u16::from_le_bytes(r.array()?) as usize;
            let name = std::str::from_utf8(r.take(len)?).ok()?.to_owned();
            s.index.insert(name.clone(), i);
            s.nodes.push(name);
        }
        for column in [&mut s.lmp, &mut s.mcc, &mut s.mlc] {
            for _ in 0..nn {
                let mut node = Vec::with_capacity(nt);
                for _ in 0..nt {
                    node.push(f32::from_le_bytes(r.array()?));
                }
                column.push(node);
            }
        }
        (r.at == bytes.len()).then_some(s)
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

const INTRADAY_MAGIC: &[u8] = b"MTRT1\0";

/// A bounds-checked little-endian cursor.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
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

    /// The market a report prices.
    pub fn market(self) -> Market {
        match self {
            Self::DaExPost | Self::DaExAnte => Market::DayAhead,
            Self::RtFinal | Self::RtPrelim => Market::RealTime,
        }
    }

    const ALL: [Self; 4] = [
        Self::DaExPost,
        Self::DaExAnte,
        Self::RtFinal,
        Self::RtPrelim,
    ];

    fn code(self) -> u8 {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0) as u8
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

    /// Compact binary form, for the day store. Layout (little-endian): magic,
    /// kind, market day, decimals, node count, node names and types, then
    /// LMP, MCC and MLC node-major as `i32` in units of `10^-decimals` $
    /// (`i32::MIN` for a missing hour), split into byte planes so gzip finds
    /// the runs. About 170 KB a day gzipped for MISO's 2,600 nodes, against
    /// 280 KB for the CSV. Decimals 255 stores the `f32` bits as they are, for
    /// a report whose values are not whole cents.
    pub fn to_bytes(&self) -> Vec<u8> {
        let values: Vec<f32> = (0..3)
            .flat_map(|c| self.rows.iter().flat_map(move |r| [r.lmp, r.mcc, r.mlc][c]))
            .collect();
        let decimals = [2, 3, 4]
            .into_iter()
            .find(|&d| values.iter().all(|&v| to_fixed(v, d).is_some()))
            .unwrap_or(RAW_BITS);
        let ints: Vec<i32> = values
            .iter()
            .map(|&v| match decimals {
                RAW_BITS => v.to_bits() as i32,
                d => to_fixed(v, d).unwrap_or(MISSING),
            })
            .collect();
        let mut out = Vec::with_capacity(32 + self.rows.len() * 300);
        out.extend_from_slice(DAY_REPORT_MAGIC);
        out.push(self.kind.code());
        out.extend_from_slice(&self.day.num_days_from_ce().to_le_bytes());
        out.push(decimals);
        out.extend_from_slice(&(self.rows.len() as u32).to_le_bytes());
        for row in &self.rows {
            for s in [&row.node, &row.node_type] {
                let b = &s.as_bytes()[..s.len().min(usize::from(u16::MAX))];
                out.extend_from_slice(&(b.len() as u16).to_le_bytes());
                out.extend_from_slice(b);
            }
        }
        for plane in 0..4 {
            out.extend(ints.iter().map(|v| v.to_le_bytes()[plane]));
        }
        out
    }

    /// Inverse of [`Self::to_bytes`]; `None` for anything malformed.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(DAY_REPORT_MAGIC.len())? != DAY_REPORT_MAGIC {
            return None;
        }
        let kind = *DayReportKind::ALL.get(usize::from(r.take(1)?[0]))?;
        let day = NaiveDate::from_num_days_from_ce_opt(i32::from_le_bytes(r.array()?))?;
        let decimals = r.take(1)?[0];
        let n = u32::from_le_bytes(r.array()?) as usize;
        let mut rows = Vec::with_capacity(n.min(100_000));
        for _ in 0..n {
            let mut text = || -> Option<String> {
                let len = u16::from_le_bytes(r.array()?) as usize;
                Some(std::str::from_utf8(r.take(len)?).ok()?.to_owned())
            };
            let (node, node_type) = (text()?, text()?);
            rows.push(DayNodeRow {
                node,
                node_type,
                lmp: [f32::NAN; 24],
                mcc: [f32::NAN; 24],
                mlc: [f32::NAN; 24],
            });
        }
        let count = n.checked_mul(3 * 24)?;
        let planes = r.take(count.checked_mul(4)?)?;
        if r.at != bytes.len() {
            return None;
        }
        let scale = 10f64.powi(i32::from(decimals));
        let value = |i: usize| -> f32 {
            let v = i32::from_le_bytes(std::array::from_fn(|p| planes[p * count + i]));
            match (decimals, v) {
                (RAW_BITS, _) => f32::from_bits(v as u32),
                (_, MISSING) => f32::NAN,
                _ => (f64::from(v) / scale) as f32,
            }
        };
        for (c, i) in (0..3).flat_map(|c| (0..n).map(move |i| (c, i))) {
            let row = &mut rows[i];
            let target = match c {
                0 => &mut row.lmp,
                1 => &mut row.mcc,
                _ => &mut row.mlc,
            };
            let base = (c * n + i) * 24;
            for (h, slot) in target.iter_mut().enumerate() {
                *slot = value(base + h);
            }
        }
        Some(Self::new(kind, day, rows))
    }
}

const DAY_REPORT_MAGIC: &[u8] = b"MTDR1\0";
/// A missing hour in the day store's fixed-point columns.
const MISSING: i32 = i32::MIN;
/// The day store's "decimals" for values kept as raw `f32` bits.
const RAW_BITS: u8 = 255;

/// `v` in units of `10^-decimals`, if that gives back exactly the same `f32`
/// (a missing value is always representable).
fn to_fixed(v: f32, decimals: u8) -> Option<i32> {
    if v.is_nan() {
        return Some(MISSING);
    }
    let scale = 10f64.powi(i32::from(decimals));
    let fixed = (f64::from(v) * scale).round();
    let ok =
        fixed > f64::from(MISSING) && fixed <= f64::from(i32::MAX) && (fixed / scale) as f32 == v;
    ok.then_some(fixed as i32)
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
    fn intraday_round_trips_through_bytes() {
        let s = RtIntraday::from_rows([row(0, "A", 10.0), row(5, "A", 11.0), row(5, "BÉ", 20.0)]);
        let back = RtIntraday::from_bytes(&s.to_bytes()).unwrap();
        assert_eq!(back.market_day, s.market_day);
        assert_eq!(back.intervals(), s.intervals());
        assert_eq!(back.node_names(), s.node_names());
        assert_eq!(back.latest("A"), s.latest("A"));
        assert!(back.series("BÉ").unwrap().lmp[0].is_nan(), "gaps survive");
        // Truncated or foreign data is rejected, not misread.
        let bytes = s.to_bytes();
        assert!(RtIntraday::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        assert!(RtIntraday::from_bytes(b"nonsense").is_none());
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

    /// A price in cents as the report parser makes it: decimal text read as
    /// `f64`, then narrowed.
    fn cents(c: i32) -> f32 {
        (f64::from(c) / 100.0) as f32
    }

    fn day_row(node: &str, base: i32) -> DayNodeRow {
        DayNodeRow {
            node: node.into(),
            node_type: "Hub".into(),
            lmp: std::array::from_fn(|h| cents(base + h as i32 * 137)),
            mcc: std::array::from_fn(|h| cents(-(h as i32) * 5)),
            mlc: std::array::from_fn(|h| cents(h as i32 - 12)),
        }
    }

    /// Equal values, with NaN equal to NaN (a missing hour).
    fn same(a: &[f32; 24], b: &[f32; 24]) -> bool {
        a.iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()))
    }

    fn same_report(a: &DayLmpReport, b: &DayLmpReport) -> bool {
        a.kind == b.kind
            && a.day == b.day
            && a.rows.len() == b.rows.len()
            && a.rows.iter().zip(&b.rows).all(|(x, y)| {
                x.node == y.node
                    && x.node_type == y.node_type
                    && same(&x.lmp, &y.lmp)
                    && same(&x.mcc, &y.mcc)
                    && same(&x.mlc, &y.mlc)
            })
    }

    #[test]
    fn day_reports_round_trip_exactly() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let mut a = day_row("MINN.HUB", 1567);
        a.lmp[3] = f32::NAN;
        a.lmp[18] = cents(999_999);
        a.mcc[5] = cents(-123_456);
        let mut b = day_row("ALTE.ALTE", -4210);
        b.node_type = "Gennode".into();
        b.mlc = [f32::NAN; 24];
        let report = DayLmpReport::new(DayReportKind::RtFinal, day, vec![a, b]);
        let bytes = report.to_bytes();
        assert_eq!(bytes[DAY_REPORT_MAGIC.len() + 5], 2, "whole cents");
        let back = DayLmpReport::from_bytes(&bytes).unwrap();
        assert!(same_report(&report, &back));
        assert_eq!(back.node("ALTE.ALTE").unwrap().node_type, "Gennode");
        assert_eq!(back.day.to_string(), "2026-10-06");
        assert_eq!(back.kind.market(), Market::RealTime);

        // Something finer than a cent is kept too, just less compactly.
        let mut odd = day_row("X", 100);
        odd.lmp[0] = 0.123_456_7;
        let report = DayLmpReport::new(DayReportKind::DaExPost, day, vec![odd]);
        let bytes = report.to_bytes();
        assert_eq!(bytes[DAY_REPORT_MAGIC.len() + 5], RAW_BITS);
        assert!(same_report(
            &report,
            &DayLmpReport::from_bytes(&bytes).unwrap()
        ));

        // Truncated, padded or foreign bytes are refused.
        assert!(DayLmpReport::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        assert!(DayLmpReport::from_bytes(&[bytes.as_slice(), &[0]].concat()).is_none());
        assert!(DayLmpReport::from_bytes(b"MTRT1\0").is_none());
    }
}
