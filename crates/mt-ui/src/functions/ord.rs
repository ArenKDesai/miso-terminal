//! ORD: the paper account's orders. Open, filled and cancelled orders (stock,
//! option and multi-leg), kept current by the order stream and reconciled
//! with Alpaca's order list after every event and reconnect; cancel or
//! replace open ones (a spread is cancelled and placed again); and the kill
//! switch, which cancels every open order (and, if asked, closes every
//! position) and turns trading off until it is turned back on here.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_alpaca::desk::KILL as KILL_KEY;
use mt_alpaca::{ActionState, OrderDesk, Outcome, Replacement};
use mt_core::account::OrderSide;
use mt_core::guard::{self, Level, Loaded, Review};
use mt_core::instrument::OptionContract;
use mt_core::money::{Decimal, parse_decimal};
use mt_core::order::{Order, OrderRequest, OrderType, day_value, price_text};

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::functions::ticket::{action_text, short_id};
use crate::market::{self, fmt};
use crate::portfolio;
use crate::trading;
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ORD",
    aliases: &["ORDERS", "BLOTTER", "OMS"],
    name: "Orders",
    category: Category::Account,
    usage: "ORD [OPEN | FILLED | CANCELED | ALL] [KILL]",
    description: "The Alpaca paper account's orders, kept current by the order stream: open, filled and cancelled; cancel or replace open orders; the kill switch (cancel every open order, optionally close every position, and turn trading off); the order audit log.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    Open,
    Filled,
    Canceled,
    All,
}

impl Filter {
    const ALL: [Self; 4] = [Self::Open, Self::Filled, Self::Canceled, Self::All];

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "OPEN" | "WORKING" => Some(Self::Open),
            "FILLED" | "FILLS" | "DONE" => Some(Self::Filled),
            "CANCELED" | "CANCELLED" | "CXL" => Some(Self::Canceled),
            "ALL" => Some(Self::All),
            _ => None,
        }
    }

    fn arg(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Filled => "FILLED",
            Self::Canceled => "CANCELED",
            Self::All => "ALL",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Filled => "Filled",
            Self::Canceled => "Cancelled, expired, rejected",
            Self::All => "All",
        }
    }

    fn matches(self, o: &Order) -> bool {
        use mt_core::order::OrderStatus as S;
        match self {
            Self::Open => o.status.is_open(),
            Self::Filled => o.status == S::Filled,
            Self::Canceled => matches!(
                o.status,
                S::Canceled | S::Expired | S::Rejected | S::Replaced
            ),
            Self::All => true,
        }
    }
}

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let mut filter = Filter::Open;
    let mut kill = false;
    for a in args {
        if a.eq_ignore_ascii_case("KILL") {
            kill = true;
        } else {
            filter = Filter::parse(a).ok_or_else(|| {
                format!("unknown view {a:?}: use OPEN, FILLED, CANCELED, ALL or KILL")
            })?;
        }
    }
    Ok(Box::new(Blotter {
        filter,
        search: String::new(),
        kill_open: kill,
        flatten: false,
        replace: None,
    }))
}

struct Blotter {
    filter: Filter,
    search: String,
    /// The kill switch's confirmation is showing.
    kill_open: bool,
    /// Also close every position.
    flatten: bool,
    replace: Option<ReplaceEdit>,
}

/// A replace being drafted: the order's new quantity and prices.
struct ReplaceEdit {
    order: Order,
    qty: String,
    limit: String,
    stop: String,
    client_order_id: String,
    acknowledged: bool,
}

impl ReplaceEdit {
    fn new(order: &Order) -> Self {
        let text = |v: Option<Decimal>| v.map(price_text).unwrap_or_default();
        Self {
            qty: order.qty.map(portfolio::qty).unwrap_or_default(),
            limit: text(order.limit_price),
            stop: text(order.stop_price),
            client_order_id: mt_alpaca::new_client_order_id(),
            acknowledged: false,
            order: order.clone(),
        }
    }

    /// The replacement as a whole new order (for the guardrails) and as the
    /// changes Alpaca takes.
    fn request(&self) -> Result<(OrderRequest, Replacement), String> {
        let o = &self.order;
        let kind = o
            .order_type
            .filter(|k| OrderType::TICKET.contains(k))
            .ok_or_else(|| format!("{} orders cannot be replaced here.", o.type_name))?;
        let qty = parse_decimal(&self.qty)
            .filter(|q| *q > Decimal::ZERO)
            .ok_or(if o.is_option() {
                "Type the number of contracts."
            } else {
                "Type the number of shares."
            })?;
        let price = |text: &str, need: bool, what: &str| -> Result<Option<Decimal>, String> {
            if !need {
                return Ok(None);
            }
            parse_decimal(text)
                .filter(|p| *p > Decimal::ZERO)
                .map(Some)
                .ok_or_else(|| format!("Type the {what} price."))
        };
        let limit = price(&self.limit, kind.needs_limit(), "limit")?;
        let stop = price(&self.stop, kind.needs_stop(), "stop")?;
        let req = OrderRequest {
            client_order_id: self.client_order_id.clone(),
            symbol: o.symbol.clone(),
            side: o.side,
            qty,
            order_type: kind,
            limit_price: limit,
            stop_price: stop,
            tif: o.tif.unwrap_or_default(),
            extended_hours: o.extended_hours,
            position_intent: o.position_intent,
            legs: Vec::new(),
        };
        let changed = |new: Option<Decimal>, old: Option<Decimal>| new.filter(|n| Some(*n) != old);
        let rep = Replacement {
            client_order_id: self.client_order_id.clone(),
            qty: changed(Some(qty), o.qty),
            limit_price: changed(limit, o.limit_price),
            stop_price: changed(stop, o.stop_price),
            tif: None,
        };
        if rep.qty.is_none() && rep.limit_price.is_none() && rep.stop_price.is_none() {
            return Err("Change the quantity or a price to replace the order.".into());
        }
        Ok((req, rep))
    }
}

impl Panel for Blotter {
    fn title(&self) -> String {
        match self.filter {
            Filter::Open => "ORD".into(),
            f => format!("ORD {}", f.arg()),
        }
    }

    fn route(&self) -> Route {
        match self.filter {
            Filter::Open => Route::code("ORD"),
            f => Route::new("ORD", [f.arg()]),
        }
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        for a in args {
            if a.eq_ignore_ascii_case("KILL") {
                self.kill_open = true;
            } else if let Some(f) = Filter::parse(a) {
                self.filter = f;
            }
        }
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let mode = cx.alpaca.mode();
        widgets::title_bar(
            ui,
            skin,
            &format!("Orders · Alpaca {} account", mode.name()),
            |ui| {
                if !cx.alpaca.is_ready() {
                    return;
                }
                let kill = egui::Button::new(
                    RichText::new("Kill switch…")
                        .strong()
                        .color(skin.background),
                )
                .fill(skin.negative);
                if ui
                    .add(kill)
                    .on_hover_text("Cancel every open order, and optionally close every position")
                    .clicked()
                {
                    self.kill_open = true;
                }
                market::status_label(ui, cx);
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let book = trading::watch_orders(cx);
        let can_send = cx.desk.can_send();

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                self.switches(ui, cx, &can_send);
                if let Some(edit) = self.replace.take() {
                    self.replace = self.replace_editor(ui, cx, &book, edit);
                }
                let all = &book.orders;
                ui.horizontal_wrapped(|ui| {
                    for f in Filter::ALL {
                        let n = all.iter().filter(|o| f.matches(o)).count();
                        if ui
                            .selectable_label(self.filter == f, format!("{} {n}", f.label()))
                            .clicked()
                        {
                            self.filter = f;
                        }
                    }
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("symbol…")
                            .desired_width(100.0),
                    );
                    if let Some(n) = book
                        .trades
                        .data()
                        .and_then(|t| t.notice.clone())
                        .or_else(|| book.trades.error.as_ref().map(ToString::to_string))
                    {
                        ui.label(RichText::new(format!("⚠ {n}")).small().color(skin.warning));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::freshness(ui, skin, &book.list);
                    });
                });
                let needle = self.search.trim().to_ascii_uppercase();
                let rows: Vec<&Order> = all
                    .iter()
                    .filter(|o| self.filter.matches(o))
                    .filter(|o| {
                        needle.is_empty()
                            || o.symbol.contains(&needle)
                            || o.legs.iter().any(|l| l.symbol.contains(&needle))
                    })
                    .collect();
                if book.list.data.is_none() && book.orders.is_empty() {
                    widgets::placeholder(
                        ui,
                        skin,
                        book.list.error.as_ref().map(ToString::to_string),
                    );
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{} of {} orders", rows.len(), all.len()))
                            .small()
                            .color(skin.text_muted),
                    );
                    csv::copy_button(ui, skin, || to_csv(&rows));
                });
                if rows.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("No orders here.").color(skin.text_muted));
                } else {
                    self.table(ui, cx, &rows, can_send.is_ok());
                }
                ui.add_space(8.0);
                audit_note(ui, cx);
            });
    }
}

impl Blotter {
    /// Trading off, the kill switch's confirmation and result, a replay's notice.
    fn switches(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, can_send: &Result<(), String>) {
        let skin = cx.skin;
        if let Err(e) = can_send {
            ui.label(RichText::new(e).color(skin.text_muted));
        }
        if !cx.config.trading.enabled {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, skin.warning))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Trading is off: tickets cannot send orders.")
                                .strong()
                                .color(skin.warning),
                        );
                        if ui.button("Turn trading on").clicked() {
                            cx.send(AppCommand::SetTradingEnabled(true));
                        }
                    });
                });
        }
        if self.kill_open {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.5, skin.negative))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.label(RichText::new("Kill switch").strong().color(skin.negative));
                    ui.label(
                        "Cancel every open order in the paper account now, and turn trading off \
                         until you turn it back on here.",
                    );
                    ui.checkbox(
                        &mut self.flatten,
                        "Also close every position at the market (outside the regular session the \
                         closing orders wait for the open)",
                    );
                    ui.horizontal(|ui| {
                        let what = if self.flatten {
                            "Cancel every order and close every position"
                        } else {
                            "Cancel every open order"
                        };
                        let go =
                            egui::Button::new(RichText::new(what).strong().color(skin.background))
                                .fill(skin.negative);
                        if ui.add_enabled(can_send.is_ok(), go).clicked() {
                            cx.desk.kill(self.flatten);
                            cx.send(AppCommand::SetTradingEnabled(false));
                            self.kill_open = false;
                        }
                        if ui.button("Back").clicked() {
                            self.kill_open = false;
                        }
                    });
                });
        }
        if let Some(state) = cx.desk.action(KILL_KEY) {
            let color = match state {
                ActionState::Working => skin.text_muted,
                ActionState::Done(_) => skin.positive,
                ActionState::Failed(_) => skin.negative,
            };
            ui.label(RichText::new(format!("Kill switch: {}", action_text(&state))).color(color));
        }
    }

    fn table(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, rows: &[&Order], can_send: bool) {
        let skin = cx.skin;
        let row_h = 22.0;
        let mut open_route = None;
        let mut start_replace = None;
        let desk = cx.desk.clone();
        egui::ScrollArea::horizontal()
            .id_salt("ord-h")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("ord-table")
                    .striped(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(132.0).at_least(124.0))
                    .column(Column::initial(150.0).at_least(80.0).clip(true))
                    .column(Column::initial(40.0))
                    .columns(Column::initial(56.0).at_least(44.0), 2)
                    .column(Column::initial(112.0).at_least(70.0))
                    .column(Column::initial(48.0))
                    .column(Column::initial(110.0).at_least(90.0))
                    .column(Column::initial(70.0).at_least(56.0))
                    .column(Column::remainder().at_least(170.0))
                    .header(row_h, |mut h| {
                        for name in [
                            "Created", "Symbol", "Side", "Qty", "Filled", "Type", "TIF", "Status",
                            "Avg fill", "",
                        ] {
                            h.col(|ui| {
                                widgets::label(ui, skin, name);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, rows.len(), |mut row| {
                            let o = rows[row.index()];
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(o.created_at.map_or_else(
                                        || fmt::DASH.to_owned(),
                                        |t| {
                                            mt_core::exchange::to_exchange(t)
                                                .format("%b %d %H:%M:%S")
                                                .to_string()
                                        },
                                    ))
                                    .monospace()
                                    .color(skin.text_muted),
                                )
                                .on_hover_text(format!(
                                    "order {}\nyour id {}",
                                    o.id, o.client_order_id
                                ));
                            });
                            row.col(|ui| {
                                let name = format!("{} US", o.symbol);
                                if o.is_multi_leg() {
                                    let legs: Vec<String> = o
                                        .strategy_legs()
                                        .iter()
                                        .map(mt_core::options::Leg::describe)
                                        .collect();
                                    ui.label(o.title()).on_hover_text(legs.join("\n"));
                                } else if let Some(c) =
                                    mt_core::instrument::OptionContract::parse_occ(&o.symbol)
                                {
                                    ui.label(portfolio::contract_name(&c))
                                        .on_hover_text(&o.symbol);
                                } else if widgets::link(ui, skin, &name).clicked() {
                                    open_route = Some(Route::new("GP", [name]));
                                }
                            });
                            row.col(|ui| {
                                let side =
                                    ui.label(RichText::new(o.side.label()).color(match o.side {
                                        OrderSide::Buy => skin.positive,
                                        OrderSide::Sell => skin.negative,
                                    }));
                                if let Some(i) = o.position_intent {
                                    side.on_hover_text(i.label());
                                }
                            });
                            row.col(|ui| {
                                crate::widgets::table::num_cell(
                                    ui,
                                    o.qty
                                        .map(portfolio::qty)
                                        .or_else(|| o.notional.map(portfolio::usd))
                                        .unwrap_or_default(),
                                )
                            });
                            row.col(|ui| {
                                crate::widgets::table::num_cell(ui, portfolio::qty(o.filled_qty))
                            });
                            row.col(|ui| {
                                ui.label(type_text(o));
                            });
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(o.tif.map_or("", |t| t.code()))
                                        .color(skin.text_muted),
                                );
                            });
                            row.col(|ui| {
                                let color = if o.status.is_open() {
                                    skin.live
                                } else if o.status == mt_core::order::OrderStatus::Filled {
                                    skin.text_strong
                                } else {
                                    skin.text_muted
                                };
                                ui.label(RichText::new(o.status.label()).color(color));
                            });
                            row.col(|ui| {
                                crate::widgets::table::num_cell(
                                    ui,
                                    o.filled_avg_price.map(portfolio::price).unwrap_or_default(),
                                )
                            });
                            row.col(|ui| {
                                if !o.status.is_open() {
                                    return;
                                }
                                if ui
                                    .add_enabled(
                                        can_send && o.status.can_cancel(),
                                        egui::Button::new("Cancel").small(),
                                    )
                                    .clicked()
                                {
                                    desk.cancel(&o.id);
                                }
                                if ui
                                    .add_enabled(
                                        can_send && o.status.can_replace() && !o.is_multi_leg(),
                                        egui::Button::new("Replace…").small(),
                                    )
                                    .on_disabled_hover_text(if o.is_multi_leg() {
                                        "To change a spread, cancel it and place it again from MLEG"
                                    } else {
                                        "This order can no longer be replaced"
                                    })
                                    .clicked()
                                {
                                    start_replace = Some(o.clone());
                                }
                                if let Some(a) = desk.action(&OrderDesk::cancel_key(&o.id)) {
                                    let color = match a {
                                        ActionState::Failed(_) => skin.negative,
                                        _ => skin.text_muted,
                                    };
                                    ui.label(RichText::new(action_text(&a)).small().color(color))
                                        .on_hover_text(action_text(&a));
                                }
                            });
                        });
                    });
            });
        if let Some(r) = open_route {
            cx.open(r);
        }
        if let Some(o) = start_replace {
            self.replace = Some(ReplaceEdit::new(&o));
        }
    }

    /// The replace editor; returns it while it should stay open.
    fn replace_editor(
        &mut self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        book: &trading::OrderBook,
        mut edit: ReplaceEdit,
    ) -> Option<ReplaceEdit> {
        let skin = cx.skin;
        let outcome = cx.desk.outcome(&edit.client_order_id);
        let locked = outcome.as_ref().is_some_and(|o| !o.may_send());
        let mut keep = true;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, skin.border_strong))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let o = &edit.order;
                ui.label(
                    RichText::new(format!(
                        "Replace order {} · {} {} {} · {}",
                        short_id(&o.id),
                        o.side.label(),
                        o.qty.map(portfolio::qty).unwrap_or_default(),
                        o.symbol,
                        type_text(o)
                    ))
                    .strong()
                    .color(skin.text_strong),
                );
                let kind = o.order_type;
                ui.add_enabled_ui(!locked, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        widgets::label(ui, skin, "Shares");
                        ui.add(egui::TextEdit::singleline(&mut edit.qty).desired_width(70.0));
                        if kind.is_some_and(OrderType::needs_stop) {
                            widgets::label(ui, skin, "Stop");
                            ui.add(egui::TextEdit::singleline(&mut edit.stop).desired_width(70.0));
                        }
                        if kind.is_some_and(OrderType::needs_limit) {
                            widgets::label(ui, skin, "Limit");
                            ui.add(egui::TextEdit::singleline(&mut edit.limit).desired_width(70.0));
                        }
                    });
                });
                let parsed = edit.request();
                let review = parsed.as_ref().ok().map(|(req, _)| {
                    if let Some(c) = req.contract() {
                        return option_replace_review(cx, book, &edit.order, req, &c);
                    }
                    let account = cx.hub.watch(&cx.alpaca.account());
                    let positions = cx.hub.watch(&cx.alpaca.positions());
                    let board = market::board(cx, std::slice::from_ref(&req.symbol));
                    let row = board.row(&req.symbol);
                    let exact = |v: Option<f64>| v.and_then(|v| mt_core::account::from_f64(v, 4));
                    let held = positions
                        .data()
                        .and_then(|l| l.iter().find(|p| p.symbol == req.symbol && !p.is_option()))
                        .map_or(Decimal::ZERO, |p| p.qty);
                    let last = exact(row.last);
                    // The order being replaced no longer counts towards today's total.
                    let today = day_value(&book.orders, trading::today(), |_| last)
                        - if edit.order.created_at.is_some_and(|t| {
                            mt_core::exchange::to_exchange(t).date_naive() == trading::today()
                        }) {
                            edit.order.value(last).abs()
                        } else {
                            Decimal::ZERO
                        };
                    // What the order would leave the position at is what it
                    // would have done before, so review it from the position
                    // before any of it filled.
                    let before = held - edit.order.side.sign() * edit.order.filled_qty;
                    guard::review(
                        req,
                        &guard::Context {
                            account: account.data(),
                            loaded: trading::loaded(&positions, book),
                            position: before,
                            asset: market::assets(cx).get(&req.symbol),
                            last,
                            bid: row.quote.as_ref().and_then(|q| exact(Some(q.bid))),
                            ask: row.quote.as_ref().and_then(|q| exact(Some(q.ask))),
                            priced_at: row.priced_at(),
                            session: market::status(cx.hub, cx.alpaca).session,
                            now: mt_core::time::now_utc(),
                            today_value: today.max(Decimal::ZERO),
                            day_trade: false,
                            available: None,
                        },
                        &cx.config.trading,
                    )
                });
                match (&parsed, &review) {
                    (Err(e), _) => {
                        ui.label(RichText::new(e).color(skin.text_muted));
                    }
                    (Ok(_), Some(r)) => {
                        for c in trading::ordered(&r.checks)
                            .into_iter()
                            .filter(|c| c.level != Level::Pass)
                        {
                            trading::check_line(ui, skin, c);
                        }
                        if r.warnings().next().is_some() && !locked {
                            ui.checkbox(&mut edit.acknowledged, "I have read the warnings");
                        }
                    }
                    _ => {}
                }
                ui.horizontal(|ui| {
                    let ready = !locked
                        && cx.desk.can_send().is_ok()
                        && review
                            .as_ref()
                            .is_some_and(|r| r.can_confirm(edit.acknowledged));
                    let go = egui::Button::new(RichText::new("Confirm replace").strong().color(
                        if ready {
                            skin.background
                        } else {
                            skin.text_muted
                        },
                    ))
                    .fill(if ready { skin.accent } else { skin.surface_alt });
                    if ui.add_enabled(ready, go).clicked()
                        && let (Ok((_, rep)), Some(r)) = (&parsed, &review)
                        && let Err(e) = r
                            .approve(edit.acknowledged)
                            .and_then(|a| cx.desk.replace(&edit.order.id, rep.clone(), &a))
                    {
                        ui.label(RichText::new(e).color(skin.warning));
                    }
                    let close = if matches!(outcome, Some(Outcome::Accepted(_))) {
                        "Done"
                    } else {
                        "Close"
                    };
                    if !outcome.as_ref().is_some_and(Outcome::is_pending)
                        && ui.button(close).clicked()
                    {
                        keep = false;
                    }
                    if matches!(outcome, Some(Outcome::Unknown { .. }))
                        && ui.button("Check again").clicked()
                    {
                        cx.desk.check_again(&edit.client_order_id);
                    }
                });
                if let Some(o) = &outcome {
                    trading::outcome_line(ui, skin, o);
                }
            });
        keep.then_some(edit)
    }
}

/// The option guardrails for a replacement, from the position as it was
/// before any of the order filled (and with nothing held back by it).
fn option_replace_review(
    cx: &PanelCx<'_>,
    book: &trading::OrderBook,
    order: &Order,
    req: &OrderRequest,
    contract: &OptionContract,
) -> Review {
    let m = trading::watch_option(cx, std::slice::from_ref(contract));
    let Some(m) = m else {
        return guard::review_option(
            req,
            &guard::OptionContext {
                account: None,
                loaded: Loaded {
                    positions: false,
                    orders: book.loaded(),
                },
                positions: &[],
                info: None,
                bid: None,
                ask: None,
                last: None,
                priced_at: None,
                session: market::status(cx.hub, cx.alpaca).session,
                now: mt_core::time::now_utc(),
                today_value: Decimal::ZERO,
                day_trade: false,
            },
            &cx.config.trading,
        );
    };
    let mark = m.mark(&req.symbol);
    let today = trading::today();
    let mut today_value = day_value(&book.orders, today, |s| {
        (s == req.symbol).then_some(mark).flatten()
    });
    if order
        .created_at
        .is_some_and(|t| mt_core::exchange::to_exchange(t).date_naive() == today)
    {
        today_value -= order.value(mark).abs();
    }
    let before = m.held(&req.symbol) - order.side.sign() * order.filled_qty;
    let mut positions = m.positions().to_vec();
    if let Some(p) = positions.iter_mut().find(|p| p.symbol == req.symbol) {
        p.qty = before;
        p.qty_available = None;
    }
    positions.retain(|p| !p.qty.is_zero());
    let mut context = m.context(
        &req.symbol,
        book,
        market::status(cx.hub, cx.alpaca).session,
        today_value.max(Decimal::ZERO),
        false,
    );
    context.positions = &positions;
    guard::review_option(req, &context, &cx.config.trading)
}

/// `Limit 82.50`, `Stop 80.00 / 79.50`, `Market`.
fn type_text(o: &Order) -> String {
    let name = o
        .order_type
        .map_or_else(|| o.type_name.replace('_', " "), |t| t.label().to_owned());
    if o.is_multi_leg() {
        return match o.limit_price {
            Some(p) if p.is_sign_negative() => format!("{name} {} credit", price_text(-p)),
            Some(p) => format!("{name} {} debit", price_text(p)),
            None => name,
        };
    }
    match (o.stop_price, o.limit_price) {
        (Some(s), Some(l)) => format!("{name} {} / {}", price_text(s), price_text(l)),
        (Some(p), None) | (None, Some(p)) => format!("{name} {}", price_text(p)),
        (None, None) => name,
    }
}

/// Where the audit log is, and a way to open it.
fn audit_note(ui: &mut Ui, cx: &mut PanelCx<'_>) {
    let skin = cx.skin;
    let audit = cx.desk.audit();
    ui.horizontal_wrapped(|ui| {
        match audit.dir() {
            Some(dir) => {
                // The path only on hover: it names the Windows user.
                ui.label(
                    RichText::new(
                        "Every order request and answer is written to the audit folder \
                         (orders-YYYY-MM.jsonl, one JSON object per line; keys are never written).",
                    )
                    .small()
                    .color(skin.text_muted),
                )
                .on_hover_text(dir.display().to_string());
                let dir = dir.to_path_buf();
                if ui
                    .small_button("Open audit folder")
                    .on_hover_text(dir.display().to_string())
                    .clicked()
                {
                    cx.send(AppCommand::RevealPath(dir));
                }
            }
            None => {
                ui.label(
                    RichText::new("Order requests are logged in memory only (no folder set).")
                        .small()
                        .color(skin.text_muted),
                );
            }
        }
        if let Some(e) = audit.error() {
            ui.label(RichText::new(e).small().color(skin.negative));
        }
    });
}

fn to_csv(rows: &[&Order]) -> String {
    let n = |v: Option<Decimal>| v.map_or_else(String::new, |v| v.normalize().to_string());
    let t =
        |v: Option<chrono::DateTime<chrono::Utc>>| v.map_or_else(String::new, |t| t.to_rfc3339());
    csv::to_csv(
        &[
            "id",
            "client_order_id",
            "created_utc",
            "symbol",
            "side",
            "qty",
            "filled_qty",
            "type",
            "limit_price",
            "stop_price",
            "time_in_force",
            "status",
            "filled_avg_price",
            "filled_utc",
            "position_intent",
            "legs",
        ],
        rows.iter().map(|o| {
            vec![
                o.id.clone(),
                o.client_order_id.clone(),
                t(o.created_at),
                o.symbol.clone(),
                o.side.label().to_owned(),
                n(o.qty),
                n(Some(o.filled_qty)),
                o.type_name.clone(),
                n(o.limit_price),
                n(o.stop_price),
                o.tif.map_or("", |x| x.as_str()).to_owned(),
                o.status.label(),
                n(o.filled_avg_price),
                t(o.filled_at),
                o.position_intent.map_or("", |i| i.as_str()).to_owned(),
                o.strategy_legs()
                    .iter()
                    .map(mt_core::options::Leg::describe)
                    .collect::<Vec<_>>()
                    .join("; "),
            ]
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_round_trip() {
        for f in Filter::ALL {
            assert_eq!(Filter::parse(f.arg()), Some(f));
        }
        assert!(open(&["NOPE".into()]).is_err());
        assert!(open(&["KILL".into()]).is_ok());
    }
}
