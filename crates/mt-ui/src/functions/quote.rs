//! Q: the quote monitor. A list of stocks and ETFs with live prices from
//! Alpaca: last, change, bid and ask, volume, the day's range and a chart of
//! the latest session. Snapshots each minute set the baseline; the stream's
//! trades, quotes and minute bars keep it current.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_alpaca::board::Row;
use mt_alpaca::{SecurityList, Timeframe};
use mt_core::instrument::Security;

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker, fmt};
use crate::widgets::table::{Sort, cmp_opt, num_cell, sort_header};
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "Q",
    aliases: &["QM", "QUOTES", "MON"],
    name: "Quote monitor",
    category: Category::Markets,
    usage: "Q [list | WL | securities…]",
    description: "Live prices for stocks and ETFs (Alpaca): last, change, bid and ask, volume, the day's range and today's chart. Q alone is the Power & gas list; Q WL your watchlist; Q XEL US AEE US any securities.",
    takes_node: false,
    takes_security: true,
    takes_option: false,
    open,
};

/// What the monitor lists.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// A built-in or configured list, by name.
    List(String),
    /// The watchlist's securities.
    Watchlist,
    /// Securities given on the command line or added here.
    Custom(Vec<Security>),
}

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let securities: Vec<Security> = args.iter().filter_map(|a| market::security_of(a)).collect();
    let source = if !securities.is_empty() {
        Source::Custom(securities)
    } else {
        match args.first().map(|a| a.trim().to_ascii_uppercase()) {
            None => Source::List(mt_alpaca::config::DEFAULT_LIST.into()),
            Some(a) if a == "WL" || a == "WATCHLIST" => Source::Watchlist,
            Some(name) => Source::List(name),
        }
    };
    Ok(Box::new(Quotes {
        source,
        sort: Sort::new(usize::MAX, false),
        picker: SecurityPicker::default(),
    }))
}

struct Quotes {
    source: Source,
    sort: Sort,
    picker: SecurityPicker,
}

const COLUMNS: [(&str, bool); 11] = [
    ("Security", false),
    ("Name", false),
    ("Last", true),
    ("Chg", true),
    ("% Chg", true),
    ("Bid", true),
    ("Ask", true),
    ("Volume", true),
    ("Day range", false),
    ("Today", false),
    ("Time", true),
];

impl Quotes {
    fn securities(&self, cx: &PanelCx<'_>) -> (String, Vec<Security>) {
        match &self.source {
            Source::List(name) => match cx.config.markets.list(name) {
                Some(l) => (l.display_title().to_owned(), l.securities()),
                None => (format!("{name} (no such list)"), Vec::new()),
            },
            Source::Watchlist => (
                "Watchlist".into(),
                cx.config
                    .ui
                    .favorite_securities
                    .iter()
                    .filter_map(|s| market::security_of(s))
                    .collect(),
            ),
            Source::Custom(s) => ("Securities".into(), s.clone()),
        }
    }

    fn selector(&mut self, ui: &mut Ui, cx: &PanelCx<'_>, current: &[Security]) {
        let lists: Vec<SecurityList> = cx.config.markets.all_lists();
        ui.horizontal_wrapped(|ui| {
            for l in &lists {
                let on = matches!(&self.source, Source::List(n) if n.eq_ignore_ascii_case(&l.name));
                if ui
                    .selectable_label(on, l.display_title())
                    .on_hover_text(format!("Q {}", l.name))
                    .clicked()
                {
                    self.source = Source::List(l.name.clone());
                }
            }
            let on = self.source == Source::Watchlist;
            if ui
                .selectable_label(on, "Watchlist")
                .on_hover_text("Q WL: the securities in WL")
                .clicked()
            {
                self.source = Source::Watchlist;
            }
            ui.add_space(8.0);
            if let Some(sec) = self.picker.show(ui, cx, "q", "add a ticker…") {
                let mut list = current.to_vec();
                if !list.contains(&sec) {
                    list.push(sec);
                }
                self.source = Source::Custom(list);
            }
        });
    }
}

impl Panel for Quotes {
    fn title(&self) -> String {
        match &self.source {
            Source::List(n) if n.eq_ignore_ascii_case(mt_alpaca::config::DEFAULT_LIST) => {
                "Q".into()
            }
            Source::List(n) => format!("Q {n}"),
            Source::Watchlist => "Q WL".into(),
            Source::Custom(s) if s.len() == 1 => format!("Q {}", s[0]),
            Source::Custom(s) => format!("Q ({})", s.len()),
        }
    }

    fn route(&self) -> Route {
        match &self.source {
            Source::List(n) if n.eq_ignore_ascii_case(mt_alpaca::config::DEFAULT_LIST) => {
                Route::code("Q")
            }
            Source::List(n) => Route::new("Q", [n.clone()]),
            Source::Watchlist => Route::new("Q", ["WL"]),
            Source::Custom(s) => Route::new("Q", s.iter().map(ToString::to_string)),
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let (title, securities) = self.securities(cx);
        widgets::title_bar(ui, skin, &format!("Quote monitor · {title}"), |ui| {
            if cx.alpaca.is_ready() {
                market::status_label(ui, cx);
            }
        });
        if market::needs_keys(ui, cx) {
            return;
        }
        self.selector(ui, cx, &securities);
        if securities.is_empty() {
            ui.add_space(8.0);
            let msg = match &self.source {
                Source::Watchlist => {
                    "No securities on the watchlist yet. Add one with WL XLU US or ☆ in GP."
                }
                _ => "No securities here. Pick a list above, or type Q XEL US AEE US.",
            };
            ui.label(RichText::new(msg).color(skin.text_muted));
            return;
        }
        let symbols: Vec<String> = securities.iter().map(|s| s.ticker.clone()).collect();
        let board = market::board(cx, &symbols);
        let assets = market::assets(cx);
        let cal = market::calendar(cx.hub, cx.alpaca);
        let (session, _) = market::sessions(&cal, mt_core::time::now_utc());
        let bars = cx
            .hub
            .watch(&cx.alpaca.bars(&symbols, Timeframe::Min15, session, None));

        let mut rows: Vec<(Row, String)> = symbols
            .iter()
            .map(|s| {
                (
                    board.row(s),
                    market::name_of(&assets, s).unwrap_or_default(),
                )
            })
            .collect();
        if self.sort.column < COLUMNS.len() {
            let sort = self.sort;
            rows.sort_by(|(a, an), (b, bn)| match sort.column {
                0 => sort.apply(a.symbol.cmp(&b.symbol)),
                1 => sort.apply(an.cmp(bn)),
                2 => cmp_opt(a.last, b.last, &sort),
                3 => cmp_opt(a.change, b.change, &sort),
                4 => cmp_opt(a.change_pct, b.change_pct, &sort),
                5 => cmp_opt(
                    a.quote.as_ref().map(|q| q.bid),
                    b.quote.as_ref().map(|q| q.bid),
                    &sort,
                ),
                6 => cmp_opt(
                    a.quote.as_ref().map(|q| q.ask),
                    b.quote.as_ref().map(|q| q.ask),
                    &sort,
                ),
                7 => cmp_opt(a.volume, b.volume, &sort),
                10 => cmp_opt(
                    a.last_time.map(|t| t.timestamp() as f64),
                    b.last_time.map(|t| t.timestamp() as f64),
                    &sort,
                ),
                _ => std::cmp::Ordering::Equal,
            });
        }

        let ticking = rows.iter().filter(|(r, _)| r.ticking).count();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} securities · prices {} · {ticking} streaming every trade, the rest by the minute",
                    rows.len(),
                    cx.alpaca.feed().label()
                ))
                .small()
                .color(skin.text_muted),
            );
            if let Some(n) = board.notice() {
                ui.label(RichText::new(format!("⚠ {n}")).small().color(skin.warning));
            }
            csv::copy_button(ui, skin, || {
                csv::to_csv(
                    &["security", "name", "last", "change", "change_pct", "bid", "ask", "volume", "low", "high", "last_trade_utc"],
                    rows.iter().map(|(r, name)| {
                        let n = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v}"));
                        vec![
                            format!("{} US", r.symbol),
                            name.clone(),
                            n(r.last),
                            n(r.change),
                            n(r.change_pct),
                            n(r.quote.as_ref().map(|q| q.bid)),
                            n(r.quote.as_ref().map(|q| q.ask)),
                            n(r.volume),
                            n(r.low),
                            n(r.high),
                            r.last_time.map_or_else(String::new, |t| t.to_rfc3339()),
                        ]
                    }),
                )
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::freshness(ui, skin, &board.snapshots)
            });
        });
        if board.snapshots.data.is_none() {
            widgets::placeholder(
                ui,
                skin,
                board.snapshots.error.as_ref().map(ToString::to_string),
            );
            return;
        }

        let row_h = 22.0;
        let mut open = None;
        let mut sort = self.sort;
        // Leave room for the notes underneath; the rows scroll.
        let table_h = (ui.available_height() - 48.0).max(row_h * 4.0);
        egui::ScrollArea::horizontal().show(ui, |ui| {
            TableBuilder::new(ui)
                .id_salt("q-table")
                .striped(true)
                .max_scroll_height(table_h)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(84.0).at_least(70.0))
                .column(Column::initial(190.0).at_least(80.0).clip(true))
                .columns(Column::initial(72.0).at_least(60.0), 6)
                .column(Column::initial(118.0).at_least(90.0))
                .column(Column::initial(130.0))
                .column(Column::initial(76.0))
                .header(row_h, |mut h| {
                    for (i, (name, numeric)) in COLUMNS.iter().enumerate() {
                        h.col(|ui| sort_header(ui, skin, name, i, *numeric, &mut sort));
                    }
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let (r, name) = &rows[row.index()];
                        row.col(|ui| {
                            let resp = widgets::link(ui, skin, &format!("{} US", r.symbol));
                            if resp.clicked() {
                                open = Some(Route::new("GP", [format!("{} US", r.symbol)]));
                            }
                            if r.ticking {
                                ui.label(RichText::new("•").color(skin.live))
                                    .on_hover_text("Every trade and quote streams");
                            }
                        });
                        row.col(|ui| {
                            let resp = ui.add(
                                egui::Label::new(RichText::new(name).color(skin.text_muted))
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            if resp.on_hover_text("DES: description").clicked() {
                                open = Some(Route::new("DES", [format!("{} US", r.symbol)]));
                            }
                        });
                        let delta = |v: Option<f64>, text: String| {
                            RichText::new(text).color(v.map_or(skin.text_muted, |v| skin.delta(v)))
                        };
                        row.col(|ui| {
                            num_cell(
                                ui,
                                RichText::new(fmt::price_opt(r.last)).color(skin.text_strong),
                            )
                        });
                        row.col(|ui| num_cell(ui, delta(r.change, fmt::change_opt(r.change))));
                        row.col(|ui| num_cell(ui, delta(r.change_pct, fmt::pct_opt(r.change_pct))));
                        let q = r.quote.as_ref().filter(|q| q.is_two_sided());
                        row.col(|ui| num_cell(ui, fmt::price_opt(q.map(|q| q.bid))));
                        row.col(|ui| num_cell(ui, fmt::price_opt(q.map(|q| q.ask))));
                        row.col(|ui| num_cell(ui, fmt::volume_opt(r.volume)));
                        row.col(|ui| {
                            num_cell(
                                ui,
                                RichText::new(match (r.low, r.high) {
                                    (Some(l), Some(h)) => {
                                        format!("{}–{}", fmt::price(l), fmt::price(h))
                                    }
                                    _ => fmt::DASH.into(),
                                })
                                .color(skin.text_muted),
                            );
                        });
                        row.col(|ui| {
                            let closes: Vec<f32> = bars
                                .data()
                                .map(|b| b.get(&r.symbol).iter().map(|b| b.close as f32).collect())
                                .unwrap_or_default();
                            let color = r.change.map_or(skin.series(0), |c| skin.delta(c));
                            widgets::sparkline(ui, &closes, color, egui::vec2(120.0, 16.0));
                        });
                        row.col(|ui| {
                            num_cell(
                                ui,
                                RichText::new(
                                    r.last_time.map_or_else(|| fmt::DASH.into(), fmt::time),
                                )
                                .monospace()
                                .color(skin.text_muted),
                            );
                        });
                    });
                });
        });
        self.sort = sort;
        if let Some(route) = open {
            cx.open(route);
        }
        ui.label(
            RichText::new(match cx.alpaca.feed() {
                mt_core::equity::Feed::Iex => {
                    "IEX carries a few percent of US volume, so prices of thinly traded names can lag; \
                     volume is IEX's. Changes are from the previous close. SET switches to every exchange, 15 minutes late."
                }
                mt_core::equity::Feed::DelayedSip => {
                    "Every exchange, 15 minutes late. Changes are from the previous close."
                }
                mt_core::equity::Feed::Sip => "Every exchange, real time. Changes are from the previous close.",
            })
            .small()
            .color(skin.text_muted),
        );
        if let Source::Custom(list) = &self.source
            && list.len() > 1
            && ui.small_button("Add all to watchlist").clicked()
        {
            for s in list {
                cx.send(AppCommand::AddFavorite(s.to_string()));
            }
        }
    }
}
