//! SEAM: MISO's interfaces with its neighbours. PJM's coordinated transaction
//! scheduling (CTS) forecast at the PJM interface against MISO's own price
//! there, and RT and DA prices at every interface pricing node.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::{LmpBoardRow, PJM_INTERFACE, interface_neighbour};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series::{self, Component};
use crate::widgets::table::num_cell;
use crate::widgets::{self, chart, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "SEAM",
    aliases: &["SEAMS", "CTS", "INTERFACE"],
    name: "Seams & interfaces",
    category: Category::Prices,
    usage: "SEAM",
    description: "MISO's interfaces with PJM, SPP, TVA, Ontario and others: PJM's CTS forecast at the PJM interface against MISO's price there, and RT and DA prices at every interface node.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Seam))
}

struct Seam;

impl Panel for Seam {
    fn title(&self) -> String {
        "SEAM".into()
    }

    fn route(&self) -> Route {
        Route::code("SEAM")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let board = cx.hub.watch(&cx.miso.lmp_board());
        let cts = cx.hub.watch(&cx.miso.cts());
        widgets::title_bar(ui, skin, "Seams · interfaces", |ui| {
            widgets::freshness(ui, skin, &board)
        });
        egui::ScrollArea::vertical()
            .id_salt("seam-scroll")
            .show(ui, |ui| {
                widgets::section(
                    ui,
                    skin,
                    "PJM interface · coordinated transaction scheduling",
                );
                let b = board.data();
                self.cts(
                    ui,
                    cx,
                    &cts,
                    b.and_then(|b| b.row(PJM_INTERFACE)),
                    b.and_then(|b| b.interval),
                );
                widgets::section(ui, skin, "Interface prices · $/MWh");
                widgets::with_data(ui, skin, &board, |ui, b| interface_table(ui, cx, b));
            });
    }
}

impl Seam {
    fn cts(
        &self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        cts: &mt_data::Snapshot<mt_core::Cts>,
        pjmc: Option<&LmpBoardRow>,
        interval: Option<chrono::NaiveDateTime>,
    ) {
        let skin = cx.skin;
        let Some(c) = cts.data() else {
            widgets::placeholder(ui, skin, cts.error.as_ref().map(ToString::to_string));
            return;
        };
        let latest = c.latest_case();
        let freshest = c.freshest();
        let miso_rt = pjmc.and_then(|r| r.rt_5min).map(|p| p.lmp);
        // PJM's forecast for the interval MISO's latest price covers, or the
        // first one the latest case has.
        let pjm_now = interval
            .and_then(|t| freshest.iter().rev().find(|(at, _)| *at <= t))
            .or_else(|| latest.first())
            .map(|p| p.1);
        ui.horizontal_wrapped(|ui| {
            widgets::stat_tile(
                ui,
                skin,
                "PJM forecast · now",
                &fmt::price_opt(pjm_now),
                Some(RichText::new("at the MISO interface").color(skin.text_muted)),
            );
            widgets::stat_tile(
                ui,
                skin,
                "MISO RT · PJMC",
                &fmt::price_opt(miso_rt),
                Some(RichText::new("five-minute ex-post").color(skin.text_muted)),
            );
            let spread = pjm_now.zip(miso_rt).map(|(p, m)| p - m);
            widgets::stat_tile(
                ui,
                skin,
                "PJM − MISO",
                &spread.map_or_else(|| fmt::DASH.into(), fmt::signed),
                spread.map(|s| {
                    let (text, color) = if s > 0.0 {
                        ("PJM higher: favours MISO → PJM", skin.text_muted)
                    } else {
                        ("MISO higher: favours PJM → MISO", skin.text_muted)
                    };
                    RichText::new(text).color(color)
                }),
            );
            if let Some(t) = c.latest_case_time() {
                widgets::stat_tile(
                    ui,
                    skin,
                    "Latest case",
                    &fmt::hm(t),
                    latest.last().map(|(end, _)| {
                        RichText::new(format!("forecasts to {} EST", fmt::hm(*end)))
                            .color(skin.text_muted)
                    }),
                );
            }
        });
        let miso_today = series::node_today(cx, PJM_INTERFACE, Component::Lmp).rt_5min;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(
                    "PJM's forecast LMP at the interface from each CTS case (about every 15 minutes), \
                     against MISO's five-minute price at its PJM interface node (PJMC).",
                )
                .small()
                .color(skin.text_muted),
            );
            csv::copy_button(ui, skin, || {
                csv::to_csv(
                    &["case_approved_est", "interval_est", "pjm_forecast_lmp"],
                    c.forecasts.iter().map(|f| {
                        vec![
                            f.case_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                            f.time.format("%Y-%m-%d %H:%M").to_string(),
                            format!("{:.2}", f.lmp),
                        ]
                    }),
                )
            });
        });
        chart::time_plot("seam-cts", skin)
            .height(220.0)
            .show(ui, |plot| {
                plot.line(chart::line(
                    "PJM forecast (freshest)",
                    &freshest,
                    skin.series(0),
                ));
                plot.line(
                    chart::line("PJM forecast (latest case)", &latest, skin.series(0))
                        .style(egui_plot::LineStyle::dashed_dense())
                        .width(2.2),
                );
                plot.line(chart::line("MISO RT at PJMC", &miso_today, skin.series(1)));
            });
    }
}

fn interface_table(ui: &mut Ui, cx: &mut PanelCx<'_>, board: &mt_core::LmpBoard) {
    let skin = cx.skin;
    let rows: Vec<&LmpBoardRow> = crate::geo::map()
        .nodes
        .iter()
        .filter(|n| n.kind == "Interface")
        .filter_map(|n| board.row(&n.node))
        .collect();
    if rows.is_empty() {
        ui.label(RichText::new("No interface nodes in the LMP table.").color(skin.warning));
        return;
    }
    csv::copy_button(ui, skin, || {
        csv::to_csv(
            &[
                "node",
                "neighbour",
                "rt_lmp",
                "rt_mcc",
                "rt_mlc",
                "da_expost",
                "rt_minus_da",
            ],
            rows.iter().map(|r| {
                let f = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:.2}"));
                vec![
                    r.node.clone(),
                    interface_neighbour(&r.node).unwrap_or("").to_owned(),
                    f(r.rt_5min.map(|p| p.lmp)),
                    f(r.rt_5min.map(|p| p.mcc)),
                    f(r.rt_5min.map(|p| p.mlc)),
                    f(r.da_expost.map(|p| p.lmp)),
                    f(r.dart()),
                ]
            }),
        )
    });
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let mut open = None;
    egui::ScrollArea::horizontal()
        .id_salt("seam-table-scroll")
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .id_salt("seam-table")
                .striped(true)
                .vscroll(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(130.0))
                .column(Column::initial(170.0))
                .columns(Column::initial(76.0).at_least(56.0), 5)
                .header(row_h, |mut h| {
                    for title in [
                        "Node",
                        "Neighbour",
                        "RT LMP",
                        "MCC",
                        "MLC",
                        "DA ex-post",
                        "RT − DA",
                    ] {
                        h.col(|ui| {
                            widgets::label(ui, skin, title);
                        });
                    }
                })
                .body(|mut body| {
                    for r in &rows {
                        body.row(row_h, |mut row| {
                            row.col(|ui| {
                                if widgets::link(ui, skin, &r.node).clicked() {
                                    open = Some(r.node.clone());
                                }
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(
                                        interface_neighbour(&r.node).unwrap_or(fmt::DASH),
                                    )
                                    .color(skin.text_muted),
                                );
                            });
                            let price = |v: Option<f64>| match v {
                                Some(v) => RichText::new(fmt::price(v)).color(cx.price_color(v)),
                                None => RichText::new(fmt::DASH).color(skin.text_muted),
                            };
                            let signed = |v: Option<f64>| match v {
                                Some(v) => RichText::new(fmt::signed(v)).color(skin.delta(v)),
                                None => RichText::new(fmt::DASH).color(skin.text_muted),
                            };
                            row.col(|ui| num_cell(ui, price(r.rt_5min.map(|p| p.lmp))));
                            row.col(|ui| num_cell(ui, signed(r.rt_5min.map(|p| p.mcc))));
                            row.col(|ui| num_cell(ui, signed(r.rt_5min.map(|p| p.mlc))));
                            row.col(|ui| num_cell(ui, price(r.da_expost.map(|p| p.lmp))));
                            row.col(|ui| num_cell(ui, signed(r.dart())));
                        });
                    }
                });
        });
    ui.label(
        RichText::new(
            "Interface nodes price imports and exports with each neighbour. Click one to graph it.",
        )
        .small()
        .color(skin.text_muted),
    );
    if let Some(node) = open {
        cx.open(Route::new("GP", [node]));
    }
}
