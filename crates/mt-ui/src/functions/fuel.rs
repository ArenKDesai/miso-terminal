//! FUEL: generation by fuel, now and through the day.

use egui::{Grid, RichText, Ui};
use egui_plot::FilledArea;
use mt_core::time::chart_x;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "FUEL",
    aliases: &["MIX", "GEN"],
    name: "Fuel mix",
    category: Category::Grid,
    usage: "FUEL",
    description: "Generation by fuel for the current interval and as a stacked chart for the day so far.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Fuel { stacked: true }))
}

struct Fuel {
    stacked: bool,
}

impl Panel for Fuel {
    fn title(&self) -> String {
        "FUEL".into()
    }

    fn route(&self) -> Route {
        Route::code("FUEL")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let now = cx.hub.watch(&cx.miso.fuel_mix());
        let today = cx.hub.watch(&cx.miso.fuel_mix_today());

        widgets::title_bar(ui, skin, "Generation by fuel", |ui| {
            widgets::freshness(ui, skin, &now)
        });
        widgets::with_data(ui, skin, &now, |ui, mix| {
            let total = mix.total();
            let mut fuels = mix.fuels.clone();
            fuels.sort_by(|a, b| b.1.total_cmp(&a.1));
            Grid::new("fuel-now")
                .striped(true)
                .num_columns(4)
                .spacing([16.0, 3.0])
                .show(ui, |ui| {
                    for h in ["", "Fuel", "MW", "Share"] {
                        widgets::label(ui, skin, h);
                    }
                    ui.end_row();
                    for (cat, mw) in &fuels {
                        widgets::lamp(ui, skin.fuel(cat));
                        ui.label(cat);
                        ui.label(fmt::mw(*mw));
                        ui.label(
                            RichText::new(fmt::pct(mw / total * 100.0)).color(skin.text_muted),
                        );
                        ui.end_row();
                    }
                    ui.label("");
                    ui.label(RichText::new("Total").strong());
                    ui.label(RichText::new(fmt::mw(total)).strong());
                    ui.end_row();
                });
        });

        ui.horizontal(|ui| {
            widgets::label(ui, skin, "Today");
            ui.selectable_value(&mut self.stacked, true, "Stacked");
            ui.selectable_value(&mut self.stacked, false, "Lines");
        });
        widgets::with_data(ui, skin, &today, |ui, hist| {
            // Biggest fuels at the bottom of the stack.
            let mut cats = hist.categories();
            let avg = |c: &str| {
                let s = hist.series(c);
                s.iter().map(|p| p.1).sum::<f64>() / s.len().max(1) as f64
            };
            cats.sort_by(|a, b| avg(b).total_cmp(&avg(a)));
            let times: Vec<_> = hist.intervals.iter().filter_map(|m| m.interval).collect();
            let xs: Vec<f64> = times.iter().map(|t| chart_x(*t)).collect();

            // The table above is the key (its lamps match the areas).
            chart::time_plot_bare("fuel-today", skin).show(ui, |plot| {
                if self.stacked {
                    let mut base = vec![0.0; xs.len()];
                    for cat in &cats {
                        let top: Vec<f64> = hist
                            .intervals
                            .iter()
                            .filter(|m| m.interval.is_some())
                            .zip(&base)
                            .map(|(m, b)| {
                                b + m
                                    .fuels
                                    .iter()
                                    .find(|(c, _)| c == cat)
                                    .map_or(0.0, |(_, v)| v.max(0.0))
                            })
                            .collect();
                        let color = skin.fuel(cat);
                        plot.add(
                            FilledArea::new(cat.as_str(), &xs, &base, &top)
                                .fill_color(color.gamma_multiply(0.85)),
                        );
                        base = top;
                    }
                } else {
                    for cat in &cats {
                        plot.line(chart::line(cat, &hist.series(cat), skin.fuel(cat)));
                    }
                }
            });
        });
    }
}
