//! HOME: the launchpad. Headline numbers, the trading hubs, the generation mix
//! and the constraints that matter right now.

use egui::{Grid, RichText, ScrollArea, Ui};
use mt_core::{TRADING_HUBS, hub_short};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "HOME",
    aliases: &["MAIN", "LAUNCH"],
    name: "Launchpad",
    category: Category::Overview,
    usage: "HOME",
    description: "Headline grid numbers, trading-hub prices, generation mix and top binding constraints.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Home))
}

struct Home;

impl Panel for Home {
    fn title(&self) -> String {
        "HOME".into()
    }

    fn route(&self) -> Route {
        Route::code("HOME")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let snapshot = cx.hub.watch(&cx.miso.snapshot());
        let board = cx.hub.watch(&cx.miso.lmp_board());
        let intraday = cx.hub.watch(&cx.miso.rt_intraday());
        let fuel = cx.hub.watch(&cx.miso.fuel_mix());
        let cons = cx.hub.watch(&cx.miso.binding_constraints());
        let exante = cx.hub.watch(&cx.miso.exante_hubs());

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            widgets::title_bar(ui, skin, "MISO at a glance", |ui| {
                widgets::freshness(ui, skin, &board)
            });

            ui.horizontal_wrapped(|ui| {
                let snap = snapshot.data();
                let demand = snap.and_then(|s| s.current_demand()).and_then(|i| i.value);
                let peak = snap.and_then(|s| s.forecast_peak()).and_then(|i| i.value);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Demand · MW",
                    &fmt::mw_opt(demand),
                    peak.map(|p| {
                        RichText::new(format!("forecast peak {}", fmt::mw(p)))
                            .color(skin.text_muted)
                    }),
                );
                let mec = snap
                    .and_then(|s| s.marginal_energy_cost())
                    .and_then(|i| i.value);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Marginal energy · $/MWh",
                    &fmt::price_opt(mec),
                    None,
                );
                let nsi = snap
                    .and_then(|s| s.scheduled_interchange())
                    .and_then(|i| i.value);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Net interchange · MW",
                    &nsi.map_or_else(|| fmt::DASH.into(), fmt::mw_signed),
                    nsi.map(|v| {
                        RichText::new(if v < 0.0 { "net import" } else { "net export" })
                            .color(skin.text_muted)
                    }),
                );
                if let Some(mix) = fuel.data() {
                    let total = mix.total();
                    let renew: f64 = mix
                        .fuels
                        .iter()
                        .filter(|(c, _)| matches!(mt_core::fuel_key(c), "wind" | "solar"))
                        .map(|(_, mw)| mw)
                        .sum();
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Generation · MW",
                        &fmt::mw(total),
                        Some(
                            RichText::new(format!(
                                "wind + solar {}",
                                fmt::pct(renew / total * 100.0)
                            ))
                            .color(skin.text_muted),
                        ),
                    );
                }
                if let Some(c) = cons.data() {
                    let worst = c
                        .constraints
                        .iter()
                        .filter_map(|k| k.shadow_price)
                        .min_by(|a, b| a.total_cmp(b));
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Binding constraints",
                        &c.constraints.len().to_string(),
                        worst.map(|w| {
                            RichText::new(format!("max shadow {}", fmt::price(w)))
                                .color(skin.text_muted)
                        }),
                    );
                }
            });

            widgets::section(ui, skin, "Trading hubs · $/MWh");
            widgets::with_data(ui, skin, &board, |ui, board| {
                let he = board
                    .rt_hour_ending
                    .map_or(String::new(), |h| format!(" HE{h}"));
                Grid::new("home-hubs")
                    .striped(true)
                    .num_columns(8)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for h in [
                            "Hub",
                            "RT 5-min",
                            "Δ 5-min",
                            "Next (ex-ante)",
                            &format!("RT{he}"),
                            "DA ex-post",
                            "RT − DA",
                            "Today",
                        ] {
                            widgets::label(ui, skin, h);
                        }
                        ui.end_row();
                        for hub in TRADING_HUBS {
                            let Some(row) = board.row(hub) else { continue };
                            if widgets::link(ui, skin, hub_short(hub)).clicked() {
                                cx.open(Route::new("GP", [hub]));
                            }
                            let rt = row.rt_5min.map(|p| p.lmp);
                            ui.label(
                                RichText::new(fmt::price_opt(rt))
                                    .color(rt.map_or(skin.text, |v| cx.price_color(v))),
                            );
                            let series = intraday.data().and_then(|d| d.series(hub));
                            let prev = intraday.data().and_then(|d| d.previous(hub)).map(|p| p.lmp);
                            match (rt, prev) {
                                (Some(now), Some(prev)) => {
                                    let d = now - prev;
                                    let arrow = if d > 0.0 {
                                        "▲"
                                    } else if d < 0.0 {
                                        "▼"
                                    } else {
                                        ""
                                    };
                                    ui.label(
                                        RichText::new(format!("{arrow} {}", fmt::signed(d)))
                                            .color(skin.delta(d)),
                                    );
                                }
                                _ => {
                                    ui.label(RichText::new(fmt::DASH).color(skin.text_muted));
                                }
                            }
                            // Ex-ante: the price RT dispatch expects for the coming interval.
                            let next = exante
                                .data()
                                .and_then(|x| x.hubs.iter().find(|(n, _)| n == hub))
                                .map(|(_, p)| p.lmp);
                            ui.label(
                                RichText::new(fmt::price_opt(next))
                                    .color(next.map_or(skin.text_muted, |v| cx.price_color(v))),
                            );
                            ui.label(fmt::price_opt(row.rt_hourly.map(|p| p.lmp)));
                            ui.label(fmt::price_opt(row.da_expost.map(|p| p.lmp)));
                            let dart = row.dart();
                            ui.label(
                                RichText::new(dart.map_or_else(|| fmt::DASH.into(), fmt::signed))
                                    .color(dart.map_or(skin.text_muted, |d| skin.delta(d))),
                            );
                            match series {
                                Some(s) => {
                                    widgets::sparkline(
                                        ui,
                                        s.lmp,
                                        skin.series(0),
                                        egui::vec2(140.0, 18.0),
                                    );
                                }
                                None => {
                                    ui.label(
                                        RichText::new(if intraday.loading {
                                            "loading…"
                                        } else {
                                            fmt::DASH
                                        })
                                        .small()
                                        .color(skin.text_muted),
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });
            });

            widgets::section(ui, skin, "Generation mix");
            widgets::with_data(ui, skin, &fuel, |ui, mix| {
                let mut fuels = mix.fuels.clone();
                fuels.sort_by(|a, b| b.1.total_cmp(&a.1));
                let segments: Vec<_> = fuels
                    .iter()
                    .map(|(c, mw)| (c.clone(), *mw, skin.fuel(c)))
                    .collect();
                widgets::stacked_bar(ui, &segments, 18.0);
                ui.horizontal_wrapped(|ui| {
                    let total = mix.total();
                    for (cat, mw, color) in &segments {
                        widgets::lamp(ui, *color);
                        ui.label(
                            RichText::new(format!("{cat} {}", fmt::pct(mw / total * 100.0)))
                                .small(),
                        );
                        ui.add_space(6.0);
                    }
                });
            });

            widgets::section(ui, skin, "Top binding constraints");
            widgets::with_data(ui, skin, &cons, |ui, c| {
                if c.constraints.is_empty() {
                    ui.label(
                        RichText::new("Nothing binding this interval.").color(skin.text_muted),
                    );
                    return;
                }
                let mut top: Vec<_> = c.constraints.iter().collect();
                top.sort_by(|a, b| {
                    a.shadow_price
                        .unwrap_or(0.0)
                        .abs()
                        .total_cmp(&b.shadow_price.unwrap_or(0.0).abs())
                        .reverse()
                });
                Grid::new("home-cons")
                    .striped(true)
                    .num_columns(2)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for k in top.iter().take(5) {
                            ui.label(RichText::new(&k.name).monospace());
                            ui.label(
                                RichText::new(fmt::price_opt(k.shadow_price)).color(skin.warning),
                            );
                            ui.end_row();
                        }
                    });
                if widgets::link(
                    ui,
                    skin,
                    &format!("All {} constraints → CONS", c.constraints.len()),
                )
                .clicked()
                {
                    cx.open(Route::code("CONS"));
                }
            });
        });
    }
}
