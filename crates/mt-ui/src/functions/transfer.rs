//! RDT: the North-South regional directional transfer against its limits.

use chrono::NaiveDateTime;
use egui::{RichText, Ui};
use egui_plot::LineStyle;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "RDT",
    aliases: &["TRANSFER", "NS"],
    name: "Regional transfer",
    category: Category::Grid,
    usage: "RDT",
    description: "North-South regional directional transfer over the last day against its limits, with utilisation.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Transfer))
}

struct Transfer;

impl Panel for Transfer {
    fn title(&self) -> String {
        "RDT".into()
    }

    fn route(&self) -> Route {
        Route::code("RDT")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let snap = cx.hub.watch(&cx.miso.regional_transfer());
        widgets::title_bar(ui, skin, "Regional directional transfer", |ui| {
            widgets::freshness(ui, skin, &snap);
        });
        ui.label(
            RichText::new("Positive = South → North, negative = North → South.")
                .small()
                .color(skin.text_muted),
        );
        widgets::with_data(ui, skin, &snap, |ui, r| {
            let Some(last) = r.points.iter().rev().find(|p| p.flow.is_some()) else {
                ui.label(RichText::new("No transfer data yet.").color(skin.text_muted));
                return;
            };
            ui.horizontal_wrapped(|ui| {
                let flow = last.flow.unwrap_or(0.0);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Flow · MW",
                    &fmt::mw_signed(flow),
                    Some(
                        RichText::new(format!(
                            "{} at {} EST",
                            if flow >= 0.0 { "S → N" } else { "N → S" },
                            fmt::hm(last.time)
                        ))
                        .color(skin.text_muted),
                    ),
                );
                if let Some(u) = last.utilization() {
                    let color = match u {
                        u if u >= 0.95 => skin.negative,
                        u if u >= 0.8 => skin.warning,
                        _ => skin.text_muted,
                    };
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Of limit",
                        &fmt::pct(u * 100.0),
                        Some(RichText::new("in the direction of flow").color(color)),
                    );
                }
                let peak = r
                    .points
                    .iter()
                    .filter_map(|p| Some((p.time, p.utilization()?)))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((t, u)) = peak {
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Peak, last day",
                        &fmt::pct(u * 100.0),
                        Some(
                            RichText::new(format!("at {}", fmt::day_hm(t))).color(skin.text_muted),
                        ),
                    );
                }
            });
            let series =
                |f: fn(&mt_core::TransferPoint) -> Option<f64>| -> Vec<(NaiveDateTime, f64)> {
                    r.points
                        .iter()
                        .filter_map(|p| Some((p.time, f(p)?)))
                        .collect()
                };
            chart::time_plot("rdt", skin).show(ui, |plot| {
                let limit = |v: Vec<(NaiveDateTime, f64)>, name: &str| {
                    chart::line(name, &v, skin.negative)
                        .style(LineStyle::dashed_dense())
                        .width(1.0)
                };
                plot.line(limit(series(|p| p.south_north_limit), "S → N limit"));
                plot.line(limit(series(|p| p.north_south_limit), "N → S limit"));
                plot.line(chart::line("Raw", &series(|p| p.raw), skin.series(2)).width(1.0));
                plot.line(chart::line(
                    "Flow (UDS)",
                    &series(|p| p.flow),
                    skin.series(0),
                ));
            });
        });
    }
}
