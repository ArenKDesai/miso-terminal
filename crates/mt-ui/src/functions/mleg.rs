//! MLEG: a ticket for an options strategy of two to four legs (a spread) in
//! the paper account, sent as one order (Alpaca's `mleg` class).
//!
//! `MLEG +XLU261218C00045000 -XLU261218C00047000 2 LMT 0.85` opens a ticket
//! for two 45/47 bull call spreads at a net debit of 0.85; a negative limit
//! (or `CREDIT 0.40`) is a credit, `+2*` a ratio. OMON builds these from its
//! chain. The ticket shows each leg's quote and greeks and whether it opens
//! or closes a position, the strategy's natural, mid and far prices, the
//! margin Alpaca holds, buying power, and what it pays at expiry, with a
//! chart; then every guardrail (`mt_core::guard::review_spread`). Only its
//! **Confirm** sends the order; commands from outside the window only open
//! it. Like BUY and SELL, it keeps one `client_order_id` until the order is
//! placed and is not restored after a restart.

use egui::{Grid, RichText, Ui};
use egui_plot::{HLine, Line, LineStyle, Plot, PlotPoints, Points, VLine};
use mt_alpaca::Outcome;
use mt_core::account::{OrderSide, from_f64, to_f64};
use mt_core::guard::{self, LegMarket, SpreadContext};
use mt_core::instrument::OptionContract;
use mt_core::money::{Decimal, RoundingStrategy, parse_decimal};
use mt_core::options::{Leg, MAX_LEGS, MULTIPLIER, PositionIntent, contract_name, strategy_name};
use mt_core::order::{OrderRequest, OrderType, TimeInForce, day_value, is_day_trade, price_text};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::functions::ticket::{Sending, confirm, side_fill};
use crate::market::{self, fmt};
use crate::options as opt;
use crate::portfolio;
use crate::trading;
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "MLEG",
    aliases: &["COMBO", "OSTRAT"],
    name: "Spread ticket",
    category: Category::Account,
    // `±`, not `<+|->`: JetBrains Mono draws `|->` as an arrow ligature.
    usage: "MLEG ±[ratio*]<option> … [units] [MKT | LMT <net> | CREDIT <net>] [DAY | GTC]",
    description: "An order ticket for an options strategy of two to four legs in the Alpaca paper account, sent as one order: MLEG +XLU261218C00045000 -XLU261218C00047000 2 LMT 0.85 (a negative limit, or CREDIT 0.40, is a credit). Each leg's quote, the net price, margin, buying power, what it pays at expiry with a chart, and every guardrail. OMON builds these from its chain. Only its Confirm button sends the order.",
    takes_node: false,
    takes_security: false,
    takes_option: true,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Spread::new(parse_args(args)?)))
}

/// A spread ticket's fields as typed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Draft {
    pub legs: Vec<Leg>,
    pub qty: String,
    pub order_type: OrderType,
    /// The net price, unsigned; `credit` says which way it goes.
    pub limit: String,
    pub credit: bool,
    pub tif: TimeInForce,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            legs: Vec::new(),
            qty: "1".into(),
            order_type: OrderType::Limit,
            limit: String::new(),
            credit: false,
            tif: TimeInForce::Day,
        }
    }
}

/// `+XLU261218C00045000`, `-2*XLU261218C00047000` or a bare symbol (bought).
pub(crate) fn parse_leg(t: &str) -> Option<Leg> {
    let (side, rest) = match t.as_bytes().first()? {
        b'+' => (OrderSide::Buy, &t[1..]),
        b'-' => (OrderSide::Sell, &t[1..]),
        _ => (OrderSide::Buy, t),
    };
    let (ratio, symbol) = match rest.split_once(['*', 'x']) {
        Some((n, s)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => {
            (n.parse::<u32>().ok().filter(|r| (1..=99).contains(r))?, s)
        }
        _ => (1, rest),
    };
    let occ = OptionContract::parse_occ(symbol)?.occ()?;
    Some(Leg::new(&occ, side, ratio))
}

/// `+XLU261218C00045000`, `-2*XLU261218C00047000`.
pub(crate) fn leg_arg(leg: &Leg) -> String {
    let sign = match leg.side {
        OrderSide::Buy => '+',
        OrderSide::Sell => '-',
    };
    if leg.ratio > 1 {
        format!("{sign}{}*{}", leg.ratio, leg.symbol)
    } else {
        format!("{sign}{}", leg.symbol)
    }
}

pub(crate) fn parse_args(args: &[String]) -> Result<Draft, String> {
    let mut d = Draft::default();
    let mut qty: Option<String> = None;
    let tokens: Vec<&str> = args.iter().map(|a| a.trim()).collect();
    let mut i = 0;
    let number = |t: Option<&&str>| t.and_then(|t| parse_decimal(t.trim_start_matches('@')));
    while i < tokens.len() {
        let t = tokens[i];
        let up = t.to_ascii_uppercase();
        if let Some(leg) = parse_leg(t) {
            if d.legs.iter().any(|l| l.symbol == leg.symbol) {
                return Err(format!("{} is in the order twice.", leg.symbol));
            }
            d.legs.push(leg);
        } else if matches!(up.as_str(), "MKT" | "MARKET") {
            d.order_type = OrderType::Market;
        } else if matches!(up.as_str(), "LMT" | "LIMIT" | "DEBIT" | "CREDIT" | "@") {
            let p = number(tokens.get(i + 1))
                .ok_or_else(|| format!("{t} needs a net price after it."))?;
            i += 1;
            d.order_type = OrderType::Limit;
            d.credit = up == "CREDIT" || (p.is_sign_negative() && !p.is_zero());
            d.limit = price_text(p.abs());
        } else if let Some(p) = t.strip_prefix('@').and_then(parse_decimal) {
            d.order_type = OrderType::Limit;
            d.credit = p.is_sign_negative() && !p.is_zero();
            d.limit = price_text(p.abs());
        } else if let Some(tif) = TimeInForce::parse(t) {
            d.tif = tif;
        } else if qty.is_none() && t.parse::<u32>().is_ok_and(|n| n > 0) {
            qty = Some(t.to_owned());
        } else {
            return Err(format!(
                "{t:?} is not part of a spread ticket. Usage: {}",
                SPEC.usage
            ));
        }
        i += 1;
    }
    if d.legs.len() > MAX_LEGS {
        return Err(format!("A spread has at most {MAX_LEGS} legs."));
    }
    if let Some(q) = qty {
        d.qty = q;
    }
    Ok(d)
}

impl Draft {
    /// The command that opens this ticket again.
    pub(crate) fn route(&self) -> Route {
        let mut args: Vec<String> = self.legs.iter().map(leg_arg).collect();
        if self.qty.trim().parse::<u32>().is_ok_and(|n| n > 0) {
            args.push(self.qty.trim().to_owned());
        }
        match self.order_type {
            OrderType::Market => args.push("MKT".into()),
            _ => {
                if let Some(p) = parse_decimal(&self.limit).filter(|p| *p >= Decimal::ZERO) {
                    let p = if self.credit { -p } else { p };
                    args.extend(["LMT".into(), price_text(p)]);
                }
            }
        }
        args.push(self.tif.code().into());
        Route::new("MLEG", args)
    }

    /// The net limit, signed (negative a credit).
    fn net_limit(&self) -> Option<Decimal> {
        let p = parse_decimal(&self.limit).filter(|p| *p >= Decimal::ZERO)?;
        Some(if self.credit { -p } else { p })
    }

    /// The order the fields describe, with each leg's intent.
    pub(crate) fn request(
        &self,
        client_order_id: &str,
        intents: &[PositionIntent],
    ) -> Result<OrderRequest, String> {
        if self.legs.len() < 2 {
            return Err("Add at least two legs (OMON builds spreads from its chain).".into());
        }
        let qty = self
            .qty
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(Decimal::from)
            .ok_or("Type how many spreads (a whole number).")?;
        let limit = match self.order_type {
            OrderType::Market => None,
            _ => Some(self.net_limit().ok_or("Type the net price.")?),
        };
        Ok(OrderRequest {
            client_order_id: client_order_id.to_owned(),
            symbol: String::new(),
            side: if limit.is_some_and(|p| p.is_sign_negative()) {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            },
            qty,
            order_type: self.order_type,
            limit_price: limit,
            stop_price: None,
            tif: self.tif,
            extended_hours: false,
            position_intent: None,
            legs: self
                .legs
                .iter()
                .zip(intents.iter().map(Some).chain(std::iter::repeat(None)))
                .map(|(l, i)| Leg {
                    intent: i.copied(),
                    ..l.clone()
                })
                .collect(),
        })
    }
}

struct Spread {
    draft: Draft,
    /// Fill the net price from the mid once the quotes are in.
    prefill: bool,
    sending: Sending,
    /// A leg being added (an OCC symbol).
    adding: String,
    /// Confirm was clicked: later commands open a new ticket instead.
    sent: bool,
}

impl Spread {
    fn new(draft: Draft) -> Self {
        Self {
            prefill: draft.order_type == OrderType::Limit && draft.limit.trim().is_empty(),
            draft,
            sending: Sending::new(),
            adding: String::new(),
            sent: false,
        }
    }
}

impl Panel for Spread {
    fn title(&self) -> String {
        if self.draft.legs.is_empty() {
            return "MLEG".into();
        }
        let under = self
            .draft
            .legs
            .iter()
            .find_map(Leg::contract)
            .map(|c| format!(" {}", c.underlying))
            .unwrap_or_default();
        format!("MLEG {}{under}", strategy_name(&self.draft.legs))
    }

    fn route(&self) -> Route {
        self.draft.route()
    }

    /// OMON sends new legs to an open ticket that has not sent anything.
    fn absorb(&mut self, args: &[String]) -> bool {
        if self.sent {
            return false;
        }
        match parse_args(args) {
            Ok(d) => {
                self.prefill = d.order_type == OrderType::Limit && d.limit.trim().is_empty();
                self.draft = d;
                self.sending.acknowledged = None;
                true
            }
            Err(_) => false,
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        widgets::title_bar(
            ui,
            skin,
            &format!("Spread ticket · Alpaca {} account", cx.alpaca.mode().name()),
            |ui| {
                if cx.alpaca.is_ready() {
                    market::status_label(ui, cx);
                }
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let outcome = cx.desk.outcome(&self.sending.client_order_id);
        let locked = outcome.as_ref().is_some_and(|o| !o.may_send());
        if outcome.is_some() {
            self.sent = true;
        }
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| self.body(ui, cx, outcome.as_ref(), locked));
    }
}

impl Spread {
    fn body(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, outcome: Option<&Outcome>, locked: bool) {
        let skin = cx.skin;
        let contracts: Vec<OptionContract> =
            self.draft.legs.iter().filter_map(Leg::contract).collect();
        let m = trading::watch_option(cx, &contracts);
        let book = trading::watch_orders(cx);
        let session = market::status(cx.hub, cx.alpaca).session;

        // The strategy, and the underlying.
        ui.horizontal_wrapped(|ui| {
            let name = strategy_name(&self.draft.legs);
            ui.label(RichText::new(name).heading().color(skin.text_strong));
            if let Some(c) = contracts.first() {
                ui.label(RichText::new(format!("on {} US", c.underlying)).color(skin.text_muted));
                let expiries: Vec<String> = {
                    let mut e: Vec<_> = contracts.iter().map(|c| c.expiry).collect();
                    e.sort();
                    e.dedup();
                    e.iter()
                        .map(|d| opt::expiry_label(*d, trading::today()))
                        .collect()
                };
                ui.label(RichText::new(expiries.join(", ")).color(skin.text_muted));
                if let Some(s) = m.as_ref().and_then(trading::OptionMarket::spot) {
                    ui.label(
                        RichText::new(format!("· {} {}", c.underlying, fmt::price(s)))
                            .color(skin.text),
                    );
                }
                if ui.small_button("OMON").on_hover_text("The chain").clicked() {
                    cx.open(Route::new("OMON", [c.occ().unwrap_or_default()]));
                }
            }
        });

        // The legs.
        let intents: Vec<PositionIntent> = self
            .draft
            .legs
            .iter()
            .map(|l| {
                PositionIntent::of(
                    l.side,
                    m.as_ref().map_or(Decimal::ZERO, |m| m.held(&l.symbol)),
                )
            })
            .collect();
        widgets::section(ui, skin, "Legs");
        let mut remove = None;
        ui.add_enabled_ui(!locked, |ui| {
            Grid::new("mleg-legs")
                .striped(true)
                .num_columns(10)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Side", "Ratio", "Contract", "Bid", "Ask", "Mid", "Delta", "IV", "Does", "",
                    ] {
                        widgets::label(ui, skin, h);
                    }
                    ui.end_row();
                    for (i, leg) in self.draft.legs.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            for (side, label, color) in [
                                (OrderSide::Buy, "Buy", skin.positive),
                                (OrderSide::Sell, "Sell", skin.negative),
                            ] {
                                let text = RichText::new(label).strong();
                                let text = if leg.side == side {
                                    text.color(color)
                                } else {
                                    text
                                };
                                if ui.selectable_label(leg.side == side, text).clicked() {
                                    leg.side = side;
                                }
                            }
                        });
                        ui.add(
                            egui::DragValue::new(&mut leg.ratio)
                                .range(1..=10)
                                .speed(0.05),
                        );
                        ui.label(
                            RichText::new(
                                leg.contract()
                                    .map_or_else(|| leg.symbol.clone(), |c| contract_name(&c)),
                            )
                            .color(skin.text_strong),
                        )
                        .on_hover_text(&leg.symbol);
                        let snap = m.as_ref().and_then(|m| m.snapshot(&leg.symbol));
                        ui.label(opt::premium_opt(snap.and_then(|s| s.bid())));
                        ui.label(opt::premium_opt(snap.and_then(|s| s.ask())));
                        ui.label(opt::premium_opt(snap.and_then(|s| s.mark())));
                        ui.label(opt::greek(snap.and_then(|s| s.greeks.delta), 2));
                        ui.label(opt::iv(snap.and_then(|s| s.implied_volatility)));
                        ui.label(
                            RichText::new(intents.get(i).map_or("", |x| x.label()))
                                .color(skin.text_muted),
                        );
                        if ui
                            .small_button("✕")
                            .on_hover_text("Remove this leg")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            ui.horizontal_wrapped(|ui| {
                let add = ui.add(
                    egui::TextEdit::singleline(&mut self.adding)
                        .hint_text("+XLU261218C00049000")
                        .desired_width(190.0),
                );
                let go = ui.button("Add leg").clicked()
                    || (add.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if go {
                    match parse_leg(self.adding.trim()) {
                        Some(leg) if !self.draft.legs.iter().any(|l| l.symbol == leg.symbol) => {
                            self.draft.legs.push(leg);
                            self.adding.clear();
                        }
                        _ => {}
                    }
                }
                ui.label(
                    RichText::new("an option symbol; + buys, - sells, 2* for a ratio")
                        .small()
                        .color(skin.text_muted),
                );
            });
            // Units, type, net price, time in force.
            let d = &mut self.draft;
            ui.horizontal_wrapped(|ui| {
                widgets::label(ui, skin, "Spreads");
                ui.add(egui::TextEdit::singleline(&mut d.qty).desired_width(50.0));
                ui.add_space(8.0);
                widgets::label(ui, skin, "Type");
                for (t, label) in [(OrderType::Limit, "Limit"), (OrderType::Market, "Market")] {
                    if ui.selectable_label(d.order_type == t, label).clicked() {
                        d.order_type = t;
                    }
                }
                if d.order_type == OrderType::Limit {
                    widgets::label(ui, skin, "Net");
                    if ui
                        .add(egui::TextEdit::singleline(&mut d.limit).desired_width(60.0))
                        .changed()
                    {
                        self.prefill = false;
                    }
                    for (credit, label) in [(false, "Debit"), (true, "Credit")] {
                        if ui.selectable_label(d.credit == credit, label).clicked() {
                            d.credit = credit;
                            self.prefill = false;
                        }
                    }
                }
                ui.add_space(8.0);
                widgets::label(ui, skin, "Time in force");
                for t in [TimeInForce::Day, TimeInForce::Gtc] {
                    if ui
                        .selectable_label(d.tif == t, t.code())
                        .on_hover_text(t.label())
                        .clicked()
                    {
                        d.tif = t;
                    }
                }
            });
        });
        if let Some(i) = remove {
            self.draft.legs.remove(i);
        }
        let Some(m) = m else {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Add the legs: an option symbol each, or build the spread in OMON.")
                    .color(skin.text_muted),
            );
            return;
        };

        // Prices, and the review.
        let legs: Vec<LegMarket<'_>> = self
            .draft
            .legs
            .iter()
            .map(|l| {
                let (bid, ask, _) = m.quote(&l.symbol);
                LegMarket {
                    info: m.info(&l.symbol),
                    bid,
                    ask,
                    quoted_at: m.quoted_at(&l.symbol),
                }
            })
            .collect();
        let request = self.draft.request(&self.sending.client_order_id, &intents);
        let today = trading::today();
        let marks: Vec<(String, Option<Decimal>)> = self
            .draft
            .legs
            .iter()
            .map(|l| (l.symbol.clone(), m.mark(&l.symbol)))
            .collect();
        let context = SpreadContext {
            account: m.account.data(),
            loaded: trading::loaded(&m.positions, &book),
            positions: m.positions(),
            legs,
            session,
            now: mt_core::time::now_utc(),
            today_value: day_value(&book.orders, today, |s| {
                marks.iter().find(|(k, _)| k == s).and_then(|(_, v)| *v)
            }),
            day_trade: self
                .draft
                .legs
                .iter()
                .any(|l| is_day_trade(&book.orders, today, &l.symbol, l.side)),
        };
        let review = request
            .as_ref()
            .ok()
            .map(|r| guard::review_spread(r, &context, &cx.config.trading));
        let net = review.as_ref().map(|r| r.net).unwrap_or_else(|| {
            mt_core::options::net_price(
                &self
                    .draft
                    .legs
                    .iter()
                    .zip(&context.legs)
                    .map(|(l, q)| (l.signed(), q.bid, q.ask))
                    .collect::<Vec<_>>(),
            )
        });
        if self.prefill
            && let Some(mid) = net.mid
        {
            let cents = mid.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
            self.draft.limit = price_text(cents.abs());
            self.draft.credit = cents.is_sign_negative() && !cents.is_zero();
            self.prefill = false;
        }
        let said = |p: Decimal| {
            if p.is_sign_negative() && !p.is_zero() {
                format!("{} credit", price_text(-p))
            } else {
                format!("{} debit", price_text(p))
            }
        };
        let units = request.as_ref().map_or(Decimal::ONE, |r| r.qty);

        widgets::section(ui, skin, "Estimate");
        Grid::new("mleg-estimate")
            .num_columns(2)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                let dash = || fmt::DASH.to_owned();
                ui.label("Net price a spread");
                ui.label(format!(
                    "natural {} · mid {} · far {}",
                    net.natural.map_or_else(dash, said),
                    net.mid.map_or_else(dash, |p| said(p.round_dp(2))),
                    net.far.map_or_else(dash, said)
                ))
                .on_hover_text(
                    "Natural: buying every leg at its ask and selling at its bid. Far: the other way round.",
                );
                ui.end_row();
                if let Some(r) = &review {
                    ui.label(if r.review.price.is_some_and(|p| p.is_sign_negative()) {
                        "Premium received"
                    } else {
                        "Premium paid"
                    });
                    ui.label(
                        RichText::new(r.review.price.map_or_else(dash, |p| {
                            portfolio::usd((p * MULTIPLIER * units).abs())
                        }))
                        .strong()
                        .color(skin.text_strong),
                    );
                    ui.end_row();
                    ui.label("Margin Alpaca holds");
                    ui.label(r.requirement.map_or_else(dash, portfolio::usd))
                        .on_hover_text("The most the opening legs can lose at expiry, premiums left out");
                    ui.end_row();
                    ui.label("Ties up");
                    ui.label(r.review.value.map_or_else(dash, portfolio::usd));
                    ui.end_row();
                    ui.label("Options buying power");
                    ui.label(match (context.account, r.review.buying_power_after) {
                        (Some(a), Some(after)) => {
                            let have = a.options_buying_power.unwrap_or(a.buying_power);
                            format!("{} → {}", portfolio::usd(have), portfolio::usd(after))
                        }
                        _ => dash(),
                    });
                    ui.end_row();
                    if let Some(p) = &r.payoff {
                        let dollars = |v: f64| {
                            from_f64(v * 100.0 * to_f64(units), 2).map_or_else(dash, portfolio::usd)
                        };
                        ui.label("At expiry");
                        ui.label(format!(
                            "most it can make {} · most it can lose {} · breakeven {}",
                            p.max_profit.map_or_else(|| "unlimited".to_owned(), dollars),
                            p.max_loss.map_or_else(|| "unlimited".to_owned(), |l| dollars(-l)),
                            if p.breakevens.is_empty() {
                                dash()
                            } else {
                                p.breakevens
                                    .iter()
                                    .map(|b| fmt::price(*b))
                                    .collect::<Vec<_>>()
                                    .join(" and ")
                            }
                        ));
                        ui.end_row();
                    } else if contracts.len() > 1 {
                        ui.label("At expiry");
                        ui.label(
                            RichText::new("The legs expire on different days, so there is no single payoff.")
                                .color(skin.text_muted),
                        );
                        ui.end_row();
                    }
                }
            });
        if let Some(p) = review.as_ref().and_then(|r| r.payoff.as_ref()) {
            payoff_chart(ui, cx, p, m.spot(), to_f64(units));
        }
        let fill = side_fill(skin, request.as_ref().map_or(OrderSide::Buy, |r| r.side));
        confirm(
            ui,
            cx,
            &mut self.sending,
            (request, review.map(|r| r.review)),
            outcome,
            &book,
            fill,
        );
        ui.label(RichText::new(opt::FEED_NOTE).small().color(skin.text_muted));
    }
}

/// Profit and loss at expiry across the underlying's price, for the whole
/// order, with the break-evens and today's price.
fn payoff_chart(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    p: &mt_core::options::Payoff,
    spot: Option<f64>,
    units: f64,
) {
    let skin = cx.skin;
    let k_lo = p.strikes.first().copied().unwrap_or(0.0);
    let k_hi = p.strikes.last().copied().unwrap_or(0.0);
    let lo_anchor = spot.map_or(k_lo, |s| s.min(k_lo));
    let hi_anchor = spot.map_or(k_hi, |s| s.max(k_hi));
    let pad = ((hi_anchor - lo_anchor) * 0.5)
        .max(hi_anchor * 0.05)
        .max(1.0);
    let (lo, hi) = ((lo_anchor - pad).max(0.0), hi_anchor + pad);
    let scale = 100.0 * units;
    let pts: Vec<[f64; 2]> = p
        .points(lo, hi)
        .into_iter()
        .map(|[x, y]| [x, y * scale])
        .filter(|[x, y]| x.is_finite() && y.is_finite())
        .collect();
    if pts.len() < 2 {
        return;
    }
    widgets::section(ui, skin, "Profit and loss at expiry");
    Plot::new("mleg-payoff")
        .height(170.0)
        .grid_color(skin.grid)
        .allow_scroll(false)
        .allow_drag(false)
        .allow_zoom(false)
        .y_axis_min_width(56.0)
        .x_axis_formatter(|m, _| fmt::price(m.value))
        .y_axis_formatter(|m, _| format!("{:+.0}", m.value))
        .label_formatter(|pos: &egui_plot::HoverPosition<'_>| {
            let v = match pos {
                egui_plot::HoverPosition::NearDataPoint { position, .. } => position,
                egui_plot::HoverPosition::Elsewhere { position } => position,
            };
            Some(format!("at {} · {:+.2}", fmt::price(v.x), v.y))
        })
        .show(ui, |plot| {
            plot.hline(
                HLine::new("Zero", 0.0)
                    .color(skin.text_muted)
                    .style(LineStyle::dashed_dense())
                    .width(1.0),
            );
            if let Some(s) = spot.filter(|s| s.is_finite()) {
                plot.vline(VLine::new("Now", s).color(skin.info).width(1.0));
            }
            plot.line(
                Line::new("At expiry", PlotPoints::from(pts))
                    .color(skin.accent)
                    .width(2.0),
            );
            let bes: Vec<[f64; 2]> = p
                .breakevens
                .iter()
                .filter(|b| b.is_finite())
                .map(|b| [*b, 0.0])
                .collect();
            if !bes.is_empty() {
                plot.points(
                    Points::new("Breakeven", PlotPoints::from(bes))
                        .color(skin.warning)
                        .radius(4.0),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn spread_commands_read_and_round_trip() {
        let d = parse_args(&args(
            "+XLU261218C00045000 -XLU261218C00047000 2 LMT 0.85 GTC",
        ))
        .unwrap();
        assert_eq!(d.legs.len(), 2);
        assert_eq!(d.legs[1].side, OrderSide::Sell);
        assert_eq!(
            (d.qty.as_str(), d.limit.as_str(), d.credit),
            ("2", "0.85", false)
        );
        assert_eq!(d.tif, TimeInForce::Gtc);
        let route = d.route();
        assert_eq!(
            route.to_string(),
            "MLEG +XLU261218C00045000 -XLU261218C00047000 2 LMT 0.85 GTC"
        );
        assert_eq!(parse_args(&route.args).unwrap(), d);
        // Credits, ratios, market orders.
        let c = parse_args(&args("-XLU261218C00045000 +XLU261218C00047000 CREDIT 0.40")).unwrap();
        assert!(c.credit && c.limit == "0.40");
        assert!(c.route().to_string().contains("LMT -0.40"));
        let n = parse_args(&args(
            "+XLU261218C00043000 -2*XLU261218C00045000 +XLU261218C00047000 MKT",
        ))
        .unwrap();
        assert_eq!(n.legs[1].ratio, 2);
        assert_eq!(n.order_type, OrderType::Market);
        assert!(n.route().to_string().contains("-2*XLU261218C00045000"));
        let bare = parse_args(&args("XLU261218C00045000 -XLU261218C00047000 @-0.30")).unwrap();
        assert_eq!(bare.legs[0].side, OrderSide::Buy);
        assert!(bare.credit);
        for bad in [
            "+XLU261218C00045000 +XLU261218C00045000",
            "+XLU261218C00045000 BANANA",
            "+XLU261218C00045000 LMT",
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad}");
        }
        assert!(parse_args(&[]).is_ok());
    }

    #[test]
    fn a_draft_becomes_an_mleg_request() {
        let d = parse_args(&args(
            "+XLU261218C00045000 -XLU261218C00047000 2 CREDIT 0.40",
        ))
        .unwrap();
        let r = d
            .request(
                "mt-s",
                &[PositionIntent::BuyToOpen, PositionIntent::SellToOpen],
            )
            .unwrap();
        assert!(r.is_multi_leg());
        assert_eq!(r.limit_price, Some("-0.40".parse().unwrap()));
        assert_eq!(r.side, OrderSide::Sell);
        assert_eq!(r.legs[1].intent, Some(PositionIntent::SellToOpen));
        let one = parse_args(&args("+XLU261218C00045000")).unwrap();
        assert!(one.request("mt-s", &[]).unwrap_err().contains("two legs"));
        assert_eq!(parse_leg("-3xXLU261218C00047000").map(|l| l.ratio), Some(3));
        assert_eq!(parse_leg("-0.40"), None);
    }
}
