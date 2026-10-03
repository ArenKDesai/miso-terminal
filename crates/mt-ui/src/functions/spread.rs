//! SPRD: the price spread between two nodes (A − B), today at five minutes or
//! hourly over N days, by component. Congestion spreads are the FTR view.

use egui::{RichText, Ui};
use egui_plot::HLine;

use super::gp::{Heat, View, five_minute_view, history_notes, parse_days, view_controls};
use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series::{self, Component, Stats};
use crate::widgets::csv;
use crate::widgets::node_picker::NodePicker;
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "SPRD",
    aliases: &["SPREAD", "BASIS"],
    name: "Node spread",
    category: Category::Prices,
    usage: "SPRD <node A> <node B> [days] [HEAT|DUR|5MIN]",
    description: "A − B price spread between two nodes: today at 5 minutes, or hourly DA and RT spreads over N days.",
    takes_node: true,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let node = |i: usize| {
        args.get(i)
            .map(|n| n.trim().to_ascii_uppercase())
            .filter(|n| !n.is_empty())
    };
    let days = parse_days(args.get(2))?;
    Ok(Box::new(Spread {
        a: node(0),
        b: node(1),
        days,
        view: View::from_args(days, args.get(3)),
        component: Component::Lmp,
        pick_a: NodePicker::default(),
        pick_b: NodePicker::default(),
        heat: Heat::Rt,
    }))
}

struct Spread {
    a: Option<String>,
    b: Option<String>,
    days: u32,
    view: View,
    component: Component,
    pick_a: NodePicker,
    pick_b: NodePicker,
    heat: Heat,
}

impl Panel for Spread {
    fn title(&self) -> String {
        match (&self.a, &self.b) {
            (Some(a), Some(b)) => {
                format!("SPRD {} − {}", mt_core::hub_short(a), mt_core::hub_short(b))
            }
            _ => "SPRD".into(),
        }
    }

    fn route(&self) -> Route {
        let mut args: Vec<String> = Vec::new();
        if let Some(a) = &self.a {
            args.push(a.clone());
            if let Some(b) = &self.b {
                args.push(b.clone());
                if self.days > 0 || self.view != View::Today {
                    args.push(self.days.max(1).to_string());
                    args.extend(self.view.flag().map(String::from));
                }
            }
        }
        Route::new("SPRD", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if self.days == 0 {
            self.days = cx.config.data.history_days.clamp(1, 90);
        }
        ui.horizontal(|ui| {
            for (label, node, picker, id) in [
                ("A", &mut self.a, &mut self.pick_a, "sprd-a"),
                ("B", &mut self.b, &mut self.pick_b, "sprd-b"),
            ] {
                widgets::label(ui, skin, label);
                if let Some(n) = node.as_ref() {
                    ui.label(RichText::new(n).heading().color(skin.text_strong));
                }
                if let Some(n) = picker.show(ui, cx, id, "node…") {
                    *node = Some(n);
                }
                ui.add_space(8.0);
            }
            if ui.small_button("⇄ swap").clicked() {
                std::mem::swap(&mut self.a, &mut self.b);
            }
        });
        let (Some(a), Some(b)) = (self.a.clone(), self.b.clone()) else {
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Pick two nodes, or type SPRD <A> <B>, e.g. SPRD MINN.HUB ILLINOIS.HUB.",
                )
                .color(skin.text_muted),
            );
            return;
        };
        view_controls(ui, &mut self.view, &mut self.component, &mut self.days);
        match self.view {
            View::Today => self.today(ui, cx, &a, &b),
            View::History => self.history(ui, cx, &a, &b),
            View::FiveMinute => {
                let fa = series::node_five_minute(cx, &a, self.component, self.days);
                let fb = series::node_five_minute(cx, &b, self.component, self.days);
                five_minute_view(
                    ui,
                    cx,
                    &format!("sprd-5min-{a}-{b}"),
                    &fa,
                    &series::subtract(&fa.rt, &fb.rt),
                    &series::subtract(&fa.da, &fb.da),
                );
            }
            View::Duration => {
                let (ha, hb) = (
                    series::node_history(cx, &a, self.component, self.days),
                    series::node_history(cx, &b, self.component, self.days),
                );
                let (da, rt) = (
                    series::subtract(&ha.da, &hb.da),
                    series::subtract(&ha.rt, &hb.rt),
                );
                super::gp::duration_view(
                    ui,
                    cx,
                    &format!("sprd-dur-{a}-{b}"),
                    [("DA spread", &da), ("RT spread", &rt)],
                );
            }
            View::Heatmap => {
                ui.horizontal(|ui| {
                    widgets::label(ui, cx.skin, "Colour by");
                    ui.selectable_value(&mut self.heat, Heat::Rt, "RT spread");
                    ui.selectable_value(&mut self.heat, Heat::Da, "DA spread");
                });
                let (ha, hb) = (
                    series::node_history(cx, &a, self.component, self.days),
                    series::node_history(cx, &b, self.component, self.days),
                );
                let pts = match self.heat {
                    Heat::Da => series::subtract(&ha.da, &hb.da),
                    _ => series::subtract(&ha.rt, &hb.rt),
                };
                widgets::heatmap::hour_day(ui, cx.skin, &pts, true);
            }
        }
    }
}

impl Spread {
    fn today(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, a: &str, b: &str) {
        let skin = cx.skin;
        let (ta, tb) = (
            series::node_today(cx, a, self.component),
            series::node_today(cx, b, self.component),
        );
        let rt = series::subtract(&ta.rt_5min, &tb.rt_5min);
        let da = series::subtract(&ta.da, &tb.da);
        ui.horizontal_wrapped(|ui| {
            let now = rt.last().map(|p| p.1);
            widgets::stat_tile(
                ui,
                skin,
                &format!("RT 5-min {} spread", self.component.label()),
                &now.map_or_else(|| fmt::DASH.into(), fmt::signed),
                rt.last().map(|(t, _)| {
                    RichText::new(format!("at {} EST", fmt::hm(*t))).color(skin.text_muted)
                }),
            );
            let avg = |p: &series::Points| series::mean(p.iter().map(|x| x.1));
            for (label, v) in [("RT avg today", avg(&rt)), ("DA avg today", avg(&da))] {
                widgets::stat_tile(
                    ui,
                    skin,
                    label,
                    &v.map_or_else(|| fmt::DASH.into(), fmt::signed),
                    None,
                );
            }
        });
        if rt.is_empty() && da.is_empty() {
            if ta.loaded {
                ui.label(
                    RichText::new(format!("No overlapping prices for {a} and {b}."))
                        .color(skin.warning),
                );
            } else {
                widgets::placeholder(ui, skin, None);
            }
            return;
        }
        chart::time_plot(&format!("sprd-today-{a}-{b}"), skin).show(ui, |plot| {
            plot.hline(HLine::new("", 0.0).color(skin.border_strong).width(1.0));
            chart::hourly_steps(plot, "DA spread", &da, skin.series(1));
            plot.line(chart::line("RT 5-min spread", &rt, skin.series(0)));
        });
    }

    fn history(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, a: &str, b: &str) {
        let skin = cx.skin;
        let (ha, hb) = (
            series::node_history(cx, a, self.component, self.days),
            series::node_history(cx, b, self.component, self.days),
        );
        let da = series::subtract(&ha.da, &hb.da);
        let rt = series::subtract(&ha.rt, &hb.rt);
        let stats = Stats::of(&da, &rt);
        let positive = rt.iter().filter(|p| p.1 > 0.0).count();
        ui.horizontal_wrapped(|ui| {
            let signed = |v: Option<f64>| v.map_or_else(|| fmt::DASH.into(), fmt::signed);
            widgets::stat_tile(ui, skin, "DA spread avg", &signed(stats.da_avg), None);
            widgets::stat_tile(ui, skin, "RT spread avg", &signed(stats.rt_avg), None);
            widgets::stat_tile(
                ui,
                skin,
                "RT spread range",
                &signed(stats.rt_max),
                Some(RichText::new(format!("min {}", signed(stats.rt_min))).color(skin.text_muted)),
            );
            if !rt.is_empty() {
                widgets::stat_tile(
                    ui,
                    skin,
                    "Hours A > B (RT)",
                    &fmt::pct(positive as f64 / rt.len() as f64 * 100.0),
                    Some(RichText::new(format!("of {} hours", rt.len())).color(skin.text_muted)),
                );
            }
        });
        let pending = series::History {
            da: Vec::new(),
            rt: Vec::new(),
            pending: ha.pending + hb.pending,
            prelim_days: ha.prelim_days.max(hb.prelim_days),
            archived_days: ha.archived_days.min(hb.archived_days),
            capped: ha.capped || hb.capped,
        };
        history_notes(ui, cx, &pending, || {
            csv::to_csv(
                &["hour_start_est", "da_spread", "rt_spread", "rt_minus_da"],
                series::hourly_rows(&da, &rt),
            )
        });
        chart::time_plot(&format!("sprd-hist-{a}-{b}"), skin).show(ui, |plot| {
            plot.hline(HLine::new("", 0.0).color(skin.border_strong).width(1.0));
            chart::hourly_steps(plot, "DA spread", &da, skin.series(1));
            chart::hourly_steps(plot, "RT spread", &rt, skin.series(0));
        });
    }
}
