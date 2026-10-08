//! CMP with securities (`CMP XEL US MINN.HUB 30`): stocks and ETFs above
//! MISO node prices on one time axis, and how they moved together day by day.
//! Securities at fifteen minutes for a few days or daily closes beyond; nodes'
//! hourly RT or DA. One clock for both charts: MISO's market time (EST).

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use egui::{Grid, RichText, Ui};
use egui_plot::{Line, PlotPoints};
use mt_alpaca::Timeframe;
use mt_core::equity::Bar;
use mt_core::exchange;
use mt_core::instrument::Security;

use crate::context::PanelCx;
use crate::function::{Panel, Route};
use crate::market::{self, SecurityPicker};
use crate::series::{self, Component, Points};
use crate::widgets::chart::{self, utc_x};
use crate::widgets::node_picker::NodePicker;
use crate::widgets::{self, csv, fmt};

pub(crate) const MAX_SECURITIES: usize = 4;
pub(crate) const MAX_NODES: usize = 4;
/// Up to this many days, securities are drawn from fifteen-minute bars.
const INTRADAY_DAYS: u32 = 10;
/// Fewest day-to-day moves worth a correlation.
const MIN_PAIRS: usize = 5;

pub(crate) fn open(securities: Vec<Security>, nodes: Vec<String>, days: u32) -> Box<dyn Panel> {
    Box::new(Cross {
        securities,
        nodes,
        days,
        da: false,
        component: Component::Lmp,
        node_picker: NodePicker::default(),
        security_picker: SecurityPicker::default(),
    })
}

struct Cross {
    securities: Vec<Security>,
    nodes: Vec<String>,
    /// 0 until the configured default is applied on the first frame.
    days: u32,
    /// Nodes' DA rather than RT.
    da: bool,
    component: Component,
    node_picker: NodePicker,
    security_picker: SecurityPicker,
}

/// A security's line: x in Unix seconds, and its bars' closes by New York date.
struct SecurityLine {
    name: String,
    points: Vec<[f64; 2]>,
    closes: BTreeMap<NaiveDate, f64>,
    first: Option<f64>,
    last: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
}

/// Where a bar's close sits on the time axis: the end of an intraday bar, or
/// 16:00 New York time for a daily one.
fn close_instant(b: &Bar, timeframe: Timeframe) -> DateTime<Utc> {
    if timeframe == Timeframe::Day1 {
        let day = exchange::to_exchange(b.time).date_naive();
        day.and_hms_opt(16, 0, 0)
            .and_then(exchange::exchange_to_utc)
            .unwrap_or(b.time)
    } else {
        b.time + chrono::Duration::seconds(timeframe.seconds())
    }
}

fn security_line(name: String, bars: &[Bar], timeframe: Timeframe, as_pct: bool) -> SecurityLine {
    let first = bars.first().map(|b| b.close).filter(|c| *c > 0.0);
    let points = bars
        .iter()
        .filter(|b| b.close.is_finite())
        .map(|b| {
            let y = match (as_pct, first) {
                (true, Some(f)) => (b.close / f - 1.0) * 100.0,
                _ => b.close,
            };
            [utc_x(close_instant(b, timeframe)), y]
        })
        .collect();
    let mut closes = BTreeMap::new();
    for b in bars {
        closes.insert(exchange::to_exchange(b.time).date_naive(), b.close);
    }
    SecurityLine {
        name,
        points,
        closes,
        first,
        last: bars.last().map(|b| b.close),
        high: bars.iter().map(|b| b.high).max_by(f64::total_cmp),
        low: bars.iter().map(|b| b.low).min_by(f64::total_cmp),
    }
}

/// Split a line where consecutive points are more than `max_gap` seconds
/// apart (nights between intraday sessions), so no line bridges them.
fn runs(points: &[[f64; 2]], max_gap: f64) -> Vec<Vec<[f64; 2]>> {
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut prev: Option<f64> = None;
    for p in points {
        if prev.is_none_or(|x| p[0] - x > max_gap) {
            out.push(Vec::new());
        }
        if let Some(run) = out.last_mut() {
            run.push(*p);
        }
        prev = Some(p[0]);
    }
    out
}

/// A node's average price per market day.
fn daily_averages(points: &[(NaiveDateTime, f64)]) -> BTreeMap<NaiveDate, f64> {
    let mut sums: BTreeMap<NaiveDate, (f64, usize)> = BTreeMap::new();
    for (t, v) in points.iter().filter(|(_, v)| v.is_finite()) {
        let e = sums.entry(t.date()).or_insert((0.0, 0));
        e.0 += v;
        e.1 += 1;
    }
    sums.into_iter()
        .map(|(d, (sum, n))| (d, sum / n as f64))
        .collect()
}

/// How a security and a node moved together: the correlation of the
/// security's daily return (%) with the change in the node's daily average
/// price, over consecutive trading days both have. With the number of days.
pub(crate) fn daily_correlation(
    closes: &BTreeMap<NaiveDate, f64>,
    node_avg: &BTreeMap<NaiveDate, f64>,
) -> Option<(f64, usize)> {
    let days: Vec<(&NaiveDate, &f64)> = closes.iter().collect();
    let pairs: Vec<(f64, f64)> = days
        .windows(2)
        .filter_map(|w| {
            let ((d0, c0), (d1, c1)) = (w[0], w[1]);
            let (a0, a1) = (node_avg.get(d0)?, node_avg.get(d1)?);
            (*c0 > 0.0).then(|| ((c1 / c0 - 1.0) * 100.0, a1 - a0))
        })
        .collect();
    if pairs.len() < MIN_PAIRS {
        return None;
    }
    let n = pairs.len() as f64;
    let (mx, my) = (
        pairs.iter().map(|p| p.0).sum::<f64>() / n,
        pairs.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let cov: f64 = pairs.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
    let vx: f64 = pairs.iter().map(|(x, _)| (x - mx).powi(2)).sum();
    let vy: f64 = pairs.iter().map(|(_, y)| (y - my).powi(2)).sum();
    (vx > 0.0 && vy > 0.0).then(|| (cov / (vx * vy).sqrt(), pairs.len()))
}

impl Panel for Cross {
    fn title(&self) -> String {
        let mut names: Vec<String> = self.securities.iter().map(|s| s.ticker.clone()).collect();
        names.extend(self.nodes.iter().map(|n| mt_core::hub_short(n).to_owned()));
        if names.len() <= 3 {
            format!("CMP {}", names.join(" "))
        } else {
            format!("CMP {} series", names.len())
        }
    }

    fn route(&self) -> Route {
        let mut args: Vec<String> = self.securities.iter().map(ToString::to_string).collect();
        args.extend(self.nodes.iter().cloned());
        if self.days > 0 {
            args.push(self.days.to_string());
        }
        Route::new("CMP", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if self.days == 0 {
            self.days = cx.config.data.history_days.clamp(1, 90);
        }
        self.header(ui, cx);
        if market::needs_keys(ui, cx) {
            return;
        }
        if self.securities.is_empty() {
            ui.label(
                RichText::new("Add a security above, or type CMP XEL US MINN.HUB 30.")
                    .color(skin.text_muted),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for c in Component::ALL {
                ui.selectable_value(&mut self.component, c, c.label());
            }
            ui.separator();
            ui.selectable_value(&mut self.da, false, "RT");
            ui.selectable_value(&mut self.da, true, "DA");
            ui.separator();
            for d in super::gp::DAY_CHOICES {
                let label = if d == 365 {
                    "1y".into()
                } else {
                    format!("{d}d")
                };
                ui.selectable_value(&mut self.days, d, label);
            }
        });

        // Securities: one request for all of them.
        let tickers: Vec<String> = self.securities.iter().map(|s| s.ticker.clone()).collect();
        let timeframe = if self.days <= INTRADAY_DAYS {
            Timeframe::Min15
        } else {
            Timeframe::Day1
        };
        let start =
            exchange::now_exchange().date_naive() - chrono::Duration::days(i64::from(self.days));
        let snap = cx
            .hub
            .watch(&cx.alpaca.bars(&tickers, timeframe, start, None));
        let as_pct = self.securities.len() > 1;
        let lines: Vec<SecurityLine> = self
            .securities
            .iter()
            .map(|s| {
                let bars = snap.data().map(|d| d.get(&s.ticker)).unwrap_or_default();
                security_line(s.to_string(), bars, timeframe, as_pct)
            })
            .collect();

        // Nodes: hourly history, as CMP draws it.
        let histories: Vec<(String, series::History)> = self
            .nodes
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    series::node_history(cx, n, self.component, self.days),
                )
            })
            .collect();
        let node_lines: Vec<(&str, &Points)> = histories
            .iter()
            .map(|(n, h)| (n.as_str(), if self.da { &h.da } else { &h.rt }))
            .collect();
        let pending: usize = histories.iter().map(|(_, h)| h.pending).sum();

        self.summary(ui, cx, &lines, &node_lines);

        ui.horizontal_wrapped(|ui| {
            let source = snap.data().map_or("", |d| d.source.label());
            ui.label(
                RichText::new(format!(
                    "One clock: MISO market time (EST); US exchanges keep New York time, an hour \
                     ahead while daylight saving is on. Securities: {} bars, {source}; nodes: \
                     hourly {} {}.",
                    timeframe.label(),
                    if self.da { "DA" } else { "RT" },
                    self.component.label()
                ))
                .small()
                .color(skin.text_muted),
            );
            if pending > 0 {
                ui.spinner();
                ui.label(
                    RichText::new(format!("Fetching {pending} daily reports…"))
                        .small()
                        .color(skin.text_muted),
                );
            }
            csv::copy_button(ui, skin, || {
                let mut rows: Vec<Vec<String>> = Vec::new();
                for l in &lines {
                    for p in &l.points {
                        let t = DateTime::from_timestamp(p[0] as i64, 0)
                            .map(mt_core::time::to_market)
                            .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string());
                        rows.push(vec![l.name.clone(), t, format!("{}", p[1])]);
                    }
                }
                for (n, pts) in &node_lines {
                    for (t, v) in *pts {
                        rows.push(vec![
                            (*n).to_owned(),
                            t.format("%Y-%m-%d %H:%M").to_string(),
                            format!("{v:.2}"),
                        ]);
                    }
                }
                let value = if as_pct {
                    "value (securities: % change)"
                } else {
                    "value"
                };
                csv::to_csv(&["series", "time_est", value], rows)
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::freshness(ui, skin, &snap);
            });
        });

        let link = egui::Id::new(("cmp-cross", self.route().to_string()));
        let height = ui.available_height();
        let top_h = if self.nodes.is_empty() {
            height
        } else {
            (height * 0.5).max(120.0)
        };
        let gap = if timeframe == Timeframe::Day1 {
            f64::INFINITY
        } else {
            2.0 * 3600.0
        };
        // Both charts span the same time, whatever each has data for.
        let xs = lines
            .iter()
            .flat_map(|l| l.points.iter().map(|p| p[0]))
            .chain(node_lines.iter().flat_map(|(_, pts)| {
                pts.iter().flat_map(|(t, _)| {
                    [
                        mt_core::time::chart_x(*t),
                        mt_core::time::chart_x(*t) + 3600.0,
                    ]
                })
            }));
        let (lo, hi) = xs.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
            (lo.min(x), hi.max(x))
        });
        let span = |plot: egui_plot::Plot<'static>| {
            if lo.is_finite() && hi > lo {
                plot.include_x(lo).include_x(hi)
            } else {
                plot
            }
        };
        let mut top = span(chart::time_plot("cmp-cross-securities", skin))
            .height(top_h)
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false]);
        if as_pct {
            top = top.y_axis_formatter(|m, _| format!("{:+.0}%", m.value));
        }
        if snap.data.is_none() {
            widgets::placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string));
        } else {
            top.show(ui, |plot| {
                for (i, l) in lines.iter().enumerate() {
                    for run in runs(&l.points, gap) {
                        plot.line(
                            Line::new(l.name.as_str(), PlotPoints::from(run))
                                .color(skin.series(i))
                                .width(1.6),
                        );
                    }
                }
            });
        }
        if self.nodes.is_empty() {
            ui.label(
                RichText::new("Add a node above to see its prices underneath.")
                    .color(skin.text_muted),
            );
            return;
        }
        let first_node = self.securities.len();
        let daily = timeframe == Timeframe::Day1;
        span(chart::time_plot("cmp-cross-nodes", skin))
            .height((ui.available_height() - 4.0).max(100.0))
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .show(ui, |plot| {
                for (i, (n, pts)) in node_lines.iter().enumerate() {
                    let color = skin.series(first_node + i);
                    if !daily {
                        chart::hourly_steps(plot, n, pts, color);
                        continue;
                    }
                    // Against daily closes, the daily average reads best;
                    // the hours stay faintly behind it.
                    chart::hourly_steps(plot, n, pts, color.gamma_multiply(0.3));
                    let avg: Points = daily_averages(pts)
                        .into_iter()
                        .filter_map(|(d, v)| Some((d.and_hms_opt(0, 0, 0)?, v)))
                        .collect();
                    for run in chart::step_runs(&avg, 86_400.0) {
                        plot.line(
                            Line::new(format!("{n} daily average"), PlotPoints::from(run))
                                .color(color)
                                .width(2.2),
                        );
                    }
                }
            });
    }
}

impl Cross {
    /// The series as chips with a lamp in their colour, and pickers to add more.
    fn header(&mut self, ui: &mut Ui, cx: &PanelCx<'_>) {
        let skin = cx.skin;
        ui.horizontal_wrapped(|ui| {
            let (mut drop_sec, mut drop_node) = (None, None);
            for (i, s) in self.securities.iter().enumerate() {
                widgets::lamp(ui, skin.series(i));
                ui.label(RichText::new(s.to_string()).color(skin.text_strong));
                if ui.small_button("✕").clicked() {
                    drop_sec = Some(i);
                }
                ui.add_space(6.0);
            }
            for (i, n) in self.nodes.iter().enumerate() {
                widgets::lamp(ui, skin.series(self.securities.len() + i));
                ui.label(RichText::new(n).color(skin.text_strong));
                if ui.small_button("✕").clicked() {
                    drop_node = Some(i);
                }
                ui.add_space(6.0);
            }
            if let Some(i) = drop_sec {
                self.securities.remove(i);
            }
            if let Some(i) = drop_node {
                self.nodes.remove(i);
            }
            if self.securities.len() < MAX_SECURITIES
                && cx.alpaca.is_ready()
                && let Some(s) = self
                    .security_picker
                    .show(ui, cx, "cmp-sec", "add a security…")
                && !self.securities.contains(&s)
            {
                self.securities.push(s);
            }
            if self.nodes.len() < MAX_NODES
                && let Some(n) = self.node_picker.show(ui, cx, "cmp-node", "add a node…")
                && !self.nodes.contains(&n)
            {
                self.nodes.push(n);
            }
        });
    }

    /// Each series' figures, and how each security moved with each node.
    fn summary(
        &self,
        ui: &mut Ui,
        cx: &PanelCx<'_>,
        lines: &[SecurityLine],
        nodes: &[(&str, &Points)],
    ) {
        let skin = cx.skin;
        ui.horizontal_wrapped(|ui| {
            Grid::new("cmp-cross-securities-summary")
                .striped(true)
                .num_columns(5)
                .spacing([14.0, 3.0])
                .show(ui, |ui| {
                    for h in ["Security", "Last", "Return", "High", "Low"] {
                        widgets::label(ui, skin, h);
                    }
                    ui.end_row();
                    for (i, l) in lines.iter().enumerate() {
                        ui.horizontal(|ui| {
                            widgets::lamp(ui, skin.series(i));
                            ui.label(&l.name);
                        });
                        let ret = l
                            .first
                            .zip(l.last)
                            .map(|(f, last)| (last / f - 1.0) * 100.0);
                        ui.label(market::fmt::price_opt(l.last));
                        ui.label(
                            RichText::new(market::fmt::pct_opt(ret))
                                .color(ret.map_or(skin.text_muted, |r| skin.delta(r))),
                        );
                        ui.label(market::fmt::price_opt(l.high));
                        ui.label(market::fmt::price_opt(l.low));
                        ui.end_row();
                    }
                });
            ui.add_space(18.0);
            if !nodes.is_empty() {
                Grid::new("cmp-cross-nodes-summary")
                    .striped(true)
                    .num_columns(4)
                    .spacing([14.0, 3.0])
                    .show(ui, |ui| {
                        for h in ["Node", "Latest", "Average", "Max"] {
                            widgets::label(ui, skin, h);
                        }
                        ui.end_row();
                        for (i, (n, pts)) in nodes.iter().enumerate() {
                            ui.horizontal(|ui| {
                                widgets::lamp(ui, skin.series(lines.len() + i));
                                ui.label(*n);
                            });
                            let vals = pts.iter().map(|p| p.1);
                            ui.label(fmt::price_opt(pts.last().map(|p| p.1)));
                            ui.label(fmt::price_opt(series::mean(vals.clone())));
                            ui.label(fmt::price_opt(vals.max_by(f64::total_cmp)));
                            ui.end_row();
                        }
                    });
                ui.add_space(18.0);
                Grid::new("cmp-cross-correlation")
                    .num_columns(nodes.len() + 1)
                    .spacing([14.0, 3.0])
                    .show(ui, |ui| {
                        widgets::label(ui, skin, "Daily moves together").on_hover_text(
                            "Correlation of each security's daily return with the day-to-day \
                                 change in the node's daily average price, over the trading days \
                                 both have. +1: they rise and fall together; 0: unrelated; −1: \
                                 opposite. A rough guide over short windows.",
                        );
                        for (n, _) in nodes {
                            widgets::label(ui, skin, mt_core::hub_short(n));
                        }
                        ui.end_row();
                        for l in lines {
                            ui.label(&l.name);
                            for (_, pts) in nodes {
                                let text = match daily_correlation(&l.closes, &daily_averages(pts))
                                {
                                    Some((r, n)) => format!("{r:+.2} ({n} d)"),
                                    None => fmt::DASH.into(),
                                };
                                ui.label(RichText::new(text).monospace()).on_hover_text(
                                    "Needs at least five day-to-day moves both have: try 14d or longer.",
                                );
                            }
                            ui.end_row();
                        }
                    });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
    }

    #[test]
    fn moves_together_or_not() {
        // The node's daily average rises by $2 for every 1% the stock gains.
        let rets = [1.0, -0.5, 2.0, 0.0, -1.5, 0.7, 1.2];
        let mut closes = BTreeMap::new();
        let mut avgs = BTreeMap::new();
        let (mut c, mut a) = (100.0, 30.0);
        closes.insert(day(1), c);
        avgs.insert(day(1), a);
        for (i, r) in rets.iter().enumerate() {
            c *= 1.0 + r / 100.0;
            a += 2.0 * r;
            closes.insert(day(2 + i as u32), c);
            avgs.insert(day(2 + i as u32), a);
        }
        let (r, n) = daily_correlation(&closes, &avgs).unwrap();
        assert!((r - 1.0).abs() < 1e-6, "{r}");
        assert_eq!(n, rets.len());
        // Opposite moves.
        let flipped: BTreeMap<_, _> = avgs.iter().map(|(d, v)| (*d, -v)).collect();
        assert!((daily_correlation(&closes, &flipped).unwrap().0 + 1.0).abs() < 1e-6);
        // Days the node lacks drop out; too few pairs is no answer.
        let sparse: BTreeMap<_, _> = avgs.iter().take(4).map(|(d, v)| (*d, *v)).collect();
        assert_eq!(daily_correlation(&closes, &sparse), None);
        // A flat price has no correlation.
        let flat: BTreeMap<_, _> = avgs.keys().map(|d| (*d, 30.0)).collect();
        assert_eq!(daily_correlation(&closes, &flat), None);
    }

    #[test]
    fn daily_closes_sit_at_four_in_new_york_and_lines_break_overnight() {
        let bar = |t: &str| Bar {
            time: t.parse().unwrap(),
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
            trades: None,
            vwap: None,
        };
        // A summer daily bar (midnight EDT) closes at 20:00 UTC.
        let daily = close_instant(&bar("2026-10-02T04:00:00Z"), Timeframe::Day1);
        assert_eq!(daily.to_rfc3339(), "2026-10-02T20:00:00+00:00");
        let q = close_instant(&bar("2026-10-02T13:30:00Z"), Timeframe::Min15);
        assert_eq!(q.to_rfc3339(), "2026-10-02T13:45:00+00:00");
        let pts = [[0.0, 1.0], [900.0, 1.0], [50_000.0, 1.0]];
        assert_eq!(runs(&pts, 7200.0).len(), 2);
        assert_eq!(runs(&pts, f64::INFINITY).len(), 1);
        let avg = daily_averages(&[
            (day(1).and_hms_opt(0, 0, 0).unwrap(), 10.0),
            (day(1).and_hms_opt(1, 0, 0).unwrap(), 20.0),
            (day(2).and_hms_opt(0, 0, 0).unwrap(), f64::NAN),
        ]);
        assert_eq!(avg.get(&day(1)), Some(&15.0));
        assert!(!avg.contains_key(&day(2)));
    }

    #[test]
    fn routes_round_trip() {
        let p = open(vec![Security::us("XEL")], vec!["MINN.HUB".into()], 30);
        assert_eq!(p.route(), Route::new("CMP", ["XEL US", "MINN.HUB", "30"]));
        assert_eq!(p.title(), "CMP XEL MINN");
    }
}
