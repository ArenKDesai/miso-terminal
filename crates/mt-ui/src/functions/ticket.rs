//! BUY and SELL: an order ticket for a stock or ETF in the paper account.
//!
//! `BUY XLU US 10 LMT 82.50 DAY` opens a ticket filled in from the command;
//! nothing is sent until its **Confirm** button is clicked. The ticket shows
//! the latest prices, the order's value, buying power and the position
//! afterwards, and every guardrail's verdict (`mt_core::guard`): warnings must
//! be ticked off and blocks stop it. Commands from outside the window
//! (`--run`, another launch, hotkeys) can only open a ticket, never send one.
//!
//! A ticket keeps one `client_order_id` until its order is placed, so
//! sending again after a lost answer can never place it twice (see
//! `mt_alpaca::OrderDesk`). Tickets are not restored after a restart.

use egui::{Grid, RichText, Ui};
use mt_alpaca::Outcome;
use mt_core::account::OrderSide;
use mt_core::guard::{self, Level};
use mt_core::money::{Decimal, RoundingStrategy, parse_decimal, round_to_tick};
use mt_core::order::{
    OrderRequest, OrderType, TimeInForce, day_value, is_day_trade, price_text, tick_for,
};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, fmt};
use crate::portfolio;
use crate::trading;
use crate::widgets;

const USAGE_TAIL: &str = "<ticker> US [qty] [MKT | LMT <price> | STP <price> | STPLMT <stop> <limit>] [DAY | GTC | IOC | FOK | OPG | CLS] [EXT]";

pub const BUY: FunctionSpec = FunctionSpec {
    code: "BUY",
    aliases: &[],
    name: "Buy ticket",
    category: Category::Account,
    usage: "BUY <ticker> US [qty] [MKT | LMT <price> | STP <price> | STPLMT <stop> <limit>] [DAY | GTC | IOC | FOK | OPG | CLS] [EXT]",
    description: "An order ticket to buy a stock or ETF in the Alpaca paper account, filled in from the command (BUY XLU US 10 LMT 82.50 DAY): the cost, buying power and position afterwards, and every guardrail. Only its Confirm button sends the order.",
    takes_node: false,
    takes_security: true,
    takes_option: false,
    open: open_buy,
};

pub const SELL: FunctionSpec = FunctionSpec {
    code: "SELL",
    aliases: &[],
    name: "Sell ticket",
    category: Category::Account,
    usage: "SELL <ticker> US [qty] [MKT | LMT <price> | STP <price> | STPLMT <stop> <limit>] [DAY | GTC | IOC | FOK | OPG | CLS] [EXT]",
    description: "An order ticket to sell (or sell short) a stock or ETF in the Alpaca paper account, filled in from the command (SELL XLU US 10): the proceeds, the position afterwards and every guardrail. Only its Confirm button sends the order.",
    takes_node: false,
    takes_security: true,
    takes_option: false,
    open: open_sell,
};

fn open_buy(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Ticket::new(parse_args(OrderSide::Buy, args)?)))
}

fn open_sell(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Ticket::new(parse_args(OrderSide::Sell, args)?)))
}

/// A ticket's fields as typed (text, so half-typed numbers survive).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Draft {
    pub side: OrderSide,
    /// The ticker, `XLU`.
    pub symbol: String,
    pub qty: String,
    pub order_type: OrderType,
    pub limit: String,
    pub stop: String,
    pub tif: TimeInForce,
    pub extended: bool,
}

impl Draft {
    fn new(side: OrderSide) -> Self {
        Self {
            side,
            symbol: String::new(),
            qty: String::new(),
            order_type: OrderType::Limit,
            limit: String::new(),
            stop: String::new(),
            tif: TimeInForce::Day,
            extended: false,
        }
    }

    fn code(&self) -> &'static str {
        match self.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }
    }

    /// The command that opens this ticket again: `BUY XLU US 10 LMT 82.50 DAY`.
    pub(crate) fn route(&self) -> Route {
        let mut args = Vec::new();
        if let Some(t) = mt_alpaca::normalize_symbol(&self.symbol) {
            args.push(format!("{t} US"));
        }
        let qty = self.qty.trim();
        if parse_decimal(qty).is_some() {
            args.push(qty.to_owned());
        }
        let (limit, stop) = (self.limit.trim(), self.stop.trim());
        match self.order_type {
            OrderType::Market => args.push("MKT".into()),
            OrderType::StopLimit if is_number(stop) && is_number(limit) => {
                args.extend(["STPLMT".into(), stop.to_owned(), limit.to_owned()]);
            }
            OrderType::Stop if is_number(stop) => {
                args.extend(["STP".into(), stop.to_owned()]);
            }
            OrderType::Limit if is_number(limit) => {
                args.extend(["LMT".into(), limit.to_owned()]);
            }
            // A type without its price yet reopens as a limit ticket.
            _ => {}
        }
        args.push(self.tif.code().into());
        if self.extended {
            args.push("EXT".into());
        }
        Route::new(self.code(), args)
    }

    /// The order the fields describe, or what is missing.
    pub(crate) fn request(&self, client_order_id: &str) -> Result<OrderRequest, String> {
        let symbol = mt_alpaca::normalize_symbol(&self.symbol)
            .ok_or_else(|| "Type the ticker (XLU).".to_owned())?;
        let qty = parse_decimal(&self.qty)
            .filter(|q| *q > Decimal::ZERO)
            .ok_or_else(|| "Type the number of shares.".to_owned())?;
        let price = |text: &str, what: &str| {
            parse_decimal(text)
                .filter(|p| *p > Decimal::ZERO)
                .ok_or_else(|| format!("Type the {what} price."))
        };
        let limit = if self.order_type.needs_limit() {
            Some(price(&self.limit, "limit")?)
        } else {
            None
        };
        let stop = if self.order_type.needs_stop() {
            Some(price(&self.stop, "stop")?)
        } else {
            None
        };
        Ok(OrderRequest {
            client_order_id: client_order_id.to_owned(),
            symbol,
            side: self.side,
            qty,
            order_type: self.order_type,
            limit_price: limit,
            stop_price: stop,
            tif: self.tif,
            extended_hours: self.extended,
        })
    }
}

fn is_number(s: &str) -> bool {
    parse_decimal(s).is_some_and(|d| d >= Decimal::ZERO)
}

/// Read a ticket command's arguments (after the code): the security, then in
/// any order a quantity (the first bare number), an order type with its
/// prices (`MKT`, `LMT 82.50`, `STP 80`, `STPLMT 80 79.50`, or `@82.50` for a
/// limit), a time in force and `EXT`.
pub(crate) fn parse_args(side: OrderSide, args: &[String]) -> Result<Draft, String> {
    let mut d = Draft::new(side);
    let mut kind: Option<OrderType> = None;
    let tokens: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut i = 0;
    let next_price = |i: &mut usize, what: &str| -> Result<String, String> {
        match tokens.get(*i + 1) {
            Some(t) if is_number(t.trim_start_matches('@')) => {
                *i += 1;
                Ok(t.trim_start_matches('@').to_owned())
            }
            _ => Err(format!("{what} needs a price after it.")),
        }
    };
    while i < tokens.len() {
        let t = tokens[i];
        let up = t.to_ascii_uppercase();
        if let Some(sec) = market::security_of(t) {
            if !d.symbol.is_empty() {
                return Err(format!("One security per ticket ({} and {sec}).", d.symbol));
            }
            d.symbol = sec.ticker;
        } else if mt_core::instrument::OptionContract::parse_occ(t).is_some() {
            return Err("Options tickets arrive with OMON (markets phase 5).".into());
        } else if let Some(tif) = TimeInForce::parse(t) {
            d.tif = tif;
        } else if matches!(up.as_str(), "EXT" | "EXTENDED") {
            d.extended = true;
        } else if let Some(p) = t.strip_prefix('@') {
            let p = if p.is_empty() {
                next_price(&mut i, "@")?
            } else if is_number(p) {
                p.to_owned()
            } else {
                return Err(format!("{t:?} is not a price."));
            };
            d.limit = p;
            kind = Some(match kind {
                Some(OrderType::Stop | OrderType::StopLimit) => OrderType::StopLimit,
                _ => OrderType::Limit,
            });
        } else if let Some(k) = OrderType::parse(t) {
            match k {
                OrderType::Market => kind = Some(OrderType::Market),
                OrderType::Limit => {
                    d.limit = next_price(&mut i, "LMT")?;
                    kind = Some(match kind {
                        Some(OrderType::Stop | OrderType::StopLimit) => OrderType::StopLimit,
                        _ => OrderType::Limit,
                    });
                }
                OrderType::Stop => {
                    d.stop = next_price(&mut i, "STP")?;
                    kind = Some(match kind {
                        Some(OrderType::Limit | OrderType::StopLimit) => OrderType::StopLimit,
                        _ => OrderType::Stop,
                    });
                }
                OrderType::StopLimit => {
                    d.stop = next_price(&mut i, "STPLMT")?;
                    d.limit = next_price(&mut i, "STPLMT")?;
                    kind = Some(OrderType::StopLimit);
                }
                OrderType::TrailingStop => {
                    return Err("Tickets do not place trailing stops.".into());
                }
            }
        } else if is_number(t) && d.qty.is_empty() {
            d.qty = t.to_owned();
        } else if is_number(t) && d.limit.is_empty() && kind.is_none() {
            // `BUY XLU US 10 82.50`: a bare second number is a limit price.
            d.limit = t.to_owned();
            kind = Some(OrderType::Limit);
        } else {
            return Err(format!(
                "{t:?} is not part of a ticket. Usage: {} {USAGE_TAIL}",
                d.code()
            ));
        }
        i += 1;
    }
    d.order_type = kind.unwrap_or(OrderType::Limit);
    Ok(d)
}

struct Ticket {
    draft: Draft,
    /// Fill the limit from the last price once it is known (nothing typed yet).
    prefill: bool,
    /// The order the user ticked the warnings for (its description).
    acknowledged: Option<String>,
    /// The id the next Confirm sends under. It changes only with *New order*,
    /// after the order was placed.
    client_order_id: String,
    /// A message from the desk that refused to send (already on its way…).
    message: Option<String>,
}

impl Ticket {
    fn new(draft: Draft) -> Self {
        Self {
            prefill: draft.order_type.needs_limit() && draft.limit.trim().is_empty(),
            draft,
            acknowledged: None,
            client_order_id: mt_alpaca::new_client_order_id(),
            message: None,
        }
    }

    fn new_order(&mut self) {
        self.client_order_id = mt_alpaca::new_client_order_id();
        self.acknowledged = None;
        self.message = None;
    }
}

impl Panel for Ticket {
    fn title(&self) -> String {
        match mt_alpaca::normalize_symbol(&self.draft.symbol) {
            Some(t) => format!("{} {t}", self.draft.code()),
            None => self.draft.code().to_owned(),
        }
    }

    fn route(&self) -> Route {
        self.draft.route()
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let mode = cx.alpaca.mode();
        let heading = match self.draft.side {
            OrderSide::Buy => "Buy",
            OrderSide::Sell => "Sell",
        };
        widgets::title_bar(
            ui,
            skin,
            &format!("{heading} ticket · Alpaca {} account", mode.name()),
            |ui| {
                if cx.alpaca.is_ready() {
                    market::status_label(ui, cx);
                }
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let outcome = cx.desk.outcome(&self.client_order_id);
        // Once placed (or while its fate is open), the fields stay as sent.
        let locked = outcome.as_ref().is_some_and(|o| !o.may_send());

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.add_enabled_ui(!locked, |ui| self.fields(ui, cx));
                let symbol = mt_alpaca::normalize_symbol(&self.draft.symbol);
                let Some(symbol) = symbol else {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Type a ticker to see its price and the checks.")
                            .color(skin.text_muted),
                    );
                    return;
                };
                self.body(ui, cx, &symbol, outcome.as_ref(), locked);
            });
    }
}

impl Ticket {
    /// Side, security, quantity, type, prices, time in force, extended hours.
    fn fields(&mut self, ui: &mut Ui, cx: &PanelCx<'_>) {
        let skin = cx.skin;
        let d = &mut self.draft;
        let assets = market::assets(cx);
        ui.horizontal_wrapped(|ui| {
            for (side, label, color) in [
                (OrderSide::Buy, "Buy", skin.positive),
                (OrderSide::Sell, "Sell", skin.negative),
            ] {
                let text = RichText::new(label).strong();
                let text = if d.side == side {
                    text.color(color)
                } else {
                    text
                };
                if ui.selectable_label(d.side == side, text).clicked() {
                    d.side = side;
                }
            }
            ui.add_space(8.0);
            widgets::label(ui, skin, "Security");
            ui.add(
                egui::TextEdit::singleline(&mut d.symbol)
                    .hint_text("XLU")
                    .desired_width(70.0),
            );
            ui.label(RichText::new("US").color(skin.text_muted));
            if let Some(name) =
                mt_alpaca::normalize_symbol(&d.symbol).and_then(|t| market::name_of(&assets, &t))
            {
                ui.label(RichText::new(name).small().color(skin.text_muted));
            }
        });
        ui.horizontal_wrapped(|ui| {
            widgets::label(ui, skin, "Shares");
            ui.add(
                egui::TextEdit::singleline(&mut d.qty)
                    .hint_text("10")
                    .desired_width(70.0),
            );
            ui.add_space(8.0);
            widgets::label(ui, skin, "Type");
            egui::ComboBox::from_id_salt("ticket-type")
                .selected_text(d.order_type.label())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for t in OrderType::TICKET {
                        ui.selectable_value(&mut d.order_type, t, t.label());
                    }
                });
            if d.order_type.needs_stop() {
                widgets::label(ui, skin, "Stop");
                ui.add(egui::TextEdit::singleline(&mut d.stop).desired_width(70.0));
            }
            if d.order_type.needs_limit() {
                widgets::label(ui, skin, "Limit");
                if ui
                    .add(egui::TextEdit::singleline(&mut d.limit).desired_width(70.0))
                    .changed()
                {
                    self.prefill = false;
                }
            }
            ui.add_space(8.0);
            widgets::label(ui, skin, "Time in force");
            egui::ComboBox::from_id_salt("ticket-tif")
                .selected_text(d.tif.code())
                .width(64.0)
                .show_ui(ui, |ui| {
                    for t in TimeInForce::ALL {
                        ui.selectable_value(&mut d.tif, t, format!("{} · {}", t.code(), t.label()));
                    }
                });
            let can_extend = d.order_type == OrderType::Limit && d.tif == TimeInForce::Day;
            if !can_extend {
                d.extended = false;
            }
            ui.add_enabled(
                can_extend,
                egui::Checkbox::new(&mut d.extended, "Extended hours"),
            )
            .on_hover_text("Day limit orders may fill from 04:00 to 20:00 New York time");
        });
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        symbol: &str,
        outcome: Option<&Outcome>,
        locked: bool,
    ) {
        let skin = cx.skin;
        let account = cx.hub.watch(&cx.alpaca.account());
        let positions = cx.hub.watch(&cx.alpaca.positions());
        let book = trading::watch_orders(cx);
        let board = market::board(cx, &[symbol.to_owned()]);
        let row = board.row(symbol);
        let assets = market::assets(cx);
        let session = market::status(cx.hub, cx.alpaca).session;
        let exact = |v: Option<f64>| v.and_then(|v| mt_core::account::from_f64(v, 4));
        let last = exact(row.last);
        let (bid, ask) = row
            .quote
            .as_ref()
            .map_or((None, None), |q| (exact(Some(q.bid)), exact(Some(q.ask))));

        // A limit order starts at the last price.
        if self.prefill
            && let Some(p) = last
        {
            self.draft.limit = price_text(round_to_tick(
                p,
                tick_for(p),
                RoundingStrategy::MidpointAwayFromZero,
            ));
            self.prefill = false;
        }

        // Prices and the position.
        let held = positions
            .data()
            .and_then(|list| list.iter().find(|p| p.symbol == symbol && !p.is_option()));
        ui.horizontal_wrapped(|ui| {
            match (row.last, row.last_time) {
                (Some(p), Some(t)) => {
                    ui.label(
                        RichText::new(format!("Last {}", fmt::price(p))).color(skin.text_strong),
                    );
                    ui.label(
                        RichText::new(market::when(t))
                            .small()
                            .color(skin.text_muted),
                    );
                }
                _ => {
                    ui.label(RichText::new("No price yet").color(skin.text_muted));
                }
            }
            if let Some(q) = row.quote.as_ref().filter(|q| q.is_two_sided()) {
                ui.label(
                    RichText::new(format!(
                        "· bid {} × {} · ask {} × {}",
                        fmt::price(q.bid),
                        fmt::volume(q.bid_size),
                        fmt::price(q.ask),
                        fmt::volume(q.ask_size)
                    ))
                    .color(skin.text),
                );
            }
            ui.label(
                RichText::new(format!("· {}", cx.alpaca.feed().label()))
                    .small()
                    .color(skin.text_muted),
            );
            ui.add_space(8.0);
            match held {
                Some(p) => ui.label(
                    RichText::new(format!(
                        "Position {} (average {})",
                        portfolio::qty(p.qty),
                        portfolio::price(p.avg_entry_price)
                    ))
                    .color(skin.text),
                ),
                None => ui.label(RichText::new("No position").color(skin.text_muted)),
            };
            let open_here = book.open().filter(|o| o.symbol == symbol).count();
            if open_here > 0 {
                let s = if open_here == 1 { "" } else { "s" };
                ui.label(
                    RichText::new(format!("· {open_here} open order{s} in {symbol} (ORD)"))
                        .color(skin.warning),
                );
            }
        });

        let request = self.draft.request(&self.client_order_id);
        let today = trading::today();
        let context = guard::Context {
            account: account.data(),
            position: held.map_or(Decimal::ZERO, |p| p.qty),
            asset: assets.get(symbol),
            last,
            bid,
            ask,
            session,
            today_value: day_value(&book.orders, today, |s| {
                (s == symbol).then_some(last).flatten()
            }),
            day_trade: is_day_trade(&book.orders, today, symbol, self.draft.side),
            available: held.and_then(|p| p.qty_available),
        };
        let review = request
            .as_ref()
            .ok()
            .map(|r| guard::review(r, &context, &cx.config.trading));

        widgets::section(ui, skin, "Estimate");
        Grid::new("ticket-estimate")
            .num_columns(2)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                let dash = || fmt::DASH.to_owned();
                let what = match self.draft.side {
                    OrderSide::Buy => "Cost",
                    OrderSide::Sell => "Proceeds",
                };
                ui.label(what);
                ui.label(
                    RichText::new(
                        review
                            .as_ref()
                            .and_then(|r| r.value)
                            .map_or_else(dash, portfolio::usd),
                    )
                    .strong()
                    .color(skin.text_strong),
                );
                ui.end_row();
                ui.label("At");
                ui.label(
                    review
                        .as_ref()
                        .and_then(|r| r.price)
                        .map_or_else(dash, |p| {
                            let how = match self.draft.order_type {
                                OrderType::Market => match self.draft.side {
                                    OrderSide::Buy => "the ask, for a market order",
                                    OrderSide::Sell => "the bid, for a market order",
                                },
                                OrderType::Stop => "the stop",
                                _ => "the limit",
                            };
                            format!("{} ({how})", portfolio::price(p))
                        }),
                );
                ui.end_row();
                ui.label("Buying power");
                ui.label(
                    match (
                        account.data(),
                        review.as_ref().and_then(|r| r.buying_power_after),
                    ) {
                        (Some(a), Some(after)) if after != a.buying_power => {
                            format!(
                                "{} → {}",
                                portfolio::usd(a.buying_power),
                                portfolio::usd(after)
                            )
                        }
                        (Some(a), Some(_)) => {
                            format!("{} (this order uses none)", portfolio::usd(a.buying_power))
                        }
                        (Some(a), None) => portfolio::usd(a.buying_power),
                        _ => dash(),
                    },
                );
                ui.end_row();
                ui.label("Position afterwards");
                ui.label(review.as_ref().map_or_else(dash, |r| {
                    let after = r.position_after;
                    let kind = if after > Decimal::ZERO {
                        "long"
                    } else if after < Decimal::ZERO {
                        "short"
                    } else {
                        "closed"
                    };
                    format!("{} shares ({kind})", portfolio::qty(after))
                }));
                ui.end_row();
                if let Some(a) = account.data() {
                    ui.label("Day trades");
                    ui.label(match a.day_trades_left() {
                        Some(n) => format!(
                            "{} of {} used in five business days ({n} left below $25,000)",
                            a.daytrade_count,
                            mt_core::account::PDT_DAY_TRADES
                        ),
                        None => format!(
                            "{} in five business days (no limit at $25,000 or more)",
                            a.daytrade_count
                        ),
                    });
                    ui.end_row();
                }
            });

        widgets::section(ui, skin, "Checks");
        let (request, review) = match (request, review) {
            (Ok(request), Some(review)) => (request, review),
            (Err(e), _) => {
                ui.label(RichText::new(e).color(skin.text_muted));
                return;
            }
            (Ok(_), None) => return,
        };
        for c in trading::ordered(&review.checks) {
            trading::check_line(ui, skin, c);
        }
        let description = request.describe();
        let has_warnings = review.warnings().next().is_some();
        let mut ack = self.acknowledged.as_deref() == Some(description.as_str());
        if has_warnings
            && !locked
            && ui
                .checkbox(
                    &mut ack,
                    "I have read the warnings and want to send this order",
                )
                .changed()
        {
            self.acknowledged = ack.then(|| description.clone());
        }

        // Confirm.
        ui.add_space(8.0);
        let can_send = cx.desk.can_send();
        let ready = review.can_confirm(ack) && can_send.is_ok() && !locked;
        let again = matches!(outcome, Some(Outcome::NotPlaced { .. }));
        let fill = match self.draft.side {
            OrderSide::Buy => skin.positive,
            OrderSide::Sell => skin.negative,
        };
        let label = format!(
            "{}: {description}",
            if again { "Send again" } else { "Confirm" }
        );
        let button = egui::Button::new(RichText::new(label).strong().color(if ready {
            skin.background
        } else {
            skin.text_muted
        }))
        .fill(if ready { fill } else { skin.surface_alt })
        .min_size(egui::vec2(260.0, 30.0));
        let clicked = ui
            .add_enabled(ready, button)
            .on_hover_text("Sends the order to Alpaca. Nothing else does.")
            .clicked();
        if clicked {
            self.message = cx.desk.submit(request.clone()).err();
        }
        if !locked && !ready {
            let why = if let Err(e) = &can_send {
                e.clone()
            } else if let Some(b) = review.checks.iter().find(|c| c.level == Level::Block) {
                format!("Blocked: {}", b.message)
            } else {
                "Tick the box above once you have read the warnings.".to_owned()
            };
            ui.label(RichText::new(why).small().color(skin.text_muted));
        }
        if let Some(m) = &self.message {
            ui.label(RichText::new(m).color(skin.warning));
        }

        // What became of it.
        if let Some(o) = outcome {
            ui.add_space(6.0);
            trading::outcome_line(ui, skin, o);
            match o {
                Outcome::Accepted(placed) => {
                    let live = book.get(&placed.id).unwrap_or(placed);
                    ui.horizontal_wrapped(|ui| {
                        let mut s =
                            format!("{} · order {}", live.status.label(), short_id(&live.id));
                        if live.filled_qty > Decimal::ZERO {
                            s.push_str(&format!(
                                " · filled {}{}",
                                portfolio::qty(live.filled_qty),
                                live.filled_avg_price
                                    .map(|p| format!(" at {}", portfolio::price(p)))
                                    .unwrap_or_default()
                            ));
                        }
                        ui.label(RichText::new(s).color(skin.text));
                    });
                    ui.horizontal(|ui| {
                        if live.status.can_cancel() && ui.button("Cancel order").clicked() {
                            cx.desk.cancel(&live.id);
                        }
                        if let Some(a) = cx.desk.action(&mt_alpaca::OrderDesk::cancel_key(&live.id))
                        {
                            ui.label(
                                RichText::new(action_text(&a))
                                    .small()
                                    .color(skin.text_muted),
                            );
                        }
                        if ui.button("Open ORD").clicked() {
                            cx.open(Route::code("ORD"));
                        }
                        if ui
                            .button("New order")
                            .on_hover_text("Start another order from these fields")
                            .clicked()
                        {
                            self.new_order();
                        }
                    });
                }
                Outcome::Unknown { .. } if ui.button("Check again").clicked() => {
                    cx.desk.check_again(&self.client_order_id);
                }
                _ => {}
            }
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Paper trading: simulated orders against Alpaca's paper account. Limits and the \
                 restricted list are in SET; the kill switch is in ORD.",
            )
            .small()
            .color(skin.text_muted),
        );
    }
}

/// The start of an order id: `0ac07af2`.
pub(crate) fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// A cancel's or the kill switch's state, in words.
pub(crate) fn action_text(a: &mt_alpaca::ActionState) -> String {
    match a {
        mt_alpaca::ActionState::Working => "working…".into(),
        mt_alpaca::ActionState::Done(s) => s.clone(),
        mt_alpaca::ActionState::Failed(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        // As the command line groups them: the security is one argument.
        let mut out: Vec<String> = Vec::new();
        let tokens: Vec<&str> = s.split_whitespace().collect();
        let mut i = 0;
        while i < tokens.len() {
            if tokens.get(i + 1) == Some(&"US") {
                out.push(format!("{} US", tokens[i]));
                i += 2;
            } else {
                out.push(tokens[i].to_owned());
                i += 1;
            }
        }
        out
    }

    #[test]
    fn tickets_read_the_command() {
        let d = parse_args(OrderSide::Buy, &args("XLU US 10 LMT 82.50 DAY")).unwrap();
        assert_eq!(
            (
                d.symbol.as_str(),
                d.qty.as_str(),
                d.order_type,
                d.limit.as_str(),
                d.tif
            ),
            ("XLU", "10", OrderType::Limit, "82.50", TimeInForce::Day)
        );
        let d = parse_args(OrderSide::Sell, &args("XLU US 10 MKT")).unwrap();
        assert_eq!(d.order_type, OrderType::Market);
        let d = parse_args(OrderSide::Buy, &args("XLU US 10 @82.5 GTC EXT")).unwrap();
        assert_eq!(
            (d.order_type, d.limit.as_str(), d.tif, d.extended),
            (OrderType::Limit, "82.5", TimeInForce::Gtc, true)
        );
        let d = parse_args(OrderSide::Buy, &args("XLU US 10 82.50")).unwrap();
        assert_eq!(
            (d.order_type, d.limit.as_str()),
            (OrderType::Limit, "82.50")
        );
        let d = parse_args(OrderSide::Sell, &args("XLU US 10 STP 80 LMT 79.50")).unwrap();
        assert_eq!(
            (d.order_type, d.stop.as_str(), d.limit.as_str()),
            (OrderType::StopLimit, "80", "79.50")
        );
        let d = parse_args(OrderSide::Sell, &args("XLU US 10 STPLMT 80 79.50 GTC")).unwrap();
        assert_eq!(
            (d.order_type, d.stop.as_str(), d.limit.as_str()),
            (OrderType::StopLimit, "80", "79.50")
        );
        let d = parse_args(OrderSide::Buy, &[]).unwrap();
        assert!(d.symbol.is_empty() && d.order_type == OrderType::Limit);
        for bad in [
            "XLU US 10 LMT",
            "XLU US 10 BANANA",
            "XLU US XEL US 10",
            "XLU US 10 TRAIL",
            "XLU261218C00082500 1",
        ] {
            assert!(parse_args(OrderSide::Buy, &args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn routes_reopen_the_same_ticket() {
        for s in [
            "XLU US 10 LMT 82.50 DAY",
            "XLU US 10 MKT DAY",
            "XLU US 0.5 MKT DAY",
            "XLU US 10 STP 80 GTC",
            "XLU US 10 STPLMT 80 79.50 GTC",
            "XLU US 10 LMT 82.50 DAY EXT",
        ] {
            let d = parse_args(OrderSide::Sell, &args(s)).unwrap();
            let route = d.route();
            assert_eq!(route.to_string(), format!("SELL {s}"));
            assert_eq!(parse_args(OrderSide::Sell, &route.args).unwrap(), d, "{s}");
        }
    }

    #[test]
    fn a_draft_becomes_a_request_only_when_complete() {
        let d = parse_args(OrderSide::Buy, &args("XLU US 10 LMT 82.50")).unwrap();
        let r = d.request("mt-1").unwrap();
        assert_eq!(r.describe(), "Buy 10 XLU · limit 82.50 · DAY");
        let mut d2 = d.clone();
        d2.limit.clear();
        assert!(d2.request("mt-1").unwrap_err().contains("limit price"));
        d2.qty = "0".into();
        assert!(d2.request("mt-1").unwrap_err().contains("shares"));
        let none = parse_args(OrderSide::Buy, &[]).unwrap();
        assert!(none.request("mt-1").unwrap_err().contains("ticker"));
    }
}
