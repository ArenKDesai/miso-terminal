//! GP: graph a node's price. Today's five-minute RT against the DA ex-post
//! staircase, or N days of hourly DA against RT with summary statistics.

use chrono::Timelike;
use egui::{RichText, Ui};

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series::{self, Component, Stats};
use crate::widgets::csv;
use crate::widgets::node_picker::NodePicker;
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "GP",
    aliases: &["GRAPH", "CHART"],
    name: "Graph price",
    category: Category::Prices,
    usage: "GP <node> [days] [HEAT|DUR|5MIN]",
    description: "Price chart for one node: today's 5-minute RT vs DA, hourly DA vs RT over N days, or five-minute RT from the local archive, by component.",
    takes_node: true,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let node = args
        .first()
        .map(|n| n.trim().to_ascii_uppercase())
        .filter(|n| !n.is_empty());
    let days = parse_days(args.get(1))?;
    Ok(Box::new(Gp {
        node,
        days,
        picker: NodePicker::default(),
        // Asking for a number of days means asking for history.
        view: View::from_args(days, args.get(2)),
        component: Component::Lmp,
        heat: Heat::Rt,
        yesterday: false,
    }))
}

/// The longest window offered: about four years, what the local archive holds.
pub(crate) const MAX_DAYS: u32 = 1500;

/// Day-count buttons shared by the history views.
pub(crate) const DAY_CHOICES: [u32; 6] = [3, 7, 14, 30, 90, 365];

/// Parse an optional day count; 0 means "use the configured default".
pub(crate) fn parse_days(arg: Option<&String>) -> Result<u32, String> {
    match arg {
        Some(d) => Ok(d
            .parse::<u32>()
            .map_err(|_| format!("days must be a number, got {d:?}"))?
            .clamp(1, MAX_DAYS)),
        None => Ok(0),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Today,
    History,
    /// Five-minute RT over N days, from the local archive.
    FiveMinute,
    Heatmap,
    Duration,
}

impl View {
    /// A day count asks for history; a trailing `HEAT` or `DUR` picks that view.
    pub(crate) fn from_args(days: u32, flag: Option<&String>) -> Self {
        match flag.map(|f| f.to_ascii_uppercase()).as_deref() {
            Some("HEAT") => Self::Heatmap,
            Some("DUR") => Self::Duration,
            Some("5MIN") => Self::FiveMinute,
            _ if days > 0 => Self::History,
            _ => Self::Today,
        }
    }

    /// The route flag that reopens this view, if it needs one.
    pub(crate) fn flag(self) -> Option<&'static str> {
        match self {
            Self::Heatmap => Some("HEAT"),
            Self::Duration => Some("DUR"),
            Self::FiveMinute => Some("5MIN"),
            Self::Today | Self::History => None,
        }
    }
}

/// Which series the heatmap colours.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Heat {
    Rt,
    Da,
    Dart,
}

struct Gp {
    node: Option<String>,
    days: u32,
    picker: NodePicker,
    view: View,
    component: Component,
    heat: Heat,
    /// Also plot yesterday's five-minute RT and DA in the Today view.
    yesterday: bool,
}

impl Panel for Gp {
    fn title(&self) -> String {
        match &self.node {
            Some(n) => format!("GP {n}"),
            None => "GP".into(),
        }
    }

    fn route(&self) -> Route {
        let mut args: Vec<String> = self.node.iter().cloned().collect();
        if !args.is_empty() && (self.days > 0 || self.view != View::Today) {
            args.push(self.days.max(1).to_string());
            args.extend(self.view.flag().map(String::from));
        }
        Route::new("GP", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if self.days == 0 {
            self.days = cx.config.data.history_days.clamp(1, 90);
        }
        self.header(ui, cx);
        let Some(node) = self.node.clone() else {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Pick a node above, or type GP <node> on the command line.")
                    .color(skin.text_muted),
            );
            return;
        };
        view_controls(ui, &mut self.view, &mut self.component, &mut self.days);
        match self.view {
            View::Today => self.today(ui, cx, &node),
            View::History => self.history(ui, cx, &node),
            View::FiveMinute => {
                let f = series::node_five_minute(cx, &node, self.component, self.days);
                five_minute_view(ui, cx, &format!("gp-5min-{node}"), &f, &f.rt, &f.da);
            }
            View::Heatmap => self.heatmap(ui, cx, &node),
            View::Duration => {
                let h = series::node_history(cx, &node, self.component, self.days);
                history_notes(ui, cx, &h, || {
                    csv::to_csv(
                        &["hour_start_est", "da", "rt", "rt_minus_da"],
                        series::hourly_rows(&h.da, &h.rt),
                    )
                });
                duration_view(
                    ui,
                    cx,
                    &format!("gp-dur-{node}"),
                    [("DA ex-post", &h.da), ("RT", &h.rt)],
                );
            }
        }
    }
}

/// Today/History, component and day-count selectors (shared with SPRD).
pub(crate) fn view_controls(
    ui: &mut Ui,
    view: &mut View,
    component: &mut Component,
    days: &mut u32,
) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(view, View::Today, "Today · 5-min");
        ui.selectable_value(view, View::History, "History · hourly");
        ui.selectable_value(view, View::FiveMinute, "History · 5-min")
            .on_hover_text("Five-minute RT for past days, from prices saved while MISO Terminal runs (plus MISO's previous-day feed for yesterday)");
        ui.selectable_value(view, View::Heatmap, "Heatmap");
        ui.selectable_value(view, View::Duration, "Duration");
        ui.separator();
        for c in Component::ALL {
            ui.selectable_value(component, c, c.label());
        }
        if *view != View::Today {
            ui.separator();
            for d in DAY_CHOICES {
                ui.selectable_value(days, d, if d == 365 { "1y".into() } else { format!("{d}d") });
            }
        }
    });
}

impl Gp {
    fn header(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        // Wrap, so a long favourites row never widens the panel past its pane.
        ui.horizontal_wrapped(|ui| {
            if let Some(n) = &self.node {
                ui.label(RichText::new(n).heading().color(skin.text_strong));
                let watched = cx.config.ui.favorite_nodes.contains(n);
                let (label, cmd) = if watched {
                    ("★ Watching", AppCommand::RemoveFavorite(n.clone()))
                } else {
                    ("☆ Watch", AppCommand::AddFavorite(n.clone()))
                };
                if ui
                    .small_button(label)
                    .on_hover_text("Add to or remove from WL")
                    .clicked()
                {
                    cx.send(cmd);
                }
            }
            if let Some(n) = self.picker.show(ui, cx, "gp", "change node…") {
                self.node = Some(n);
            }
            for fav in &cx.config.ui.favorite_nodes {
                if Some(fav) != self.node.as_ref()
                    && widgets::link(ui, skin, mt_core::hub_short(fav)).clicked()
                {
                    self.node = Some(fav.clone());
                }
            }
        });
    }

    fn today(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, node: &str) {
        let skin = cx.skin;
        let t = series::node_today(cx, node, self.component);
        let intraday = cx.hub.peek(&cx.miso.rt_intraday());
        ui.checkbox(&mut self.yesterday, "+ yesterday")
            .on_hover_text(
                "Also show yesterday's five-minute RT and DA (one ~11 MB download per day)",
            );
        let (y_rt, y_da, y_loading) = if self.yesterday {
            let prev = cx.hub.watch(&cx.miso.rt_previous_day());
            let day = mt_core::time::market_today() - chrono::Duration::days(1);
            let da = cx
                .hub
                .watch(&cx.miso.day_report(mt_core::DayReportKind::DaExPost, day));
            let rt = prev
                .data()
                .and_then(|d| d.series(node))
                .map(|s| self.component.intraday(&s))
                .unwrap_or_default();
            (
                rt,
                series::report_points(da.data(), node, self.component, day),
                prev.loading,
            )
        } else {
            (Vec::new(), Vec::new(), false)
        };
        if y_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Fetching yesterday's five-minute prices…")
                        .small()
                        .color(skin.text_muted),
                );
            });
        }

        ui.horizontal_wrapped(|ui| {
            let sub = t.rt_5min.last().map(|(at, _)| {
                RichText::new(format!("at {} EST", fmt::hm(*at))).color(skin.text_muted)
            });
            widgets::stat_tile(
                ui,
                skin,
                &format!("RT 5-min {}", self.component.label()),
                &fmt::price_opt(t.rt_5min.last().map(|p| p.1)),
                sub,
            );
            let avg = |pts: &series::Points| series::mean(pts.iter().map(|p| p.1));
            widgets::stat_tile(
                ui,
                skin,
                "RT avg today",
                &fmt::price_opt(avg(&t.rt_5min)),
                None,
            );
            widgets::stat_tile(ui, skin, "DA avg today", &fmt::price_opt(avg(&t.da)), None);
            if !t.da_tomorrow.is_empty() {
                widgets::stat_tile(
                    ui,
                    skin,
                    "DA avg tomorrow",
                    &fmt::price_opt(avg(&t.da_tomorrow)),
                    None,
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                widgets::freshness(ui, skin, &intraday)
            });
        });

        if t.rt_5min.is_empty() && t.da.is_empty() {
            if t.loaded {
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
            // Yesterday shares today's colours and names, so it reads as one series.
            chart::hourly_steps(plot, "DA ex-post", &y_da, skin.series(1));
            plot.line(chart::line("RT 5-min", &y_rt, skin.series(0)));
            chart::hourly_steps(plot, "DA ex-post", &t.da, skin.series(1));
            if !t.da_tomorrow.is_empty() {
                chart::forecast_steps(plot, "DA tomorrow", &t.da_tomorrow, skin.series(1));
            }
            plot.line(chart::line("RT 5-min", &t.rt_5min, skin.series(0)));
        });
    }

    fn history(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, node: &str) {
        let skin = cx.skin;
        let h = series::node_history(cx, node, self.component, self.days);
        let stats = Stats::of(&h.da, &h.rt);
        ui.horizontal_wrapped(|ui| {
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
        history_notes(ui, cx, &h, || {
            csv::to_csv(
                &["hour_start_est", "da", "rt", "rt_minus_da"],
                series::hourly_rows(&h.da, &h.rt),
            )
        });
        chart::time_plot(&format!("gp-hist-{node}"), skin).show(ui, |plot| {
            chart::hourly_steps(plot, "DA ex-post", &h.da, skin.series(1));
            chart::hourly_steps(plot, "RT", &h.rt, skin.series(0));
        });
    }
}

impl Gp {
    fn heatmap(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, node: &str) {
        let h = series::node_history(cx, node, self.component, self.days);
        ui.horizontal(|ui| {
            widgets::label(ui, cx.skin, "Colour by");
            ui.selectable_value(&mut self.heat, Heat::Rt, "RT");
            ui.selectable_value(&mut self.heat, Heat::Da, "DA");
            ui.selectable_value(&mut self.heat, Heat::Dart, "RT − DA");
        });
        let levels = matches!(self.component, Component::Lmp | Component::Energy);
        let (points, zero_centred) = match self.heat {
            Heat::Rt => (h.rt.clone(), !levels),
            Heat::Da => (h.da.clone(), !levels),
            Heat::Dart => (series::subtract(&h.rt, &h.da), true),
        };
        history_notes(ui, cx, &h, || {
            csv::to_csv(
                &["hour_start_est", "da", "rt", "rt_minus_da"],
                series::hourly_rows(&h.da, &h.rt),
            )
        });
        widgets::heatmap::hour_day(ui, cx.skin, &points, zero_centred);
    }
}

/// Five-minute RT over N days against the hourly DA staircase, with notes on
/// which days the archive has. `rt` and `da` may be spreads. Shared with SPRD.
pub(crate) fn five_minute_view(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    id: &str,
    f: &series::FiveMinute,
    rt: &series::Points,
    da: &series::Points,
) {
    let skin = cx.skin;
    let rt_vals: Vec<f64> = rt.iter().map(|p| p.1).collect();
    // Compare like with like: DA over the hours that have five-minute RT.
    let rt_hours: std::collections::HashSet<_> = rt
        .iter()
        .filter_map(|(t, _)| t.date().and_hms_opt(t.hour(), 0, 0))
        .collect();
    let da_matched = series::mean(da.iter().filter(|(t, _)| rt_hours.contains(t)).map(|p| p.1));
    ui.horizontal_wrapped(|ui| {
        widgets::stat_tile(
            ui,
            skin,
            "RT avg",
            &fmt::price_opt(series::mean(rt_vals.iter().copied())),
            Some(RichText::new(format!("{} intervals", rt_vals.len())).color(skin.text_muted)),
        );
        widgets::stat_tile(
            ui,
            skin,
            "DA avg",
            &fmt::price_opt(da_matched),
            Some(RichText::new("same hours as RT").color(skin.text_muted)),
        );
        widgets::stat_tile(
            ui,
            skin,
            "RT max",
            &fmt::price_opt(rt_vals.iter().copied().max_by(f64::total_cmp)),
            Some(
                RichText::new(format!(
                    "min {}",
                    fmt::price_opt(rt_vals.iter().copied().min_by(f64::total_cmp))
                ))
                .color(skin.text_muted),
            ),
        );
        widgets::stat_tile(
            ui,
            skin,
            "RT std dev",
            &fmt::price_opt(series::std_dev(&rt_vals)),
            Some(
                RichText::new(format!(
                    "{} negative intervals",
                    rt_vals.iter().filter(|v| **v < 0.0).count()
                ))
                .color(skin.text_muted),
            ),
        );
    });
    ui.horizontal_wrapped(|ui| {
        if f.pending > 0 {
            ui.spinner();
        }
        let mut note = String::from(
            "Past days come from prices saved while MISO Terminal runs; yesterday also from MISO's previous-day feed.",
        );
        if f.partial > 0 {
            note += &format!(" {} day(s) only partly saved.", f.partial);
        }
        if !f.missing.is_empty() {
            let days: Vec<String> = f
                .missing
                .iter()
                .map(|d| d.format("%b %-d").to_string())
                .collect();
            note += &format!(" Not saved: {}.", days.join(", "));
        }
        ui.label(RichText::new(note).small().color(skin.text_muted));
        csv::copy_button(ui, skin, || {
            let da_at: std::collections::HashMap<_, _> = da.iter().copied().collect();
            csv::to_csv(
                &["interval_start_est", "rt", "da_hour"],
                rt.iter().map(|(t, v)| {
                    let hour = t.date().and_hms_opt(t.hour(), 0, 0).unwrap_or(*t);
                    vec![
                        t.format("%Y-%m-%d %H:%M").to_string(),
                        format!("{v:.2}"),
                        da_at
                            .get(&hour)
                            .map_or_else(String::new, |d: &f64| format!("{d:.2}")),
                    ]
                }),
            )
        });
    });
    if rt.is_empty() && da.is_empty() {
        if f.pending == 0 {
            ui.label(RichText::new("No prices saved for this span yet.").color(skin.warning));
        }
        return;
    }
    chart::time_plot(id, skin).show(ui, |plot| {
        chart::hourly_steps(plot, "DA ex-post", da, skin.series(1));
        chart::gapped_line(plot, "RT 5-min", rt, skin.series(0), 10.0 * 60.0);
    });
}

/// Price duration curves (share of hours at or above each price) for a DA and
/// an RT series, with RT percentile tiles. Shared with SPRD.
pub(crate) fn duration_view(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    id: &str,
    series: [(&str, &series::Points); 2],
) {
    let skin = cx.skin;
    let [(da_name, da), (rt_name, rt)] = series;
    let rt_curve = series::duration_curve(rt.iter().map(|p| p.1));
    ui.horizontal_wrapped(|ui| {
        // The curve is sorted high to low, so x = 10% is the 90th percentile.
        let at = |pct: f64| rt_curve.iter().find(|p| p[0] >= pct).map(|p| p[1]);
        let signed = |v: Option<f64>| v.map_or_else(|| fmt::DASH.into(), fmt::price);
        widgets::stat_tile(
            ui,
            skin,
            &format!("{rt_name} top 10% ≥"),
            &signed(at(10.0)),
            None,
        );
        widgets::stat_tile(
            ui,
            skin,
            &format!("{rt_name} median"),
            &signed(at(50.0)),
            None,
        );
        widgets::stat_tile(
            ui,
            skin,
            &format!("{rt_name} bottom 10% ≤"),
            &signed(at(90.0)),
            None,
        );
    });
    // Frame the 1st-99th percentile so one scarcity hour does not flatten the
    // curve; double-click the plot to see the full range.
    let all: Vec<f64> = da
        .iter()
        .chain(rt.iter())
        .map(|p| p.1)
        .filter(|v| v.is_finite())
        .collect();
    let mut plot = chart::duration_plot(id, skin);
    if let (Some(lo), Some(hi)) = (
        series::percentile(&all, 0.01),
        series::percentile(&all, 0.99),
    ) {
        let pad = ((hi - lo) * 0.08).max(1.0);
        plot = plot.default_y_bounds(lo - pad, hi + pad);
    }
    plot.show(ui, |plot| {
        let line = |name: &str, pts: Vec<[f64; 2]>, color| {
            egui_plot::Line::new(name, egui_plot::PlotPoints::from(pts))
                .color(color)
                .width(1.6)
        };
        plot.line(line(
            da_name,
            series::duration_curve(da.iter().map(|p| p.1)),
            skin.series(1),
        ));
        plot.line(line(rt_name, rt_curve.clone(), skin.series(0)));
    });
}

/// Loading progress, the preliminary-RT note and a Copy CSV button.
pub(crate) fn history_notes(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    h: &series::History,
    make_csv: impl FnOnce() -> String,
) {
    let skin = cx.skin;
    ui.horizontal(|ui| {
        csv::copy_button(ui, skin, make_csv);
        if h.pending > 0 {
            ui.spinner();
            ui.label(
                RichText::new(format!("Fetching {} daily reports…", h.pending))
                    .small()
                    .color(skin.text_muted),
            );
        }
        if h.prelim_days > 0 {
            ui.label(
                RichText::new(format!(
                    "RT is preliminary for {} day(s); final reports trail by about a week.",
                    h.prelim_days
                ))
                .small()
                .color(skin.text_muted),
            );
        }
        if h.archived_days > 0 {
            ui.label(
                RichText::new(format!(
                    "{} day(s) from the local archive.",
                    h.archived_days
                ))
                .small()
                .color(skin.text_muted),
            );
        }
        if h.capped {
            ui.label(
                RichText::new(format!(
                    "Showing {} days: longer history needs the local archive \
                     (uv run tools/export_history.py).",
                    series::DOWNLOAD_DAYS
                ))
                .small()
                .color(skin.warning),
            );
        }
    });
}

/// A GP Today panel with the yesterday overlay on, for the smoke tests.
#[cfg(test)]
pub(crate) fn with_yesterday(node: &str) -> Box<dyn Panel> {
    Box::new(Gp {
        node: Some(node.to_owned()),
        days: 0,
        picker: NodePicker::default(),
        view: View::Today,
        component: Component::Lmp,
        heat: Heat::Rt,
        yesterday: true,
    })
}
