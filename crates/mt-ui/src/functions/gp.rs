//! GP: graph a node's price. Today's five-minute RT against the DA ex-post
//! staircase, or N days of hourly DA against RT with summary statistics.

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
    usage: "GP <node> [days] [HEAT|DUR]",
    description: "Price chart for one node: today's 5-minute RT vs DA, or hourly DA vs RT over N days, by component.",
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
    }))
}

/// Parse an optional day count; 0 means "use the configured default".
pub(crate) fn parse_days(arg: Option<&String>) -> Result<u32, String> {
    match arg {
        Some(d) => Ok(d
            .parse::<u32>()
            .map_err(|_| format!("days must be a number, got {d:?}"))?
            .clamp(1, 90)),
        None => Ok(0),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Today,
    History,
    Heatmap,
    Duration,
}

impl View {
    /// A day count asks for history; a trailing `HEAT` or `DUR` picks that view.
    pub(crate) fn from_args(days: u32, flag: Option<&String>) -> Self {
        match flag.map(|f| f.to_ascii_uppercase()).as_deref() {
            Some("HEAT") => Self::Heatmap,
            Some("DUR") => Self::Duration,
            _ if days > 0 => Self::History,
            _ => Self::Today,
        }
    }

    /// The route flag that reopens this view, if it needs one.
    pub(crate) fn flag(self) -> Option<&'static str> {
        match self {
            Self::Heatmap => Some("HEAT"),
            Self::Duration => Some("DUR"),
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
        ui.selectable_value(view, View::Heatmap, "Heatmap");
        ui.selectable_value(view, View::Duration, "Duration");
        ui.separator();
        for c in Component::ALL {
            ui.selectable_value(component, c, c.label());
        }
        if *view != View::Today {
            ui.separator();
            for d in [3, 7, 14, 30] {
                ui.selectable_value(days, d, format!("{d}d"));
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

    fn today(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, node: &str) {
        let skin = cx.skin;
        let t = series::node_today(cx, node, self.component);
        let intraday = cx.hub.peek(&cx.miso.rt_intraday());

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
    });
}
