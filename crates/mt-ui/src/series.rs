//! Price series for one node, assembled from whichever feeds cover each span:
//! the five-minute intraday store for today, and for past days the price
//! history (the day store), else daily DA ex-post and RT final-or-preliminary
//! reports. Shared by GP, SPRD, CMP, HUBS and WL.

use std::collections::HashMap;

use chrono::{Duration, NaiveDate, NaiveDateTime, Timelike};
use mt_core::time::market_today;
use mt_core::{DayLmpReport, DayNodeRow, DayReportKind, Market, NodeSeries, RtIntraday};
use mt_miso::StoredPrices;
use mt_miso::parse::hourly_points;

use crate::context::PanelCx;

/// `(market time, value)` points, sorted by time.
pub type Points = Vec<(NaiveDateTime, f64)>;

/// Which part of the LMP to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Lmp,
    Energy,
    Congestion,
    Loss,
}

impl Component {
    pub const ALL: [Self; 4] = [Self::Lmp, Self::Energy, Self::Congestion, Self::Loss];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lmp => "LMP",
            Self::Energy => "Energy",
            Self::Congestion => "Congestion",
            Self::Loss => "Loss",
        }
    }

    pub fn pick(self, lmp: f32, mcc: f32, mlc: f32) -> f32 {
        match self {
            Self::Lmp => lmp,
            Self::Energy => lmp - mcc - mlc,
            Self::Congestion => mcc,
            Self::Loss => mlc,
        }
    }

    pub fn hourly(self, row: &DayNodeRow) -> [f32; 24] {
        std::array::from_fn(|h| self.pick(row.lmp[h], row.mcc[h], row.mlc[h]))
    }

    pub fn intraday(self, s: &NodeSeries<'_>) -> Points {
        s.intervals
            .iter()
            .enumerate()
            .map(|(i, t)| (*t, f64::from(self.pick(s.lmp[i], s.mcc[i], s.mlc[i]))))
            .filter(|(_, v)| v.is_finite())
            .collect()
    }
}

/// Hourly points for `node` from a daily report (empty if unpublished or absent).
pub fn report_points(
    report: Option<&Option<DayLmpReport>>,
    node: &str,
    component: Component,
    day: NaiveDate,
) -> Points {
    report
        .and_then(Option::as_ref)
        .and_then(|r| r.node(node))
        .map(|row| hourly_points(day, &component.hourly(row)))
        .unwrap_or_default()
}

/// Average five-minute points into hour-starting buckets.
pub fn hourly_average(pts: &[(NaiveDateTime, f64)]) -> Points {
    let mut out: Vec<(NaiveDateTime, f64, u32)> = Vec::new();
    for (t, v) in pts {
        // MISO stamps five-minute intervals by their start (00:00 to 23:55).
        let Some(start) = t.date().and_hms_opt(t.hour(), 0, 0) else {
            continue;
        };
        match out.last_mut() {
            Some((s, sum, n)) if *s == start => {
                *sum += v;
                *n += 1;
            }
            _ => out.push((start, *v, 1)),
        }
    }
    out.into_iter()
        .map(|(s, sum, n)| (s, sum / f64::from(n)))
        .collect()
}

/// `a − b` at the timestamps both share.
pub fn subtract(a: &[(NaiveDateTime, f64)], b: &[(NaiveDateTime, f64)]) -> Points {
    let b: HashMap<NaiveDateTime, f64> = b.iter().copied().collect();
    a.iter()
        .filter_map(|(t, v)| Some((*t, v - b.get(t)?)))
        .collect()
}

/// Population standard deviation.
pub fn std_dev(values: &[f64]) -> Option<f64> {
    let m = mean(values.iter().copied())?;
    mean(values.iter().map(|v| (v - m).powi(2))).map(f64::sqrt)
}

/// MISO's on-peak block: HE 8 to HE 23 (hours starting 07:00 to 22:00 EST),
/// Monday to Friday. NERC holidays (also off-peak) are not excluded.
pub fn is_on_peak(hour_start: NaiveDateTime) -> bool {
    use chrono::Datelike;
    let weekday = hour_start.weekday().number_from_monday() <= 5;
    weekday && (7..=22).contains(&hour_start.hour())
}

pub fn mean(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values
        .into_iter()
        .fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

/// Today's prices at a node: five-minute RT, DA for today, DA for tomorrow.
pub struct Today {
    pub rt_5min: Points,
    pub da: Points,
    pub da_tomorrow: Points,
    /// Whether the feeds have answered at all (to tell "loading" from "no such node").
    pub loaded: bool,
}

pub fn node_today(cx: &PanelCx<'_>, node: &str, component: Component) -> Today {
    let today = market_today();
    let tomorrow = today + Duration::days(1);
    let intraday = cx.hub.watch(&cx.miso.rt_intraday());
    let da = cx
        .hub
        .watch(&cx.miso.day_report(DayReportKind::DaExPost, today));
    let da_next = cx
        .hub
        .watch(&cx.miso.day_report(DayReportKind::DaExPost, tomorrow));
    Today {
        rt_5min: intraday
            .data()
            .and_then(|d| d.series(node))
            .map(|s| component.intraday(&s))
            .unwrap_or_default(),
        da: report_points(da.data(), node, component, today),
        da_tomorrow: report_points(da_next.data(), node, component, tomorrow),
        loaded: intraday.data.is_some() && da.data.is_some(),
    }
}

/// Hourly DA and RT at a node over the last `days` days (plus tomorrow's DA).
#[derive(Debug, Default)]
pub struct History {
    pub da: Points,
    pub rt: Points,
    /// Daily reports still downloading, or the price history still being read.
    pub pending: usize,
    /// Days whose RT is preliminary (final reports trail about a week).
    pub prelim_days: usize,
    /// Days in the window the price history does not hold that are too old
    /// for a chart to download ([`DOWNLOAD_DAYS`]): SET's *Price history*
    /// fills them.
    pub unstored_days: usize,
}

/// How far back a chart downloads the days the price history lacks (two
/// reports a day, every node in each); before that it shows what is stored.
pub const DOWNLOAD_DAYS: u32 = 90;

pub fn node_history(cx: &PanelCx<'_>, node: &str, component: Component, days: u32) -> History {
    nodes_history(cx, &[node], component, days)
        .pop()
        .unwrap_or_default()
}

/// [`node_history`] for several nodes, which read the price history together
/// (each stored day is read once for all of them).
pub fn nodes_history(
    cx: &PanelCx<'_>,
    nodes: &[&str],
    component: Component,
    days: u32,
) -> Vec<History> {
    let today = market_today();
    let first = today - Duration::days(i64::from(days.max(1)) - 1);
    let snap = cx.hub.watch(&cx.miso.stored_prices(nodes, first));
    // What is stored decides what to download, so wait for it.
    let stored = match (snap.data, snap.error) {
        (Some(s), _) => s,
        (None, Some(_)) => std::sync::Arc::default(),
        (None, None) => {
            return nodes
                .iter()
                .map(|_| History {
                    pending: 1,
                    ..History::default()
                })
                .collect();
        }
    };
    nodes
        .iter()
        .map(|node| one_history(cx, node, component, today, first, &stored))
        .collect()
}

fn one_history(
    cx: &PanelCx<'_>,
    node: &str,
    component: Component,
    today: NaiveDate,
    first: NaiveDate,
    stored: &StoredPrices,
) -> History {
    let mut h = History::default();
    let from_store = |market: Market, day: NaiveDate| -> Points {
        stored
            .row(node, market, day)
            .map(|row| hourly_points(day, &component.hourly(row)))
            .unwrap_or_default()
    };
    let download_from = today - Duration::days(i64::from(DOWNLOAD_DAYS) - 1);
    let mut unstored = std::collections::BTreeSet::new();
    let mut day = first;
    while day <= today + Duration::days(1) {
        // DA ex-post: stored, else downloaded if recent. A stored day without
        // this node has nothing to download either.
        if stored.stored(Market::DayAhead, day).is_some() {
            h.da.extend(from_store(Market::DayAhead, day));
        } else if day >= download_from {
            let d = cx
                .hub
                .watch(&cx.miso.day_report(DayReportKind::DaExPost, day));
            h.pending += usize::from(d.data.is_none() && d.error.is_none());
            h.da.extend(report_points(d.data(), node, component, day));
        } else {
            unstored.insert(day);
        }
        if day < today {
            // RT: a stored final report; a preliminary day recent enough to
            // download is asked for again, since its final may be out.
            let kind = stored.stored(Market::RealTime, day);
            if kind == Some(DayReportKind::RtFinal) {
                h.rt.extend(from_store(Market::RealTime, day));
            } else if day >= download_from {
                let r = cx.hub.watch(&cx.miso.rt_best_day(day));
                h.pending += usize::from(r.data.is_none() && r.error.is_none());
                if let Some(Some(report)) = r.data()
                    && report.kind == DayReportKind::RtPrelim
                {
                    h.prelim_days += 1;
                }
                h.rt.extend(report_points(r.data(), node, component, day));
            } else if kind == Some(DayReportKind::RtPrelim) {
                h.prelim_days += 1;
                h.rt.extend(from_store(Market::RealTime, day));
            } else {
                unstored.insert(day);
            }
        }
        day += Duration::days(1);
    }
    h.unstored_days = unstored.len();
    // Today's RT comes from the five-minute feed, averaged to hours.
    if let Some(s) = cx
        .hub
        .watch(&cx.miso.rt_intraday())
        .data()
        .and_then(|d| d.series(node))
    {
        h.rt.extend(hourly_average(&component.intraday(&s)));
    }
    h
}

/// Five-minute RT at a node over the last `days` days, with hourly DA for the
/// same span. Past days come from the local archive (saved while the app
/// runs) and, for yesterday, MISO's previous-day feed; today from the live feed.
pub struct FiveMinute {
    pub rt: Points,
    pub da: Points,
    /// Past days in the window with nothing saved.
    pub missing: Vec<NaiveDate>,
    /// Past days with only part of the day saved.
    pub partial: usize,
    /// Archive reads or downloads still in progress.
    pub pending: usize,
}

pub fn node_five_minute(
    cx: &PanelCx<'_>,
    node: &str,
    component: Component,
    days: u32,
) -> FiveMinute {
    let today = market_today();
    let first = today - Duration::days(i64::from(days.max(1)) - 1);
    let mut out = FiveMinute {
        rt: Vec::new(),
        da: Vec::new(),
        missing: Vec::new(),
        partial: 0,
        pending: 0,
    };
    let points = |s: &RtIntraday| {
        s.series(node)
            .map(|s| component.intraday(&s))
            .unwrap_or_default()
    };
    let mut day = first;
    while day <= today {
        let da = cx
            .hub
            .watch(&cx.miso.day_report(DayReportKind::DaExPost, day));
        out.pending += usize::from(da.data.is_none() && da.error.is_none());
        out.da
            .extend(report_points(da.data(), node, component, day));
        if day < today {
            let saved = cx.hub.watch(&cx.miso.rt_archive_day(day));
            let mut waiting = saved.data.is_none() && saved.error.is_none();
            let mut best = saved
                .data()
                .and_then(Option::as_ref)
                .map(|s| (points(s), s.intervals().len()));
            // Yesterday is still on MISO's previous-day feed, which fills the
            // archive in as a side effect.
            if day == today - Duration::days(1)
                && best
                    .as_ref()
                    .is_none_or(|(_, n)| *n < mt_miso::INTERVALS_PER_DAY)
            {
                let prev = cx.hub.watch(&cx.miso.rt_previous_day());
                waiting |= prev.data.is_none() && prev.error.is_none();
                if let Some(s) = prev.data().filter(|s| s.market_day == Some(day)) {
                    best = Some((points(s), s.intervals().len()));
                }
            }
            out.pending += usize::from(waiting);
            match best {
                Some((pts, n)) if !pts.is_empty() => {
                    out.partial += usize::from(n < mt_miso::INTERVALS_PER_DAY);
                    out.rt.extend(pts);
                }
                _ if !waiting => out.missing.push(day),
                _ => {}
            }
        } else if let Some(s) = cx.hub.watch(&cx.miso.rt_intraday()).data() {
            out.rt.extend(points(s));
        }
        day += Duration::days(1);
    }
    out
}

/// Summary statistics for a pair of hourly series (DA and RT, or two spreads).
#[derive(Debug, Default, PartialEq)]
pub struct Stats {
    pub da_avg: Option<f64>,
    pub rt_avg: Option<f64>,
    /// Mean of RT − DA over hours present in both.
    pub dart_avg: Option<f64>,
    pub matched: usize,
    pub rt_min: Option<f64>,
    pub rt_max: Option<f64>,
    pub rt_negative: usize,
}

impl Stats {
    pub fn of(da: &[(NaiveDateTime, f64)], rt: &[(NaiveDateTime, f64)]) -> Self {
        let spreads = subtract(rt, da);
        Self {
            da_avg: mean(da.iter().map(|p| p.1)),
            rt_avg: mean(rt.iter().map(|p| p.1)),
            dart_avg: mean(spreads.iter().map(|p| p.1)),
            matched: spreads.len(),
            rt_min: rt.iter().map(|p| p.1).min_by(f64::total_cmp),
            rt_max: rt.iter().map(|p| p.1).max_by(f64::total_cmp),
            rt_negative: rt.iter().filter(|p| p.1 < 0.0).count(),
        }
    }
}

/// The `p` quantile (0-1) of `values`, nearest-rank.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[((v.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize])
}

/// A price duration curve: values sorted high to low against the share of
/// hours (0-100%) at or above each value.
pub fn duration_curve(values: impl IntoIterator<Item = f64>) -> Vec<[f64; 2]> {
    let mut v: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
    v.sort_by(|a, b| b.total_cmp(a));
    let last = v.len().saturating_sub(1).max(1) as f64;
    v.into_iter()
        .enumerate()
        .map(|(i, y)| [i as f64 / last * 100.0, y])
        .collect()
}

/// CSV rows `time, DA, RT, RT − DA` over the union of hours, for export.
pub fn hourly_rows(da: &[(NaiveDateTime, f64)], rt: &[(NaiveDateTime, f64)]) -> Vec<Vec<String>> {
    let da_map: HashMap<NaiveDateTime, f64> = da.iter().copied().collect();
    let rt_map: HashMap<NaiveDateTime, f64> = rt.iter().copied().collect();
    let mut hours: Vec<NaiveDateTime> = da_map.keys().chain(rt_map.keys()).copied().collect();
    hours.sort();
    hours.dedup();
    let cell = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:.2}"));
    hours
        .into_iter()
        .map(|t| {
            let (d, r) = (da_map.get(&t).copied(), rt_map.get(&t).copied());
            vec![
                t.format("%Y-%m-%d %H:%M").to_string(),
                cell(d),
                cell(r),
                cell(r.zip(d).map(|(r, d)| r - d)),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 2)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn five_minute_points_average_into_hours() {
        let pts = [
            (t(0, 5), 10.0),
            (t(0, 30), 20.0),
            (t(1, 0), 30.0),
            (t(1, 5), 100.0),
        ];
        assert_eq!(hourly_average(&pts), vec![(t(0, 0), 15.0), (t(1, 0), 65.0)]);
    }

    #[test]
    fn stats_match_hours() {
        let da = [(t(0, 0), 10.0), (t(1, 0), 20.0)];
        let rt = [(t(0, 0), 15.0), (t(1, 0), -5.0), (t(2, 0), 40.0)];
        let s = Stats::of(&da, &rt);
        assert_eq!(s.matched, 2);
        assert_eq!(s.dart_avg, Some(-10.0)); // (15-10) and (-5-20)
        assert_eq!(s.rt_negative, 1);
        assert_eq!(s.rt_max, Some(40.0));
    }

    #[test]
    fn subtract_aligns_on_shared_timestamps() {
        let a = [(t(0, 0), 10.0), (t(0, 5), 12.0), (t(0, 10), 9.0)];
        let b = [(t(0, 0), 4.0), (t(0, 10), 10.0)];
        assert_eq!(subtract(&a, &b), vec![(t(0, 0), 6.0), (t(0, 10), -1.0)]);
    }

    #[test]
    fn on_peak_is_weekday_he8_to_he23() {
        // 2026-10-02 is a Friday, 2026-10-03 a Saturday.
        assert!(is_on_peak(t(7, 0)));
        assert!(is_on_peak(t(22, 0)));
        assert!(!is_on_peak(t(6, 0)));
        assert!(!is_on_peak(t(23, 0)));
        let saturday = NaiveDate::from_ymd_opt(2026, 10, 3)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        assert!(!is_on_peak(saturday));
        assert_eq!(
            std_dev(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]),
            Some(2.0)
        );
    }

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (0..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.99), Some(99.0));
        assert_eq!(percentile(&v, 0.0), Some(0.0));
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn duration_curve_runs_high_to_low_over_0_to_100() {
        let c = duration_curve([3.0, 1.0, f64::NAN, 2.0]);
        assert_eq!(c, vec![[0.0, 3.0], [50.0, 2.0], [100.0, 1.0]]);
        assert_eq!(duration_curve([5.0]), vec![[0.0, 5.0]]);
        assert!(duration_curve(Vec::new()).is_empty());
    }

    #[test]
    fn csv_rows_cover_the_union_of_hours() {
        let rows = hourly_rows(&[(t(0, 0), 10.0)], &[(t(0, 0), 12.5), (t(1, 0), 8.0)]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["2026-10-02 00:00", "10.00", "12.50", "2.50"]);
        assert_eq!(rows[1], vec!["2026-10-02 01:00", "", "8.00", ""]);
    }
}
