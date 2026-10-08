//! OUT: generation outages for the five days either side of today.

use chrono::NaiveTime;
use egui::{Grid, RichText, Ui};
use egui_plot::{Bar, BarChart};
use mt_core::time::{chart_x, market_today};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "OUT",
    aliases: &["OUTAGES"],
    name: "Generation outages",
    category: Category::Grid,
    usage: "OUT",
    description: "Planned, unplanned, forced and derated generation outages for ±5 days.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Outages))
}

struct Outages;

impl Panel for Outages {
    fn title(&self) -> String {
        "OUT".into()
    }

    fn route(&self) -> Route {
        Route::code("OUT")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let snap = cx.hub.watch(&cx.miso.outages());
        widgets::title_bar(ui, skin, "Generation outages · MW", |ui| {
            widgets::freshness(ui, skin, &snap);
        });
        widgets::with_data(ui, skin, &snap, |ui, o| {
            ui.label(RichText::new(&o.headline).color(skin.text_muted));
            let today = market_today();
            type Pick = fn(&mt_core::OutageDay) -> f64;
            let kinds: [(&str, Pick); 4] = [
                ("Planned", |d| d.planned),
                ("Unplanned", |d| d.unplanned),
                ("Forced", |d| d.forced),
                ("Derated", |d| d.derated),
            ];
            let day_secs = 86_400.0;
            let width = day_secs / 5.0;
            ui.allocate_ui(
                egui::vec2(
                    ui.available_width(),
                    (ui.available_height() * 0.6).max(160.0),
                ),
                |ui| {
                    chart::time_plot("outages", skin).show(ui, |plot| {
                        for (i, (name, pick)) in kinds.iter().enumerate() {
                            let bars = o
                                .days
                                .iter()
                                .map(|d| {
                                    let noon = chart_x(d.day.and_time(
                                        NaiveTime::from_hms_opt(12, 0, 0).unwrap_or_default(),
                                    ));
                                    Bar::new(noon + (i as f64 - 1.5) * width, pick(d))
                                        .width(width * 0.9)
                                })
                                .collect();
                            plot.bar_chart(BarChart::new(*name, bars).color(skin.series(i)));
                        }
                    });
                },
            );
            Grid::new("outages-table")
                .striped(true)
                .num_columns(5)
                .spacing([16.0, 3.0])
                .show(ui, |ui| {
                    widgets::label(ui, skin, "Day");
                    for (name, _) in &kinds {
                        widgets::label(ui, skin, name);
                    }
                    ui.end_row();
                    for d in &o.days {
                        let label = d.day.format("%a %b %d").to_string();
                        let text = if d.day == today {
                            RichText::new(label + " (today)").strong()
                        } else {
                            RichText::new(label)
                        };
                        ui.label(text);
                        for (_, pick) in &kinds {
                            ui.label(fmt::mw(pick(d)));
                        }
                        ui.end_row();
                    }
                });
        });
    }
}
