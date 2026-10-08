//! PORT: the paper account's portfolio. Equity, the day's P&L, cash and
//! buying power on top; stock and ETF positions with their cost, value and
//! P&L (today's and since entry), moving with the quote stream between
//! Alpaca's minute re-reads; options grouped by underlying with each
//! underlying's net delta in shares.

use egui::{Grid, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::account::{net_delta, options_by_underlying, to_f64};
use mt_core::exchange;
use mt_core::money::Decimal;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market;
use crate::portfolio::{self, Line, Totals};
use crate::widgets::table::{Sort, cmp_opt, num_cell, sort_header};
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "PORT",
    aliases: &["POS", "POSITIONS", "PRTU"],
    name: "Portfolio",
    category: Category::Account,
    usage: "PORT",
    description: "The Alpaca paper account's positions: quantity, average cost, market value, the day's and unrealized P&L, kept live by the quote stream; cash, buying power and equity; options grouped by underlying with net delta. Right-click a position for a ticket to buy, sell or close it.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Portfolio {
        sort: Sort::new(4, true),
    }))
}

struct Portfolio {
    sort: Sort,
}

const COLUMNS: [(&str, bool); 10] = [
    ("Security", false),
    ("Qty", true),
    ("Avg cost", true),
    ("Last", true),
    ("Mkt value", true),
    ("Weight", true),
    ("Day P&L", true),
    ("Day %", true),
    ("Unrealized", true),
    ("Unr. %", true),
];

fn opt_f(v: Option<Decimal>) -> Option<f64> {
    v.map(to_f64)
}

impl Panel for Portfolio {
    fn title(&self) -> String {
        "PORT".into()
    }

    fn route(&self) -> Route {
        Route::code("PORT")
    }

    fn absorb(&mut self, _args: &[String]) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let mode = cx.alpaca.mode();
        widgets::title_bar(
            ui,
            skin,
            &format!("Portfolio · Alpaca {} account", mode.name()),
            |ui| {
                if cx.alpaca.is_ready() {
                    market::status_label(ui, cx);
                }
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let book = portfolio::watch(cx);
        let lines = book.lines();
        let totals = book.totals(&lines);
        let Some(account) = book.account.data() else {
            widgets::placeholder(
                ui,
                skin,
                book.account.error.as_ref().map(ToString::to_string),
            );
            return;
        };
        let Some(t) = totals else { return };

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                tiles(ui, cx, account, &t);
                ui.horizontal_wrapped(|ui| {
                    let stocks = lines.iter().filter(|l| !l.position.is_option()).count();
                    let options = lines.len() - stocks;
                    let mut info = format!(
                        "{stocks} stock and ETF positions · {options} option positions · prices {}",
                        cx.alpaca.feed().label()
                    );
                    if t.live > 0 {
                        info.push_str(&format!(" · {} moved by the stream since Alpaca's last valuation", t.live));
                    }
                    ui.label(RichText::new(info).small().color(skin.text_muted));
                    if let Some(n) = book.stream_notice() {
                        ui.label(RichText::new(format!("⚠ {n}")).small().color(skin.warning));
                    }
                    csv::copy_button(ui, skin, || to_csv(&lines));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::freshness(ui, skin, &book.positions);
                    });
                });
                if book.positions.data.is_none() {
                    widgets::placeholder(
                        ui,
                        skin,
                        book.positions.error.as_ref().map(ToString::to_string),
                    );
                    return;
                }
                if lines.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("No open positions in the paper account.")
                            .color(skin.text_muted),
                    );
                    return;
                }
                widgets::section(ui, skin, "Stocks and ETFs");
                self.stocks(ui, cx, &lines, &t);
                options(ui, cx, &lines);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "From Alpaca, re-read every minute and at once after each order event; between \
                         re-reads stock prices move with the live stream. Day P&L is from the previous \
                         close (from the entry price for positions opened today). Options are valued at \
                         Alpaca's price; greeks come from its indicative feed.",
                    )
                    .small()
                    .color(skin.text_muted),
                );
            });
    }
}

/// A position's right-click menu: a ticket to buy, sell, or close it.
fn trade_menu(ui: &mut Ui, security: &str, held: Decimal) -> Option<Route> {
    let mut out = None;
    if ui.button("Buy…").clicked() {
        out = Some(Route::new("BUY", [security]));
    }
    if ui.button("Sell…").clicked() {
        out = Some(Route::new("SELL", [security]));
    }
    let (code, verb) = if held.is_sign_negative() {
        ("BUY", "Cover")
    } else {
        ("SELL", "Close")
    };
    let qty = portfolio::qty(held.abs());
    if ui
        .button(format!("{verb} the position ({qty} shares)…"))
        .on_hover_text(
            "Opens a ticket for the whole position; nothing is sent until you confirm it",
        )
        .clicked()
    {
        out = Some(Route::new(
            code,
            [security.to_owned(), qty.replace(',', "")],
        ));
    }
    if out.is_some() {
        ui.close();
    }
    out
}

/// An option position's right-click menu: tickets to buy or sell the
/// contract, or to close the position.
fn option_menu(ui: &mut Ui, symbol: &str, held: Decimal) -> Option<Route> {
    let mut out = None;
    if ui.button("Buy…").clicked() {
        out = Some(Route::new("BUY", [symbol]));
    }
    if ui.button("Sell…").clicked() {
        out = Some(Route::new("SELL", [symbol]));
    }
    let code = if held.is_sign_negative() {
        "BUY"
    } else {
        "SELL"
    };
    let qty = portfolio::qty(held.abs());
    if ui
        .button(format!("Close the position ({qty} contracts)…"))
        .on_hover_text(
            "Opens a ticket for the whole position, at the mid; nothing is sent until you confirm it",
        )
        .clicked()
    {
        out = Some(Route::new(
            code,
            [symbol.to_owned(), qty.replace(',', "")],
        ));
    }
    if ui.button("Show in OMON").clicked() {
        out = Some(Route::new("OMON", [symbol]));
    }
    if out.is_some() {
        ui.close();
    }
    out
}

fn tiles(ui: &mut Ui, cx: &mut PanelCx<'_>, a: &mt_core::account::Account, t: &Totals) {
    let skin = cx.skin;
    ui.horizontal_wrapped(|ui| {
        widgets::stat_tile(
            ui,
            skin,
            "Equity",
            &portfolio::usd(t.equity),
            Some(
                RichText::new(format!("previous close {}", portfolio::usd(a.last_equity)))
                    .color(skin.text_muted),
            ),
        );
        widgets::stat_tile(
            ui,
            skin,
            "Day P&L",
            &portfolio::usd_signed(t.day_pl),
            Some(
                RichText::new(portfolio::pct_signed(t.day_pct)).color(skin.delta(to_f64(t.day_pl))),
            ),
        );
        let unr_pct = (!t.cost.is_zero()).then(|| to_f64(t.unrealized) / to_f64(t.cost));
        widgets::stat_tile(
            ui,
            skin,
            "Unrealized P&L",
            &portfolio::usd_signed(t.unrealized),
            Some(
                RichText::new(format!("{} on cost", portfolio::pct_signed(unr_pct)))
                    .color(skin.delta(to_f64(t.unrealized))),
            ),
        );
        widgets::stat_tile(ui, skin, "Cash", &portfolio::usd(a.cash), None);
        widgets::stat_tile(
            ui,
            skin,
            "Buying power",
            &portfolio::usd(a.buying_power),
            a.non_marginable_buying_power.map(|n| {
                RichText::new(format!("non-marginable {}", portfolio::usd(n)))
                    .color(skin.text_muted)
            }),
        )
        .on_hover_text("ACCT for margin and day trading");
        widgets::stat_tile(
            ui,
            skin,
            "Long · short",
            &portfolio::usd(t.long_value),
            Some(
                RichText::new(format!("short {}", portfolio::usd(t.short_value)))
                    .color(skin.text_muted),
            ),
        );
    });
}

impl Portfolio {
    fn stocks(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, lines: &[Line], t: &Totals) {
        let skin = cx.skin;
        let mut rows: Vec<&Line> = lines.iter().filter(|l| !l.position.is_option()).collect();
        if rows.is_empty() {
            ui.label(RichText::new("None.").color(skin.text_muted));
            return;
        }
        let weight = |l: &Line| {
            (!t.equity.is_zero()).then(|| to_f64(l.mark.market_value) / to_f64(t.equity))
        };
        let sort = self.sort;
        rows.sort_by(|a, b| {
            let (pa, pb) = (&a.position, &b.position);
            match sort.column {
                0 => sort.apply(pa.symbol.cmp(&pb.symbol)),
                1 => sort.apply(pa.qty.cmp(&pb.qty)),
                2 => sort.apply(pa.avg_entry_price.cmp(&pb.avg_entry_price)),
                3 => sort.apply(a.mark.price.cmp(&b.mark.price)),
                4 | 5 => sort.apply(a.mark.market_value.abs().cmp(&b.mark.market_value.abs())),
                6 => cmp_opt(opt_f(a.mark.day_pl), opt_f(b.mark.day_pl), &sort),
                7 => cmp_opt(a.mark.day_pct, b.mark.day_pct, &sort),
                8 => cmp_opt(
                    opt_f(a.mark.unrealized_pl),
                    opt_f(b.mark.unrealized_pl),
                    &sort,
                ),
                9 => cmp_opt(a.mark.unrealized_pct, b.mark.unrealized_pct, &sort),
                _ => std::cmp::Ordering::Equal,
            }
        });
        let row_h = 22.0;
        let mut open = None;
        let mut sort = self.sort;
        egui::ScrollArea::horizontal()
            .id_salt("port-stocks-h")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("port-stocks")
                    .striped(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(96.0).at_least(80.0))
                    .columns(Column::initial(70.0).at_least(56.0), 3)
                    .column(Column::initial(100.0).at_least(80.0))
                    .column(Column::initial(64.0).at_least(52.0))
                    .column(Column::initial(92.0).at_least(72.0))
                    .column(Column::initial(66.0).at_least(56.0))
                    .column(Column::initial(96.0).at_least(72.0))
                    .column(Column::initial(66.0).at_least(56.0))
                    .header(row_h, |mut h| {
                        for (i, (name, numeric)) in COLUMNS.iter().enumerate() {
                            h.col(|ui| sort_header(ui, skin, name, i, *numeric, &mut sort));
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, rows.len(), |mut row| {
                            let l = rows[row.index()];
                            let (p, m) = (&l.position, &l.mark);
                            row.col(|ui| {
                                let name = format!("{} US", p.symbol);
                                let resp = widgets::link(ui, skin, &name)
                                    .on_hover_text("GP: chart · right-click to trade");
                                if resp.clicked() {
                                    open = Some(Route::new("GP", [name.clone()]));
                                }
                                resp.context_menu(|ui| {
                                    if let Some(r) = trade_menu(ui, &name, p.qty) {
                                        open = Some(r);
                                    }
                                });
                                if p.is_short() {
                                    ui.label(RichText::new("short").small().color(skin.warning));
                                }
                            });
                            row.col(|ui| num_cell(ui, portfolio::qty(p.qty)));
                            row.col(|ui| {
                                num_cell(
                                    ui,
                                    RichText::new(portfolio::price(p.avg_entry_price))
                                        .color(skin.text_muted),
                                );
                            });
                            row.col(|ui| {
                                let mut text = RichText::new(portfolio::price(m.price))
                                    .color(skin.text_strong);
                                if m.live {
                                    text = text.color(skin.live);
                                }
                                num_cell(ui, text);
                            });
                            row.col(|ui| num_cell(ui, portfolio::usd(m.market_value)));
                            row.col(|ui| {
                                num_cell(
                                    ui,
                                    RichText::new(weight(l).map_or_else(
                                        || market::fmt::DASH.to_owned(),
                                        |w| format!("{:.1}%", w * 100.0),
                                    ))
                                    .color(skin.text_muted),
                                );
                            });
                            row.col(|ui| {
                                num_cell(
                                    ui,
                                    RichText::new(m.day_pl.map_or_else(
                                        || market::fmt::DASH.to_owned(),
                                        portfolio::usd_signed,
                                    ))
                                    .color(portfolio::delta_color(skin, m.day_pl)),
                                );
                            });
                            row.col(|ui| {
                                num_cell(
                                    ui,
                                    RichText::new(portfolio::pct_signed(m.day_pct))
                                        .color(portfolio::delta_color(skin, m.day_pl)),
                                );
                            });
                            row.col(|ui| {
                                num_cell(
                                    ui,
                                    RichText::new(m.unrealized_pl.map_or_else(
                                        || market::fmt::DASH.to_owned(),
                                        portfolio::usd_signed,
                                    ))
                                    .color(portfolio::delta_color(skin, m.unrealized_pl)),
                                );
                            });
                            row.col(|ui| {
                                num_cell(
                                    ui,
                                    RichText::new(portfolio::pct_signed(m.unrealized_pct))
                                        .color(portfolio::delta_color(skin, m.unrealized_pl)),
                                );
                            });
                        });
                    });
            });
        self.sort = sort;
        if let Some(r) = open {
            cx.open(r);
        }
    }
}

/// Options by underlying: each contract, and the underlying's net delta.
fn options(ui: &mut Ui, cx: &mut PanelCx<'_>, lines: &[Line]) {
    let skin = cx.skin;
    let positions: Vec<mt_core::account::Position> =
        lines.iter().map(|l| l.position.clone()).collect();
    let groups = options_by_underlying(&positions);
    if groups.is_empty() {
        return;
    }
    let symbols: Vec<String> = groups
        .values()
        .flatten()
        .map(|p| p.symbol.clone())
        .collect();
    let snaps = cx.hub.watch(&cx.alpaca.option_snapshots(&symbols));
    widgets::section(
        ui,
        skin,
        &format!("Options · greeks {}", mt_alpaca::OPTION_FEED_LABEL),
    );
    let today = exchange::now_exchange().date_naive();
    let mut open = None;
    for (underlying, contracts) in &groups {
        let shares: Decimal = positions
            .iter()
            .filter(|p| !p.is_option() && &p.symbol == underlying)
            .map(|p| p.qty)
            .sum();
        let delta_of = |sym: &str| {
            snaps
                .data()
                .and_then(|s| s.get(sym))
                .and_then(|s| s.greeks.delta)
        };
        let net = net_delta(
            shares,
            contracts.iter().map(|p| (p.qty, delta_of(&p.symbol))),
        );
        ui.horizontal_wrapped(|ui| {
            let name = format!("{underlying} US");
            if widgets::link(ui, skin, &name).clicked() {
                open = Some(Route::new("GP", [name.clone()]));
            }
            if widgets::link(ui, skin, "OMON")
                .on_hover_text(format!("The option chain for {name}"))
                .clicked()
            {
                open = Some(Route::new("OMON", [name]));
            }
            ui.label(
                RichText::new(format!(
                    "{} shares · {} option position{}",
                    portfolio::qty(shares),
                    contracts.len(),
                    if contracts.len() == 1 { "" } else { "s" }
                ))
                .small()
                .color(skin.text_muted),
            );
            let net_text = net.map_or_else(
                || {
                    if snaps.loading {
                        "net delta: loading…".to_owned()
                    } else {
                        "net delta: greeks unavailable".to_owned()
                    }
                },
                |n| format!("net delta {n:+.0} shares"),
            );
            ui.label(
                RichText::new(net_text)
                    .strong()
                    .color(net.map_or(skin.text_muted, |n| skin.delta(n))),
            )
            .on_hover_text(
                "Shares held plus each option's contracts × 100 × its delta: \
                 how many shares the group moves like.",
            );
        });
        Grid::new(("port-options", underlying))
            .striped(true)
            .num_columns(11)
            .spacing([14.0, 3.0])
            .show(ui, |ui| {
                for h in [
                    "Contract",
                    "Expiry",
                    "Qty",
                    "Avg",
                    "Mark",
                    "Mkt value",
                    "Day P&L",
                    "Unrealized",
                    "Delta",
                    "IV",
                    "Delta (sh)",
                ] {
                    widgets::label(ui, skin, h);
                }
                ui.end_row();
                for p in contracts {
                    let Some(c) = p.contract() else { continue };
                    let Some(l) = lines.iter().find(|l| l.position.symbol == p.symbol) else {
                        continue;
                    };
                    let m = &l.mark;
                    let snap = snaps.data().and_then(|s| s.get(&p.symbol));
                    let resp = ui
                        .add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} {} {}",
                                    c.underlying,
                                    portfolio::strike(c.strike),
                                    if c.right == mt_core::instrument::OptionRight::Call {
                                        "call"
                                    } else {
                                        "put"
                                    }
                                ))
                                .color(skin.text_strong),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text(format!("{}: right-click to trade it", p.symbol));
                    resp.context_menu(|ui| {
                        if let Some(r) = option_menu(ui, &p.symbol, p.qty) {
                            open = Some(r);
                        }
                    });
                    let days = (c.expiry - today).num_days();
                    let (when, color) = match days {
                        ..=-1 => ("expired".to_owned(), skin.warning),
                        0 => ("expires today".to_owned(), skin.warning),
                        1..=7 => (
                            format!("{} · {days}d", c.expiry.format("%b %d")),
                            skin.warning,
                        ),
                        _ => (c.expiry.format("%b %d %Y").to_string(), skin.text_muted),
                    };
                    ui.label(RichText::new(when).color(color));
                    ui.label(portfolio::qty(p.qty));
                    ui.label(
                        RichText::new(portfolio::price(p.avg_entry_price)).color(skin.text_muted),
                    );
                    ui.label(portfolio::price(m.price));
                    ui.label(portfolio::usd(m.market_value));
                    ui.label(
                        RichText::new(
                            m.day_pl.map_or_else(
                                || market::fmt::DASH.to_owned(),
                                portfolio::usd_signed,
                            ),
                        )
                        .color(portfolio::delta_color(skin, m.day_pl)),
                    );
                    ui.label(
                        RichText::new(
                            m.unrealized_pl.map_or_else(
                                || market::fmt::DASH.to_owned(),
                                portfolio::usd_signed,
                            ),
                        )
                        .color(portfolio::delta_color(skin, m.unrealized_pl)),
                    );
                    let delta = snap.and_then(|s| s.greeks.delta);
                    ui.label(
                        delta.map_or_else(|| market::fmt::DASH.to_owned(), |d| format!("{d:+.2}")),
                    );
                    ui.label(
                        RichText::new(snap.and_then(|s| s.implied_volatility).map_or_else(
                            || market::fmt::DASH.to_owned(),
                            |v| format!("{:.1}%", v * 100.0),
                        ))
                        .color(skin.text_muted),
                    );
                    ui.label(delta.map_or_else(
                        || market::fmt::DASH.to_owned(),
                        |d| format!("{:+.0}", to_f64(p.qty) * 100.0 * d),
                    ));
                    ui.end_row();
                }
            });
        let expiring: Vec<String> = contracts
            .iter()
            .filter_map(|p| p.contract())
            .filter(|c| c.expiry == today)
            .map(|c| mt_core::options::contract_name(&c))
            .collect();
        if !expiring.is_empty() {
            ui.label(
                RichText::new(format!(
                    "⚠ {} {} today. Alpaca takes orders for {underlying}'s expiring contracts until {} \
                     New York time, exercises those in the money by a cent or more at the close, and \
                     may sell a position the account cannot afford to exercise in the last hour.",
                    expiring.join(", "),
                    if expiring.len() == 1 { "expires" } else { "expire" },
                    mt_core::options::expiry_cutoff(underlying).format("%H:%M")
                ))
                .color(skin.warning),
            );
        }
        ui.add_space(6.0);
    }
    if let Some(r) = open {
        cx.open(r);
    }
}

fn to_csv(lines: &[Line]) -> String {
    let n = |v: Option<Decimal>| v.map_or_else(String::new, |v| v.normalize().to_string());
    let f = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v}"));
    csv::to_csv(
        &[
            "symbol",
            "class",
            "qty",
            "avg_entry_price",
            "price",
            "market_value",
            "cost_basis",
            "day_pl",
            "day_pct",
            "unrealized_pl",
            "unrealized_pct",
        ],
        lines.iter().map(|l| {
            let (p, m) = (&l.position, &l.mark);
            vec![
                p.symbol.clone(),
                if p.is_option() { "option" } else { "equity" }.to_owned(),
                n(Some(p.qty)),
                n(Some(p.avg_entry_price)),
                n(Some(m.price)),
                n(Some(m.market_value)),
                n(Some(p.cost_basis)),
                n(m.day_pl),
                f(m.day_pct),
                n(m.unrealized_pl),
                f(m.unrealized_pct),
            ]
        }),
    )
}
