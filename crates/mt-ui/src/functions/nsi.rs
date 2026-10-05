//! NSI: net scheduled interchange with each neighbour.

use chrono::NaiveDateTime;
use egui::{Grid, RichText, Ui};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "NSI",
    aliases: &["INTERCHANGE", "IMPORTS"],
    name: "Interchange",
    category: Category::Grid,
    usage: "NSI",
    description: "Net scheduled interchange by neighbouring balancing authority, now and over the last day.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Nsi { by_ba: false }))
}

struct Nsi {
    by_ba: bool,
}

impl Panel for Nsi {
    fn title(&self) -> String {
        "NSI".into()
    }

    fn route(&self) -> Route {
        Route::code("NSI")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let now = cx.hub.watch(&cx.miso.interchange());
        let hist = cx.hub.watch(&cx.miso.interchange_history());
        widgets::title_bar(ui, skin, "Net scheduled interchange", |ui| {
            widgets::freshness(ui, skin, &now)
        });
        ui.label(
            RichText::new("Positive = MISO exporting, negative = importing.")
                .small()
                .color(skin.text_muted),
        );

        // Scheduled against metered: the gap is inadvertent interchange.
        let actual = cx.hub.watch(&cx.miso.actual_interchange());
        ui.horizontal_wrapped(|ui| {
            let scheduled = now.data().and_then(|n| n.net());
            let metered = actual.data().and_then(|a| a.mw);
            let mw = |v: Option<f64>| v.map_or_else(|| fmt::DASH.into(), fmt::mw_signed);
            widgets::stat_tile(ui, skin, "Scheduled (NSI) · MW", &mw(scheduled), None);
            widgets::stat_tile(
                ui,
                skin,
                "Actual (NAI) · MW",
                &mw(metered),
                actual.data().and_then(|a| a.time).map(|t| {
                    RichText::new(format!("metered at {} EST", fmt::hm(t))).color(skin.text_muted)
                }),
            );
            if let (Some(s), Some(m)) = (scheduled, metered) {
                widgets::stat_tile(
                    ui,
                    skin,
                    "Actual − scheduled",
                    &fmt::mw_signed(m - s),
                    Some(RichText::new("inadvertent flow").color(skin.text_muted)),
                );
            }
        });

        widgets::with_data(ui, skin, &now, |ui, n| {
            Grid::new("nsi-now")
                .striped(true)
                .num_columns(3)
                .spacing([16.0, 3.0])
                .show(ui, |ui| {
                    widgets::label(ui, skin, "Neighbour");
                    widgets::label(ui, skin, "MW");
                    widgets::label(ui, skin, "");
                    ui.end_row();
                    let mut rows: Vec<_> = n.neighbours().collect();
                    rows.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
                    for (ba, mw) in rows {
                        ui.label(ba);
                        ui.label(RichText::new(fmt::mw_signed(*mw)).color(skin.delta(*mw)));
                        ui.label(
                            RichText::new(if *mw < 0.0 { "import" } else { "export" })
                                .small()
                                .color(skin.text_muted),
                        );
                        ui.end_row();
                    }
                    if let Some(net) = n.net() {
                        ui.label(RichText::new("MISO net").strong());
                        ui.label(
                            RichText::new(fmt::mw_signed(net))
                                .strong()
                                .color(skin.delta(net)),
                        );
                        ui.end_row();
                    }
                });
        });

        ui.horizontal(|ui| {
            widgets::label(ui, skin, "Five-minute history");
            ui.selectable_value(&mut self.by_ba, false, "Net");
            ui.selectable_value(&mut self.by_ba, true, "By neighbour");
        });
        widgets::with_data(ui, skin, &hist, |ui, h| {
            let series = |ba: &str| -> Vec<(NaiveDateTime, f64)> {
                h.points
                    .iter()
                    .filter_map(|p| Some((p.time?, p.by_ba.iter().find(|(b, _)| b == ba)?.1)))
                    .collect()
            };
            let bas: Vec<String> = h
                .points
                .last()
                .map(|p| {
                    p.by_ba
                        .iter()
                        .map(|(b, _)| b.clone())
                        .filter(|b| b != "MISO")
                        .collect()
                })
                .unwrap_or_default();
            chart::time_plot("nsi-hist", skin).show(ui, |plot| {
                if self.by_ba {
                    for (i, ba) in bas.iter().enumerate() {
                        plot.line(chart::line(ba, &series(ba), skin.series(i)));
                    }
                } else {
                    plot.line(chart::line("MISO net", &series("MISO"), skin.series(0)).fill(0.0));
                }
            });
        });
    }
}
