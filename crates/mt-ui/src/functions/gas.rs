//! GAS: Henry Hub natural gas, the fuel on the margin in most MISO hours, and
//! what the day-ahead hub prices imply against it: the market heat rate and
//! the spark spread.

use chrono::{Datelike, Duration, NaiveDateTime};
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::time::market_today;
use mt_core::{DayReportKind, hub_short, implied_heat_rate, spark_spread};

use super::dam::{blocks, hub_days};
use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series::Component;
use crate::widgets::table::num_cell;
use crate::widgets::{self, chart, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "GAS",
    aliases: &["HH", "HENRY", "FUELPRICE"],
    name: "Natural gas",
    category: Category::Prices,
    usage: "GAS",
    description: "Henry Hub gas spot (EIA) with recent change, and the market heat rate and spark spread each hub's day-ahead on-peak price implies.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Gas {
        days: 365,
        heat_rate: 7.0,
    }))
}

struct Gas {
    /// Chart window, days.
    days: i64,
    /// Reference heat rate for the spark spread, MMBtu/MWh (7 is a modern
    /// combined-cycle unit).
    heat_rate: f64,
}

impl Panel for Gas {
    fn title(&self) -> String {
        "GAS".into()
    }

    fn route(&self) -> Route {
        Route::code("GAS")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let hh = cx.hub.watch(&cx.eia.henry_hub());
        widgets::title_bar(ui, skin, "Natural gas · Henry Hub", |ui| {
            widgets::freshness(ui, skin, &hh)
        });
        let Some(spot) = hh.data() else {
            widgets::placeholder(ui, skin, hh.error.as_ref().map(ToString::to_string));
            return;
        };
        let Some((last_day, last)) = spot.latest() else {
            ui.label(RichText::new("No prices in EIA's file.").color(skin.warning));
            return;
        };
        let week = spot.recent(7);
        let month_ago = spot.on_or_before(last_day - Duration::days(30));
        let year = spot.recent(365);
        ui.horizontal_wrapped(|ui| {
            widgets::stat_tile(
                ui,
                skin,
                "Henry Hub · $/MMBtu",
                &format!("{last:.2}"),
                Some(
                    RichText::new(format!("spot, {}", last_day.format("%a %b %-d")))
                        .color(skin.text_muted),
                ),
            );
            let avg = week.iter().map(|p| p.1).sum::<f64>() / week.len().max(1) as f64;
            widgets::stat_tile(ui, skin, "7-day average", &format!("{avg:.2}"), None);
            if let Some(then) = month_ago {
                let change = last - then;
                widgets::stat_tile(
                    ui,
                    skin,
                    "vs 30 days ago",
                    &format!("{change:+.2}"),
                    Some(
                        RichText::new(format!("{:+.1}%", change / then * 100.0))
                            .color(skin.delta(change)),
                    ),
                );
            }
            let (lo, hi) = year.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                (lo.min(p.1), hi.max(p.1))
            });
            if lo <= hi {
                widgets::stat_tile(ui, skin, "1-year range", &format!("{lo:.2}–{hi:.2}"), None);
            }
        });

        // The latest DA market day: tomorrow once posted, else today.
        let today = market_today();
        let next = today + Duration::days(1);
        let next_snap = cx
            .hub
            .watch(&cx.miso.day_report(DayReportKind::DaExPost, next));
        let posted = next_snap.data().is_some_and(Option::is_some);
        let (day, snap) = if posted {
            (next, next_snap)
        } else {
            (
                today,
                cx.hub
                    .watch(&cx.miso.day_report(DayReportKind::DaExPost, today)),
            )
        };
        let report = snap.data().and_then(Option::as_ref);
        let gas = spot.on_or_before(day).unwrap_or(last);
        widgets::section(
            ui,
            skin,
            &format!("Day-ahead {} against gas", day.format("%a %b %-d")),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Spark spread at")
                    .small()
                    .color(skin.text_muted),
            );
            ui.add(
                egui::DragValue::new(&mut self.heat_rate)
                    .range(5.0..=14.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix(" MMBtu/MWh"),
            )
            .on_hover_text(
                "Reference heat rate: about 7 for a combined-cycle unit, 10-11 for a peaker",
            );
        });
        let hubs = hub_days(report, Component::Lmp);
        let weekend = matches!(day.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let rows: Vec<(&str, Option<f64>)> = hubs
            .iter()
            .map(|h| {
                let [on, _, all] = blocks(day, &h.hours);
                (h.hub, if weekend { all } else { on })
            })
            .collect();
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let mut open = None;
        egui::ScrollArea::horizontal()
            .id_salt("gas-hubs")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("gas-table")
                    .striped(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(100.0))
                    .columns(Column::initial(120.0).at_least(80.0), 3)
                    .header(row_h, |mut h| {
                        let block = if weekend {
                            "DA all hours"
                        } else {
                            "DA on-peak"
                        };
                        for title in [
                            "Hub",
                            &format!("{block} $/MWh"),
                            "Heat rate MMBtu/MWh",
                            "Spark spread $/MWh",
                        ] {
                            h.col(|ui| {
                                widgets::label(ui, skin, title);
                            });
                        }
                    })
                    .body(|mut body| {
                        for (hub, price) in &rows {
                            body.row(row_h, |mut row| {
                                row.col(|ui| {
                                    if widgets::link(ui, skin, hub_short(hub)).clicked() {
                                        open = Some(*hub);
                                    }
                                });
                                let dash = || RichText::new(fmt::DASH).color(skin.text_muted);
                                row.col(|ui| {
                                    num_cell(
                                        ui,
                                        price.map_or_else(dash, |p| {
                                            RichText::new(fmt::price(p)).color(cx.price_color(p))
                                        }),
                                    );
                                });
                                row.col(|ui| {
                                    num_cell(
                                        ui,
                                        price
                                            .and_then(|p| implied_heat_rate(p, gas))
                                            .map_or_else(dash, |hr| {
                                                RichText::new(format!("{hr:.1}"))
                                            }),
                                    );
                                });
                                row.col(|ui| {
                                    num_cell(
                                        ui,
                                        price.map_or_else(dash, |p| {
                                            let s = spark_spread(p, gas, self.heat_rate);
                                            RichText::new(fmt::signed(s)).color(skin.delta(s))
                                        }),
                                    );
                                });
                            });
                        }
                    });
            });
        if let Some(hub) = open {
            cx.open(Route::new("GP", [hub.to_owned()]));
        }

        ui.horizontal_wrapped(|ui| {
            for (label, d) in [("90d", 90), ("1y", 365), ("5y", 5 * 365)] {
                ui.selectable_value(&mut self.days, d, label);
            }
            csv::copy_button(ui, skin, || {
                csv::to_csv(
                    &["date", "henry_hub_usd_per_mmbtu"],
                    spot.recent(self.days)
                        .iter()
                        .map(|(d, v)| vec![d.to_string(), format!("{v:.3}")]),
                )
            });
            ui.label(
                RichText::new(format!(
                    "Heat rates use {gas:.2} $/MMBtu, the latest spot on or before {}. Henry Hub \
                     spot from EIA, updated weekly and a few days behind; gas delivered into MISO \
                     trades at a basis to it.",
                    day.format("%b %-d")
                ))
                .small()
                .color(skin.text_muted),
            );
        });
        let pts: Vec<(NaiveDateTime, f64)> = spot
            .recent(self.days)
            .iter()
            .filter_map(|(d, v)| Some((d.and_hms_opt(0, 0, 0)?, *v)))
            .collect();
        chart::time_plot("gas-hh", skin).show(ui, |plot| {
            plot.line(chart::line("Henry Hub $/MMBtu", &pts, skin.series(2)));
        });
    }
}
