//! CAP: committed capacity against demand (MISO's supply/demand display).

use chrono::NaiveDateTime;
use egui::{RichText, Ui};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "CAP",
    aliases: &["CSAT", "HEADROOM"],
    name: "Capacity & headroom",
    category: Category::Grid,
    usage: "CAP",
    description: "Committed capacity vs demand through the day, with forecasts and available capacity.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Capacity))
}

struct Capacity;

impl Panel for Capacity {
    fn title(&self) -> String {
        "CAP".into()
    }

    fn route(&self) -> Route {
        Route::code("CAP")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let cap = cx.hub.watch(&cx.miso.capacity());
        widgets::title_bar(ui, skin, "Capacity & headroom", |ui| {
            widgets::freshness(ui, skin, &cap)
        });
        widgets::with_data(ui, skin, &cap, |ui, c| {
            let series =
                |f: fn(&mt_core::CapacityPoint) -> Option<f64>| -> Vec<(NaiveDateTime, f64)> {
                    c.points
                        .iter()
                        .filter_map(|p| Some((p.time, f(p)?)))
                        .collect()
                };
            let demand = series(|p| p.demand);
            let committed = series(|p| p.committed);
            let demand_f = series(|p| p.demand_forecast);
            let committed_f = series(|p| p.committed_forecast);
            let available = series(|p| p.available);

            ui.horizontal_wrapped(|ui| {
                let last = c
                    .points
                    .iter()
                    .rev()
                    .find(|p| p.demand.is_some() && p.committed.is_some());
                if let Some(p) = last {
                    let (d, k) = (p.demand.unwrap_or(0.0), p.committed.unwrap_or(0.0));
                    widgets::stat_tile(ui, skin, "Demand · MW", &fmt::mw(d), None);
                    widgets::stat_tile(ui, skin, "Committed · MW", &fmt::mw(k), None);
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Headroom · MW",
                        &fmt::mw(k - d),
                        Some(
                            RichText::new(format!("at {} EST", fmt::hm(p.time)))
                                .color(skin.text_muted),
                        ),
                    );
                }
                let tightest = c
                    .points
                    .iter()
                    .filter_map(|p| Some((p.time, p.committed_forecast? - p.demand_forecast?)))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((t, h)) = tightest {
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Tightest forecast",
                        &fmt::mw(h),
                        Some(RichText::new(format!("at {} EST", fmt::hm(t))).color(
                            if h < 2000.0 {
                                skin.warning
                            } else {
                                skin.text_muted
                            },
                        )),
                    );
                }
            });
            chart::time_plot("cap-today", skin).show(ui, |plot| {
                plot.line(chart::line("Committed", &committed, skin.series(0)));
                plot.line(chart::line("Demand", &demand, skin.series(1)));
                plot.line(
                    chart::line("Committed (fcst)", &committed_f, skin.series(0))
                        .style(egui_plot::LineStyle::dashed_dense()),
                );
                plot.line(
                    chart::line("Demand (fcst)", &demand_f, skin.series(1))
                        .style(egui_plot::LineStyle::dashed_dense()),
                );
                plot.line(
                    chart::line("Available", &available, skin.series(2))
                        .style(egui_plot::LineStyle::dotted_loose()),
                );
            });
        });
    }
}
