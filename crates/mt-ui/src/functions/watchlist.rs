//! WL: the watchlist. Your favourite nodes in one table, RT against DA, with
//! today's five-minute sparkline, and your securities in another (last,
//! change, volume and today's chart, from Alpaca). Edits save to `config.toml`.

use chrono::Timelike;
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::DayReportKind;
use mt_core::time::{market_today, now_market};

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker};
use crate::widgets::node_picker::NodePicker;
use crate::widgets::table::num_cell;
use crate::widgets::{self, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "WL",
    aliases: &["WATCH", "FAV"],
    name: "Watchlist",
    category: Category::Prices,
    usage: "WL [node | security]",
    description: "Your favourite nodes (RT vs DA, change, today's sparkline) and securities (last, change, volume, today's chart). WL <node> or WL XLU US adds one; ☆ in GP does too.",
    takes_node: true,
    takes_security: true,
    takes_option: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Watchlist {
        add: args
            .first()
            .map(|n| n.trim().to_ascii_uppercase())
            .filter(|n| !n.is_empty()),
        picker: NodePicker::default(),
        securities: SecurityPicker::default(),
    }))
}

struct Watchlist {
    /// A node or security given on the command line, added on first draw.
    add: Option<String>,
    picker: NodePicker,
    securities: SecurityPicker,
}

struct Row {
    node: String,
    rt: Option<f64>,
    change: Option<f64>,
    rt_hourly: Option<f64>,
    da: Option<f64>,
}

impl Row {
    fn dart(&self) -> Option<f64> {
        Some(self.rt? - self.da?)
    }
}

impl Panel for Watchlist {
    fn title(&self) -> String {
        "WL".into()
    }

    fn route(&self) -> Route {
        Route::code("WL")
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        // One watchlist: `WL <node>` adds to the open one.
        if let Some(n) = args.first() {
            self.add = Some(n.trim().to_ascii_uppercase());
        }
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if let Some(n) = self.add.take() {
            cx.send(AppCommand::AddFavorite(n));
        }
        let board = cx.hub.watch(&cx.miso.lmp_board());
        let intraday = cx.hub.watch(&cx.miso.rt_intraday());
        let da_report = cx
            .hub
            .watch(&cx.miso.day_report(DayReportKind::DaExPost, market_today()));
        let he = now_market().hour() as usize; // index of the current hour ending

        let rows: Vec<Row> = cx
            .config
            .ui
            .favorite_nodes
            .iter()
            .map(|node| {
                let b = board.data().and_then(|b| b.row(node));
                let latest = intraday.data().and_then(|d| d.latest(node));
                let rt = b
                    .and_then(|r| r.rt_5min.map(|p| p.lmp))
                    .or(latest.map(|l| l.1.lmp));
                let prev = intraday
                    .data()
                    .and_then(|d| d.previous(node))
                    .map(|p| p.lmp);
                let da = b.and_then(|r| r.da_expost.map(|p| p.lmp)).or_else(|| {
                    let v = da_report
                        .data()?
                        .as_ref()?
                        .node(node)?
                        .lmp
                        .get(he)
                        .copied()?;
                    v.is_finite().then_some(f64::from(v))
                });
                Row {
                    node: node.clone(),
                    rt,
                    change: latest.map(|l| l.1.lmp).zip(prev).map(|(now, p)| now - p),
                    rt_hourly: b.and_then(|r| r.rt_hourly.map(|p| p.lmp)),
                    da,
                }
            })
            .collect();

        widgets::title_bar(ui, skin, "Watchlist", |ui| {
            widgets::freshness(ui, skin, &board);
        });
        ui.horizontal(|ui| {
            if let Some(n) = self.picker.show(ui, cx, "wl", "add a node…") {
                cx.send(AppCommand::AddFavorite(n));
            }
            csv::copy_button(ui, skin, || {
                csv::to_csv(
                    &[
                        "node",
                        "rt_5min",
                        "change_5min",
                        "rt_hourly",
                        "da_expost",
                        "rt_minus_da",
                    ],
                    rows.iter().map(|r| {
                        let c = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:.2}"));
                        vec![
                            r.node.clone(),
                            c(r.rt),
                            c(r.change),
                            c(r.rt_hourly),
                            c(r.da),
                            c(r.dart()),
                        ]
                    }),
                )
            });
        });
        if rows.is_empty() {
            ui.label(
                RichText::new("No nodes yet. Add one above, or press ☆ Watch in GP.")
                    .color(skin.text_muted),
            );
        } else {
            node_table(ui, cx, &rows, &intraday);
        }
        securities(ui, cx, &mut self.securities);
    }
}

fn node_table(
    ui: &mut Ui,
    cx: &mut PanelCx<'_>,
    rows: &[Row],
    intraday: &mt_data::Snapshot<mt_core::RtIntraday>,
) {
    let skin = cx.skin;
    let row_h = 22.0;
    let mut open = None;
    let mut remove = None;
    TableBuilder::new(ui)
        .id_salt("wl-table")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(160.0).at_least(90.0).clip(true))
        .columns(Column::initial(80.0), 5)
        .column(Column::initial(150.0))
        .column(Column::exact(24.0))
        .header(row_h, |mut h| {
            for title in [
                "Node",
                "RT 5-min",
                "Δ 5-min",
                "RT hour",
                "DA ex-post",
                "RT − DA",
                "Today",
                "",
            ] {
                h.col(|ui| {
                    widgets::label(ui, skin, title);
                });
            }
        })
        .body(|body| {
            body.rows(row_h, rows.len(), |mut row| {
                let r = &rows[row.index()];
                row.col(|ui| {
                    if widgets::link(ui, skin, &r.node).clicked() {
                        open = Some(r.node.clone());
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
                row.col(|ui| num_cell(ui, price(r.rt)));
                row.col(|ui| num_cell(ui, signed(r.change)));
                row.col(|ui| num_cell(ui, price(r.rt_hourly)));
                row.col(|ui| num_cell(ui, price(r.da)));
                row.col(|ui| num_cell(ui, signed(r.dart())));
                row.col(|ui| match intraday.data().and_then(|d| d.series(&r.node)) {
                    Some(s) => {
                        widgets::sparkline(ui, s.lmp, skin.series(0), egui::vec2(140.0, 16.0));
                    }
                    None => {
                        ui.label(RichText::new(fmt::DASH).color(skin.text_muted));
                    }
                });
                row.col(|ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Remove from watchlist")
                        .clicked()
                    {
                        remove = Some(r.node.clone());
                    }
                });
            });
        });
    if let Some(n) = open {
        cx.open(Route::new("GP", [n]));
    }
    if let Some(n) = remove {
        cx.send(AppCommand::RemoveFavorite(n));
    }
}

/// The watchlist's securities: last, change, volume and today's chart.
fn securities(ui: &mut Ui, cx: &mut PanelCx<'_>, picker: &mut SecurityPicker) {
    let skin = cx.skin;
    let list: Vec<mt_core::instrument::Security> = cx
        .config
        .ui
        .favorite_securities
        .iter()
        .filter_map(|s| market::security_of(s))
        .collect();
    widgets::section(ui, skin, "Securities");
    if !cx.alpaca.is_ready() {
        if list.is_empty() {
            ui.label(
                RichText::new(
                    "Stocks and ETFs can sit here too, once Alpaca keys are stored in SET.",
                )
                .small()
                .color(skin.text_muted),
            );
        } else {
            market::needs_keys(ui, cx);
        }
        return;
    }
    ui.horizontal(|ui| {
        if let Some(s) = picker.show(ui, cx, "wl-sec", "add a ticker…") {
            cx.send(AppCommand::AddFavorite(s.to_string()));
        }
        if !list.is_empty() && ui.small_button("Open in Q").clicked() {
            cx.open(Route::new("Q", ["WL"]));
        }
    });
    if list.is_empty() {
        ui.label(
            RichText::new(
                "No securities yet. Add one above, type WL XLU US, or press ☆ Watch in GP.",
            )
            .color(skin.text_muted),
        );
        return;
    }
    let symbols: Vec<String> = list.iter().map(|s| s.ticker.clone()).collect();
    let board = market::board(cx, &symbols);
    let assets = market::assets(cx);
    let cal = market::calendar(cx.hub, cx.alpaca);
    let (session, _) = market::sessions(&cal, mt_core::time::now_utc());
    let bars = cx.hub.watch(
        &cx.alpaca
            .bars(&symbols, mt_alpaca::Timeframe::Min15, session, None),
    );
    let row_h = 22.0;
    let (mut open, mut remove) = (None, None);
    TableBuilder::new(ui)
        .id_salt("wl-securities")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(84.0).at_least(70.0))
        .column(Column::initial(170.0).at_least(80.0).clip(true))
        .columns(Column::initial(76.0), 4)
        .column(Column::initial(130.0))
        .column(Column::exact(24.0))
        .header(row_h, |mut h| {
            for title in [
                "Security", "Name", "Last", "Chg", "% Chg", "Volume", "Today", "",
            ] {
                h.col(|ui| {
                    widgets::label(ui, skin, title);
                });
            }
        })
        .body(|body| {
            body.rows(row_h, symbols.len(), |mut row| {
                let sym = &symbols[row.index()];
                let r = board.row(sym);
                let sec = format!("{sym} US");
                row.col(|ui| {
                    if widgets::link(ui, skin, &sec).clicked() {
                        open = Some(sec.clone());
                    }
                });
                row.col(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(market::name_of(&assets, sym).unwrap_or_default())
                                .color(skin.text_muted),
                        )
                        .truncate(),
                    );
                });
                let delta = |v: Option<f64>, text: String| {
                    RichText::new(text).color(v.map_or(skin.text_muted, |v| skin.delta(v)))
                };
                row.col(|ui| num_cell(ui, market::fmt::price_opt(r.last)));
                row.col(|ui| num_cell(ui, delta(r.change, market::fmt::change_opt(r.change))));
                row.col(|ui| num_cell(ui, delta(r.change_pct, market::fmt::pct_opt(r.change_pct))));
                row.col(|ui| num_cell(ui, market::fmt::volume_opt(r.volume)));
                row.col(|ui| {
                    let closes: Vec<f32> = bars
                        .data()
                        .map(|b| b.get(sym).iter().map(|b| b.close as f32).collect())
                        .unwrap_or_default();
                    let color = r.change.map_or(skin.series(0), |c| skin.delta(c));
                    widgets::sparkline(ui, &closes, color, egui::vec2(120.0, 16.0));
                });
                row.col(|ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Remove from watchlist")
                        .clicked()
                    {
                        remove = Some(sec.clone());
                    }
                });
            });
        });
    if let Some(s) = open {
        cx.open(Route::new("GP", [s]));
    }
    if let Some(s) = remove {
        cx.send(AppCommand::RemoveFavorite(s));
    }
}
