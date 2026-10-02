//! GP: graph a node's price. Today's five-minute RT against the DA ex-post
//! staircase, or N days of hourly DA against RT with summary statistics.

use chrono::{Duration, NaiveDate, NaiveDateTime, Timelike};
use egui::{RichText, Ui};
use mt_core::time::market_today;
use mt_core::{DayLmpReport, DayNodeRow, DayReportKind, NodeSeries};
use mt_miso::parse::hourly_points;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "GP",
    aliases: &["GRAPH", "CHART"],
    name: "Graph price",
    category: Category::Prices,
    usage: "GP <node> [days]",
    description: "Price chart for one node: today's 5-minute RT vs DA, or hourly DA vs RT over N days, by component.",
    takes_node: true,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let node = args
        .first()
        .map(|n| n.trim().to_ascii_uppercase())
        .filter(|n| !n.is_empty());
    let days = match args.get(1) {
        Some(d) => d
            .parse::<u32>()
            .map_err(|_| format!("days must be a number, got {d:?}"))?
            .clamp(1, 90),
        None => 0, // "use the configured default" (resolved on first draw)
    };
    Ok(Box::new(Gp {
        node,
        days,
        search: String::new(),
        // Asking for a number of days means asking for history.
        view: if days > 0 { View::History } else { View::Today },
        component: Component::Lmp,
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Today,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Component {
    Lmp,
    Energy,
    Congestion,
    Loss,
}

impl Component {
    const ALL: [Self; 4] = [Self::Lmp, Self::Energy, Self::Congestion, Self::Loss];

    fn label(self) -> &'static str {
        match self {
            Self::Lmp => "LMP",
            Self::Energy => "Energy",
            Self::Congestion => "Congestion",
            Self::Loss => "Loss",
        }
    }

    fn pick(self, lmp: f32, mcc: f32, mlc: f32) -> f32 {
        match self {
            Self::Lmp => lmp,
            Self::Energy => lmp - mcc - mlc,
            Self::Congestion => mcc,
            Self::Loss => mlc,
        }
    }

    fn hourly(self, row: &DayNodeRow) -> [f32; 24] {
        std::array::from_fn(|h| self.pick(row.lmp[h], row.mcc[h], row.mlc[h]))
    }

    fn intraday(self, s: &NodeSeries<'_>) -> Vec<(NaiveDateTime, f64)> {
        s.intervals
            .iter()
            .enumerate()
            .map(|(i, t)| (*t, f64::from(self.pick(s.lmp[i], s.mcc[i], s.mlc[i]))))
            .filter(|(_, v)| v.is_finite())
            .collect()
    }
}

struct Gp {
    node: Option<String>,
    days: u32,
    search: String,
    view: View,
    component: Component,
}

impl Panel for Gp {
    fn title(&self) -> String {
        match &self.node {
            Some(n) => format!("GP {n}"),
            None => "GP".into(),
        }
    }

    fn route(&self) -> Route {
        match &self.node {
            Some(n) if self.days > 0 => Route::new("GP", [n.clone(), self.days.to_string()]),
            Some(n) => Route::new("GP", [n.clone()]),
            None => Route::code("GP"),
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if self.days == 0 {
            self.days = cx.config.data.history_days.clamp(1, 90);
        }
        self.node_picker(ui, cx);
        let Some(node) = self.node.clone() else {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Pick a node above, or type GP <node> on the command line.")
                    .color(skin.text_muted),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view, View::Today, "Today · 5-min");
            ui.selectable_value(&mut self.view, View::History, "History · hourly");
            ui.separator();
            for c in Component::ALL {
                ui.selectable_value(&mut self.component, c, c.label());
            }
            if self.view == View::History {
                ui.separator();
                for d in [3, 7, 14, 30] {
                    ui.selectable_value(&mut self.days, d, format!("{d}d"));
                }
            }
        });
        match self.view {
            View::Today => self.today(ui, cx, &node),
            View::History => self.history(ui, cx, &node),
        }
    }
}

impl Gp {
    fn node_picker(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        ui.horizontal(|ui| {
            if let Some(n) = &self.node {
                ui.label(RichText::new(n).heading().color(skin.text_strong));
            }
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("change node…")
                    .desired_width(180.0),
            );
            if resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !self.search.trim().is_empty()
            {
                self.node = Some(self.search.trim().to_ascii_uppercase());
                self.search.clear();
            }
            for fav in &cx.config.ui.favorite_nodes {
                if Some(fav) != self.node.as_ref()
                    && widgets::link(ui, skin, mt_core::hub_short(fav)).clicked()
                {
                    self.node = Some(fav.clone());
                }
            }
        });
        // Suggestions while typing.
        let needle = self.search.trim().to_ascii_uppercase();
        if needle.len() >= 2 {
            let board = cx.hub.peek(&cx.miso.lmp_board());
            let intraday = cx.hub.peek(&cx.miso.rt_intraday());
            let mut names: Vec<String> = board
                .data()
                .map(|b| b.rows.iter().map(|r| r.node.clone()).collect())
                .unwrap_or_default();
            if let Some(d) = intraday.data() {
                names.extend(d.node_names().iter().cloned());
            }
            names.sort();
            names.dedup();
            ui.horizontal_wrapped(|ui| {
                for n in names.iter().filter(|n| n.contains(&needle)).take(12) {
                    if widgets::link(ui, skin, n).clicked() {
                        self.node = Some(n.clone());
                        self.search.clear();
                    }
                }
            });
        }
    }

    fn today(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, node: &str) {
        let skin = cx.skin;
        let today = market_today();
        let intraday = cx.hub.watch(&cx.miso.rt_intraday());
        let da_today = cx
            .hub
            .watch(&cx.miso.day_report(DayReportKind::DaExPost, today));
        let da_tomorrow = cx.hub.watch(
            &cx.miso
                .day_report(DayReportKind::DaExPost, today + Duration::days(1)),
        );
        let rt: Vec<(NaiveDateTime, f64)> = intraday
            .data()
            .and_then(|d| d.series(node))
            .map(|s| self.component.intraday(&s))
            .unwrap_or_default();
        let da = report_points(da_today.data(), node, self.component, today);
        let da_next = report_points(
            da_tomorrow.data(),
            node,
            self.component,
            today + Duration::days(1),
        );

        ui.horizontal_wrapped(|ui| {
            let latest = rt.last().map(|(_, v)| *v);
            let sub = rt.last().map(|(t, _)| {
                RichText::new(format!("at {} EST", fmt::hm(*t))).color(skin.text_muted)
            });
            widgets::stat_tile(
                ui,
                skin,
                &format!("RT 5-min {}", self.component.label()),
                &fmt::price_opt(latest),
                sub,
            );
            let avg = |pts: &[(NaiveDateTime, f64)]| {
                (!pts.is_empty()).then(|| pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64)
            };
            widgets::stat_tile(ui, skin, "RT avg today", &fmt::price_opt(avg(&rt)), None);
            widgets::stat_tile(ui, skin, "DA avg today", &fmt::price_opt(avg(&da)), None);
            if !da_next.is_empty() {
                widgets::stat_tile(
                    ui,
                    skin,
                    "DA avg tomorrow",
                    &fmt::price_opt(avg(&da_next)),
                    None,
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                widgets::freshness(ui, skin, &intraday)
            });
        });

        if rt.is_empty() && da.is_empty() {
            if intraday.data().is_some() && da_today.data().is_some() {
                ui.label(
                    RichText::new(format!(
                        "No prices for {node}. Check the node name (LMP lists them)."
                    ))
                    .color(skin.warning),
                );
            } else {
                widgets::placeholder(ui, skin, intraday.error.as_ref().map(ToString::to_string));
            }
            return;
        }
        chart::time_plot(&format!("gp-today-{node}"), skin).show(ui, |plot| {
            chart::hourly_steps(plot, "DA ex-post", &da, skin.series(1));
            if !da_next.is_empty() {
                chart::forecast_steps(plot, "DA tomorrow", &da_next, skin.series(1));
            }
            plot.line(chart::line("RT 5-min", &rt, skin.series(0)));
        });
    }

    fn history(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, node: &str) {
        let skin = cx.skin;
        let today = market_today();
        let first = today - Duration::days(i64::from(self.days) - 1);
        let mut da = Vec::new();
        let mut rt = Vec::new();
        let mut pending = 0;
        let mut prelim_days = Vec::new();
        let mut day = first;
        while day <= today + Duration::days(1) {
            let d = cx
                .hub
                .watch(&cx.miso.day_report(DayReportKind::DaExPost, day));
            pending += usize::from(d.data.is_none() && d.error.is_none());
            da.extend(report_points(d.data(), node, self.component, day));
            if day < today {
                let r = cx.hub.watch(&cx.miso.rt_best_day(day));
                pending += usize::from(r.data.is_none() && r.error.is_none());
                if let Some(Some(report)) = r.data()
                    && report.kind == DayReportKind::RtPrelim
                {
                    prelim_days.push(day);
                }
                rt.extend(report_points(r.data(), node, self.component, day));
            }
            day += Duration::days(1);
        }
        // Today's RT comes from the five-minute feed, averaged to hours.
        if let Some(s) = cx
            .hub
            .watch(&cx.miso.rt_intraday())
            .data()
            .and_then(|d| d.series(node))
        {
            rt.extend(hourly_average(&self.component.intraday(&s)));
        }

        ui.horizontal_wrapped(|ui| {
            let stats = Stats::of(&da, &rt);
            widgets::stat_tile(ui, skin, "DA avg", &fmt::price_opt(stats.da_avg), None);
            widgets::stat_tile(ui, skin, "RT avg", &fmt::price_opt(stats.rt_avg), None);
            widgets::stat_tile(
                ui,
                skin,
                "RT − DA avg",
                &stats.dart_avg.map_or_else(|| fmt::DASH.into(), fmt::signed),
                Some(
                    RichText::new(format!("{} matched hours", stats.matched))
                        .color(skin.text_muted),
                ),
            );
            widgets::stat_tile(
                ui,
                skin,
                "RT max",
                &fmt::price_opt(stats.rt_max),
                Some(
                    RichText::new(format!(
                        "min {} · {} negative hours",
                        fmt::price_opt(stats.rt_min),
                        stats.rt_negative
                    ))
                    .color(skin.text_muted),
                ),
            );
        });
        if pending > 0 {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new(format!("Fetching {pending} daily reports…"))
                        .small()
                        .color(skin.text_muted),
                );
            });
        }
        if !prelim_days.is_empty() {
            ui.label(
                RichText::new(format!(
                    "RT is preliminary for {} day(s); final reports trail by about a week.",
                    prelim_days.len()
                ))
                .small()
                .color(skin.text_muted),
            );
        }
        chart::time_plot(&format!("gp-hist-{node}"), skin).show(ui, |plot| {
            chart::hourly_steps(plot, "DA ex-post", &da, skin.series(1));
            chart::hourly_steps(plot, "RT", &rt, skin.series(0));
        });
    }
}

fn report_points(
    report: Option<&Option<DayLmpReport>>,
    node: &str,
    component: Component,
    day: NaiveDate,
) -> Vec<(NaiveDateTime, f64)> {
    report
        .and_then(Option::as_ref)
        .and_then(|r| r.node(node))
        .map(|row| hourly_points(day, &component.hourly(row)))
        .unwrap_or_default()
}

/// Average five-minute points into hour-starting buckets.
fn hourly_average(pts: &[(NaiveDateTime, f64)]) -> Vec<(NaiveDateTime, f64)> {
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

#[derive(Default)]
struct Stats {
    da_avg: Option<f64>,
    rt_avg: Option<f64>,
    dart_avg: Option<f64>,
    matched: usize,
    rt_min: Option<f64>,
    rt_max: Option<f64>,
    rt_negative: usize,
}

impl Stats {
    fn of(da: &[(NaiveDateTime, f64)], rt: &[(NaiveDateTime, f64)]) -> Self {
        let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
        let da_vals: Vec<f64> = da.iter().map(|p| p.1).collect();
        let rt_vals: Vec<f64> = rt.iter().map(|p| p.1).collect();
        let da_by_hour: std::collections::HashMap<NaiveDateTime, f64> =
            da.iter().copied().collect();
        let spreads: Vec<f64> = rt
            .iter()
            .filter_map(|(t, v)| Some(v - da_by_hour.get(t)?))
            .collect();
        Self {
            da_avg: mean(&da_vals),
            rt_avg: mean(&rt_vals),
            dart_avg: mean(&spreads),
            matched: spreads.len(),
            rt_min: rt_vals.iter().copied().min_by(f64::total_cmp),
            rt_max: rt_vals.iter().copied().max_by(f64::total_cmp),
            rt_negative: rt_vals.iter().filter(|v| **v < 0.0).count(),
        }
    }
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
        let h = hourly_average(&pts);
        assert_eq!(h, vec![(t(0, 0), 15.0), (t(1, 0), 65.0)]);
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
}
