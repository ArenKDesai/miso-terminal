//! LMP: the price monitor. Every key pricing node with RT and DA side by side,
//! or the latest five-minute price at all ~2,600 CP nodes.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::is_trading_hub;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::table::{Sort, cmp_opt, num_cell, sort_header};
use crate::widgets::{self, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "LMP",
    aliases: &["PRICES", "MON"],
    name: "LMP monitor",
    category: Category::Prices,
    usage: "LMP [ALL]",
    description: "Real-time and day-ahead LMPs by node, sortable and filterable. ALL lists every CP node's latest 5-minute price.",
    takes_node: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let all = args.first().is_some_and(|a| a.eq_ignore_ascii_case("ALL"));
    Ok(Box::new(Lmp {
        all_nodes: all,
        filter: String::new(),
        region: None,
        hubs_only: false,
        sort: Sort::new(0, false),
    }))
}

struct Lmp {
    all_nodes: bool,
    filter: String,
    region: Option<String>,
    hubs_only: bool,
    sort: Sort,
}

/// One table row, whichever source it came from.
struct Row {
    node: String,
    region: String,
    rt: Option<f64>,
    rt_hourly: Option<f64>,
    da_exante: Option<f64>,
    da_expost: Option<f64>,
    mcc: Option<f64>,
    mlc: Option<f64>,
    change: Option<f64>,
}

impl Row {
    fn dart(&self) -> Option<f64> {
        Some(self.rt? - self.da_expost?)
    }
}

const KEY_COLUMNS: &[(&str, bool)] = &[
    ("Node", false),
    ("Region", false),
    ("RT 5-min", true),
    ("RT hour", true),
    ("DA ex-ante", true),
    ("DA ex-post", true),
    ("RT − DA", true),
    ("MCC", true),
    ("MLC", true),
];

const ALL_COLUMNS: &[(&str, bool)] = &[
    ("Node", false),
    ("RT 5-min", true),
    ("Δ 5-min", true),
    ("MCC", true),
    ("MLC", true),
];

impl Lmp {
    fn rows(&self, cx: &PanelCx<'_>) -> (Vec<Row>, mt_data::Snapshot<()>) {
        if self.all_nodes {
            let snap = cx.hub.watch(&cx.miso.rt_intraday());
            let rows = snap
                .data()
                .map(|d| {
                    d.latest_all()
                        .into_iter()
                        .map(|(node, p)| Row {
                            node: node.to_owned(),
                            region: String::new(),
                            rt: Some(p.lmp),
                            rt_hourly: None,
                            da_exante: None,
                            da_expost: None,
                            mcc: Some(p.mcc),
                            mlc: Some(p.mlc),
                            change: d.previous(node).map(|prev| p.lmp - prev.lmp),
                        })
                        .collect()
                })
                .unwrap_or_default();
            (rows, snap.status())
        } else {
            let snap = cx.hub.watch(&cx.miso.lmp_board());
            let rows = snap
                .data()
                .map(|b| {
                    b.rows
                        .iter()
                        .map(|r| Row {
                            node: r.node.clone(),
                            region: r.region.clone(),
                            rt: r.rt_5min.map(|p| p.lmp),
                            rt_hourly: r.rt_hourly.map(|p| p.lmp),
                            da_exante: r.da_exante.map(|p| p.lmp),
                            da_expost: r.da_expost.map(|p| p.lmp),
                            mcc: r.rt_5min.map(|p| p.mcc),
                            mlc: r.rt_5min.map(|p| p.mlc),
                            change: None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            (rows, snap.status())
        }
    }

    fn sort_rows(&self, rows: &mut [Row]) {
        let s = self.sort;
        let key = |r: &Row, col: usize| -> Option<f64> {
            match (self.all_nodes, col) {
                (false, 2) | (true, 1) => r.rt,
                (false, 3) => r.rt_hourly,
                (false, 4) => r.da_exante,
                (false, 5) => r.da_expost,
                (false, 6) => r.dart(),
                (false, 7) | (true, 3) => r.mcc,
                (false, 8) | (true, 4) => r.mlc,
                (true, 2) => r.change,
                _ => None,
            }
        };
        rows.sort_by(|a, b| match (self.all_nodes, s.column) {
            (_, 0) => s.apply(a.node.cmp(&b.node)),
            (false, 1) => s
                .apply(a.region.cmp(&b.region))
                .then_with(|| a.node.cmp(&b.node)),
            (_, c) => cmp_opt(key(a, c), key(b, c), &s).then_with(|| a.node.cmp(&b.node)),
        });
    }
}

impl Panel for Lmp {
    fn title(&self) -> String {
        if self.all_nodes {
            "LMP · all nodes".into()
        } else {
            "LMP".into()
        }
    }

    fn route(&self) -> Route {
        if self.all_nodes {
            Route::new("LMP", ["ALL"])
        } else {
            Route::code("LMP")
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let (mut rows, snap) = self.rows(cx);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("filter nodes")
                    .desired_width(160.0),
            );
            let before = self.all_nodes;
            ui.selectable_value(&mut self.all_nodes, false, "Key nodes");
            ui.selectable_value(&mut self.all_nodes, true, "All CP nodes");
            if before != self.all_nodes {
                self.sort = Sort::new(0, false);
            }
            if !self.all_nodes {
                egui::ComboBox::from_id_salt("lmp-region")
                    .selected_text(self.region.as_deref().unwrap_or("All regions"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.region, None, "All regions");
                        for r in ["North", "Midwest", "South"] {
                            ui.selectable_value(&mut self.region, Some(r.to_owned()), r);
                        }
                    });
            }
            ui.checkbox(&mut self.hubs_only, "Hubs");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::freshness(ui, skin, &snap);
            });
        });

        let needle = self.filter.trim().to_ascii_uppercase();
        rows.retain(|r| {
            (needle.is_empty() || r.node.contains(&needle))
                && (!self.hubs_only || is_trading_hub(&r.node))
                && self.region.as_ref().is_none_or(|reg| &r.region == reg)
        });
        self.sort_rows(&mut rows);

        if rows.is_empty() {
            match (&snap.error, snap.data.is_some()) {
                (_, true) => {
                    ui.label(RichText::new("No nodes match.").color(skin.text_muted));
                }
                (err, false) => {
                    widgets::placeholder(ui, skin, err.as_ref().map(ToString::to_string))
                }
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} nodes · click a node to graph it", rows.len()))
                    .small()
                    .color(skin.text_muted),
            );
            let all = self.all_nodes;
            csv::copy_button(ui, skin, || {
                let c = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:.2}"));
                if all {
                    csv::to_csv(
                        &["node", "rt_5min", "change_5min", "mcc", "mlc"],
                        rows.iter().map(|r| {
                            vec![r.node.clone(), c(r.rt), c(r.change), c(r.mcc), c(r.mlc)]
                        }),
                    )
                } else {
                    csv::to_csv(
                        &[
                            "node",
                            "region",
                            "rt_5min",
                            "rt_hourly",
                            "da_exante",
                            "da_expost",
                            "rt_minus_da",
                            "mcc",
                            "mlc",
                        ],
                        rows.iter().map(|r| {
                            vec![
                                r.node.clone(),
                                r.region.clone(),
                                c(r.rt),
                                c(r.rt_hourly),
                                c(r.da_exante),
                                c(r.da_expost),
                                c(r.dart()),
                                c(r.mcc),
                                c(r.mlc),
                            ]
                        }),
                    )
                }
            });
        });

        let columns = if self.all_nodes {
            ALL_COLUMNS
        } else {
            KEY_COLUMNS
        };
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
        let mut table = TableBuilder::new(ui)
            .id_salt(("lmp-table", self.all_nodes))
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(170.0).at_least(90.0).clip(true));
        for _ in 1..columns.len() {
            table = table.column(Column::initial(84.0).at_least(50.0));
        }
        // Today's five-minute trend; only visible rows draw one.
        table = table.column(Column::initial(130.0).at_least(60.0));
        let intraday = cx.hub.watch(&cx.miso.rt_intraday());
        let mut sort = self.sort;
        let mut clicked: Option<String> = None;
        table
            .header(row_h + 4.0, |mut header| {
                for (i, (name, numeric)) in columns.iter().enumerate() {
                    header.col(|ui| sort_header(ui, skin, name, i, *numeric, &mut sort));
                }
                header.col(|ui| {
                    widgets::label(ui, skin, "Today");
                });
            })
            .body(|body| {
                body.rows(row_h, rows.len(), |mut row| {
                    let r = &rows[row.index()];
                    row.col(|ui| {
                        if widgets::link(ui, skin, &r.node).clicked() {
                            clicked = Some(r.node.clone());
                        }
                    });
                    let price = |v: Option<f64>| {
                        RichText::new(fmt::price_opt(v))
                            .color(v.map_or(skin.text_muted, |v| cx.price_color(v)))
                    };
                    let signed = |v: Option<f64>| {
                        RichText::new(v.map_or_else(|| fmt::DASH.into(), fmt::signed))
                            .color(v.map_or(skin.text_muted, |v| skin.delta(v)))
                    };
                    if self.all_nodes {
                        row.col(|ui| num_cell(ui, price(r.rt)));
                        row.col(|ui| num_cell(ui, signed(r.change)));
                        row.col(|ui| num_cell(ui, fmt::price_opt(r.mcc)));
                        row.col(|ui| num_cell(ui, fmt::price_opt(r.mlc)));
                    } else {
                        row.col(|ui| {
                            ui.label(RichText::new(&r.region).color(skin.text_muted));
                        });
                        row.col(|ui| num_cell(ui, price(r.rt)));
                        row.col(|ui| num_cell(ui, price(r.rt_hourly)));
                        row.col(|ui| num_cell(ui, price(r.da_exante)));
                        row.col(|ui| num_cell(ui, price(r.da_expost)));
                        row.col(|ui| num_cell(ui, signed(r.dart())));
                        row.col(|ui| num_cell(ui, fmt::price_opt(r.mcc)));
                        row.col(|ui| num_cell(ui, fmt::price_opt(r.mlc)));
                    }
                    row.col(|ui| {
                        if let Some(s) = intraday.data().and_then(|d| d.series(&r.node)) {
                            widgets::sparkline(ui, s.lmp, skin.series(0), egui::vec2(120.0, 14.0));
                        }
                    });
                });
            });
        self.sort = sort;
        if let Some(node) = clicked {
            cx.open(Route::new("GP", [node]));
        }
    }
}
