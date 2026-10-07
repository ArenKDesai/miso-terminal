//! The guardrails for a single-leg option order. Shared with stocks: the
//! switch, the restricted list (an underlying's options are restricted with
//! it), what the broker would refuse, the account's standing, sessions, the
//! three caps, the fat-finger check and day trades. For options: the
//! contract is tradable and not past its expiry-day cutoff, the account's
//! options level allows the order, a sold call is covered by shares and a
//! sold put by buying power (Alpaca allows no uncovered options), the order
//! does not flip a position, the collar is measured against the quote (a
//! buy limit above the ask, a sell limit below the bid), the price is on the
//! exchanges' step, and contracts per order are capped.

use chrono::{DateTime, Utc};

use super::{Check, Level, Limits, Loaded, Review, check_loaded, dollars, pct, pct_text, positive};
use crate::account::{Account, OrderSide, PDT_DAY_TRADES, Position};
use crate::exchange::Session;
use crate::instrument::{OptionContract, OptionRight};
use crate::money::{Decimal, fmt_qty, fmt_usd};
use crate::options::{ContractInfo, MULTIPLIER, PositionIntent, expiry_cutoff};
use crate::order::{OrderRequest, OrderType, price_text};

/// What an option ticket knows besides the request.
#[derive(Clone, Debug)]
pub struct OptionContext<'a> {
    pub account: Option<&'a Account>,
    /// Whether the position and order lists have loaded.
    pub loaded: Loaded,
    /// Every position, stocks and options: what the order opens or closes,
    /// and the shares that cover a call.
    pub positions: &'a [Position],
    /// The contract as listed, once the contract list has loaded.
    pub info: Option<&'a ContractInfo>,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    pub last: Option<Decimal>,
    /// The exchange's session now.
    pub session: Session,
    /// Now, for the expiry-day cutoff.
    pub now: DateTime<Utc>,
    /// What today's orders are worth already ([`crate::order::day_value`]).
    pub today_value: Decimal,
    /// Whether this order would be a day trade ([`crate::order::is_day_trade`]).
    pub day_trade: bool,
}

impl OptionContext<'_> {
    /// Contracts held of `symbol` (negative when short).
    pub fn held(&self, symbol: &str) -> Decimal {
        self.positions
            .iter()
            .filter(|p| p.symbol.eq_ignore_ascii_case(symbol))
            .map(|p| p.qty)
            .sum()
    }
}

/// The account's options level: what it trades at, else what it is approved for.
fn level(a: &Account) -> Option<u8> {
    a.options_trading_level.or(a.options_approved_level)
}

/// The level an order needs: buying calls and puts 2, covered calls and
/// cash-secured puts 1; closing needs none.
fn level_needed(intent: PositionIntent) -> Option<(u8, &'static str)> {
    match intent {
        PositionIntent::BuyToOpen => Some((2, "Buying calls and puts")),
        PositionIntent::SellToOpen => Some((1, "Selling covered calls and cash-secured puts")),
        _ => None,
    }
}

/// Run every rule on a single-leg option order.
pub fn review_option(req: &OrderRequest, cx: &OptionContext<'_>, limits: &Limits) -> Review {
    let mut checks = Vec::new();
    let mut add = |level: Level, rule: &'static str, message: String| {
        checks.push(Check {
            level,
            rule,
            message,
        });
    };
    let Some(contract) = req.contract() else {
        add(
            Level::Block,
            "valid",
            format!("{} is not an option contract.", req.symbol),
        );
        return Review {
            checks,
            price: None,
            value: None,
            position_after: Decimal::ZERO,
            opening: Decimal::ZERO,
            buying_power_after: None,
            reviewed: req.clone(),
        };
    };
    let underlying = contract.underlying.clone();
    let symbol = contract.occ().unwrap_or_else(|| req.symbol.clone());
    let size = cx.info.map_or(MULTIPLIER, |i| i.size);
    let multiplier = cx.info.map_or(MULTIPLIER, |i| i.multiplier);

    // Switches, lists and the account.
    if !limits.enabled {
        add(
            Level::Block,
            "enabled",
            "Trading is off (the kill switch was used). Turn it back on in ORD.".into(),
        );
    }
    if limits.is_restricted(&underlying) {
        add(
            Level::Block,
            "restricted",
            format!("{underlying} is on your restricted list (SET), and so are its options."),
        );
    }
    for p in req.problems() {
        add(Level::Block, "valid", p);
    }
    match cx.account {
        None => add(
            Level::Block,
            "account",
            "The account has not loaded yet.".into(),
        ),
        Some(a) if !a.restrictions().is_empty() => add(
            Level::Block,
            "account",
            format!("The account cannot trade: {}.", a.restrictions().join(", ")),
        ),
        Some(_) => {}
    }
    check_loaded(cx.loaded, &mut add);

    // The contract.
    if let Some(info) = cx.info {
        if !info.tradable || !info.active {
            add(
                Level::Block,
                "asset",
                format!("Alpaca does not trade {symbol}."),
            );
        }
        if !info.is_standard() {
            add(
                Level::Warn,
                "asset",
                format!(
                    "An adjusted contract (root {}): it delivers {} shares, not the usual 100. Check what it is before trading it.",
                    info.root,
                    fmt_qty(info.size)
                ),
            );
        }
    }
    expiry(&contract, cx.now, &mut add);

    // What the order does to the position.
    let held = cx.held(&symbol);
    let intent = req
        .position_intent
        .unwrap_or_else(|| PositionIntent::of(req.side, held));
    let after = held + req.side.sign() * req.qty;
    let opening = if intent.opens() {
        req.qty
    } else {
        Decimal::ZERO
    };
    let crosses =
        !held.is_zero() && !after.is_zero() && held.is_sign_negative() != after.is_sign_negative();
    if crosses {
        add(
            Level::Block,
            "flip",
            format!(
                "You hold {} contracts; this would take the position to {} in one order. Close the {} first, then open the new position.",
                fmt_qty(held),
                fmt_qty(after),
                fmt_qty(held.abs())
            ),
        );
    } else if !intent.opens() {
        let free = cx
            .positions
            .iter()
            .find(|p| p.symbol.eq_ignore_ascii_case(&symbol))
            .and_then(|p| p.qty_available)
            .map_or(held.abs(), |a| a.abs());
        if held.is_zero() || req.qty > held.abs() {
            add(
                Level::Block,
                "available",
                format!(
                    "{} needs a position to close: you hold {} contracts.",
                    intent.label(),
                    fmt_qty(held)
                ),
            );
        } else if req.qty > free {
            add(
                Level::Block,
                "available",
                format!(
                    "Only {} of the {} contracts are free; the rest are held by open orders (cancel them in ORD first).",
                    fmt_qty(free),
                    fmt_qty(held.abs())
                ),
            );
        }
    } else if (intent == PositionIntent::BuyToOpen && held.is_sign_negative() && !held.is_zero())
        || (intent == PositionIntent::SellToOpen && held > Decimal::ZERO)
    {
        add(
            Level::Block,
            "flip",
            format!(
                "{} while you hold {} contracts: close the position instead.",
                intent.label(),
                fmt_qty(held)
            ),
        );
    }

    // The options level.
    if let (Some(a), Some((need, what))) = (cx.account, level_needed(intent))
        && let Some(have) = level(a)
        && have < need
    {
        add(
            Level::Block,
            "options_level",
            format!("{what} needs options level {need}; the account has level {have} (ACCT)."),
        );
    }

    // Prices and the order's value.
    let market_price = match req.side {
        OrderSide::Buy => positive(cx.ask),
        OrderSide::Sell => positive(cx.bid),
    }
    .or(positive(cx.last));
    let price = req.reference_price(market_price);
    let value = price.map(|p| (p * req.qty * multiplier).round_dp(2));

    // Sold options must be covered.
    let mut buying_power_needed = Decimal::ZERO;
    if intent == PositionIntent::SellToOpen && !crosses {
        match contract.right {
            OptionRight::Call => {
                let stock = cx
                    .positions
                    .iter()
                    .find(|p| p.symbol.eq_ignore_ascii_case(&underlying) && !p.is_option());
                let held = stock.map_or(Decimal::ZERO, |p| p.qty.max(Decimal::ZERO));
                let shares = stock.map_or(Decimal::ZERO, |p| {
                    p.qty_available.unwrap_or(p.qty).max(Decimal::ZERO)
                });
                let covering: Decimal = cx
                    .positions
                    .iter()
                    .filter(|p| p.is_short())
                    .filter_map(|p| Some((p.contract()?, p.qty)))
                    .filter(|(c, _)| c.underlying == underlying && c.right == OptionRight::Call)
                    .map(|(_, q)| q.abs() * MULTIPLIER)
                    .sum();
                let free = (shares - covering).max(Decimal::ZERO);
                let need = req.qty * size;
                if free < need {
                    let mut why = Vec::new();
                    if held > shares {
                        why.push(format!("open orders hold {}", fmt_qty(held - shares)));
                    }
                    if covering > Decimal::ZERO {
                        why.push(format!("{} cover calls already sold", fmt_qty(covering)));
                    }
                    let why = if why.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", why.join("; "))
                    };
                    add(
                        Level::Block,
                        "covered",
                        format!(
                            "Alpaca allows only covered calls: {} contracts need {} shares of {underlying}, and {} of the {} you hold are free{why}.",
                            fmt_qty(req.qty),
                            fmt_qty(need),
                            fmt_qty(free),
                            fmt_qty(held)
                        ),
                    );
                } else {
                    add(
                        Level::Pass,
                        "covered",
                        format!(
                            "Covered by {} of your {} {underlying} shares.",
                            fmt_qty(need),
                            fmt_qty(held)
                        ),
                    );
                }
            }
            OptionRight::Put => {
                // Cash-secured: the strike's worth of buying power, less the premium.
                let secured = (contract.strike * req.qty * size).round_dp(2);
                buying_power_needed = (secured - value.unwrap_or(Decimal::ZERO)).max(Decimal::ZERO);
                add(
                    Level::Pass,
                    "covered",
                    format!(
                        "A cash-secured put: {} at the strike, less the premium.",
                        fmt_usd(secured, 2)
                    ),
                );
            }
        }
    }

    // Sessions: options trade in the regular session only.
    match (req.order_type, cx.session) {
        (_, Session::Regular) => {}
        (OrderType::Market, _) => add(
            Level::Block,
            "session",
            "Market orders only in the regular session (09:30 to 16:00 New York). Use a limit order."
                .into(),
        ),
        (_, Session::Closed) => add(
            Level::Warn,
            "session",
            "The market is closed: this order waits for the next session.".into(),
        ),
        _ => add(
            Level::Warn,
            "session",
            "Options trade 09:30 to 16:00 New York: this order waits for the next session.".into(),
        ),
    }

    // Value and caps.
    match value {
        None => add(
            Level::Block,
            "price",
            format!("No quote for {symbol} yet, so the order cannot be valued."),
        ),
        Some(v) => {
            let cap = dollars(limits.max_order_value);
            if limits.max_order_value > 0 {
                add(
                    if v > cap { Level::Block } else { Level::Pass },
                    "order_cap",
                    format!(
                        "Worth {} in premium, {} the per-order cap of {}.",
                        fmt_usd(v, 2),
                        if v > cap { "more than" } else { "within" },
                        fmt_usd(cap, 0)
                    ),
                );
            }
            if limits.max_daily_value > 0 {
                let cap = dollars(limits.max_daily_value);
                let total = cx.today_value + v;
                add(
                    if total > cap {
                        Level::Block
                    } else {
                        Level::Pass
                    },
                    "daily_cap",
                    format!(
                        "Today's orders would come to {} of the daily cap of {}.",
                        fmt_usd(total, 2),
                        fmt_usd(cap, 0)
                    ),
                );
            }
        }
    }
    if let Some(p) = price
        && opening > Decimal::ZERO
        && limits.max_position_value > 0
    {
        let worth = (after.abs() * p * multiplier).round_dp(2);
        let cap = dollars(limits.max_position_value);
        add(
            if worth > cap {
                Level::Block
            } else {
                Level::Pass
            },
            "position_cap",
            format!(
                "The position would be {} contracts, worth {} (the cap is {}).",
                fmt_qty(after),
                fmt_usd(worth, 2),
                fmt_usd(cap, 0)
            ),
        );
    }

    // The collar, against the quote: paying above the ask, or selling below the bid.
    collar(req, cx, limits, &mut add);
    // The exchanges' price step.
    if let Some(info) = cx.info {
        for (name, p) in [("limit", req.limit_price), ("stop", req.stop_price)] {
            let Some(p) = p.filter(|_| match name {
                "limit" => req.order_type.needs_limit(),
                _ => req.order_type.needs_stop(),
            }) else {
                continue;
            };
            let step = info.tick(p);
            if !(p % step).is_zero() {
                add(
                    Level::Warn,
                    "tick",
                    format!(
                        "{symbol} is quoted in steps of {} at this price; a {name} of {} may be routed elsewhere or refused.",
                        price_text(step),
                        price_text(p)
                    ),
                );
            }
        }
    }

    // Size.
    if limits.max_contracts > 0 && req.qty > dollars(limits.max_contracts) {
        add(
            Level::Block,
            "max_contracts",
            format!(
                "{} contracts is more than the {} a single order may be for.",
                fmt_qty(req.qty),
                limits.max_contracts
            ),
        );
    }
    if let (Some(v), Some(a), Some(limit)) = (value, cx.account, pct(limits.fat_finger_pct))
        && a.equity > Decimal::ZERO
        && opening > Decimal::ZERO
    {
        let share = (v / a.equity * Decimal::ONE_HUNDRED).round_dp(1);
        if share > limit {
            add(
                Level::Warn,
                "fat_finger",
                format!(
                    "This order is {} of equity (more than {}): check the quantity.",
                    pct_text(share),
                    pct_text(limit)
                ),
            );
        }
    }
    if req.order_type == OrderType::Market
        && let (Some(b), Some(a)) = (positive(cx.bid), positive(cx.ask))
    {
        let mid = (a + b) / Decimal::TWO;
        if mid > Decimal::ZERO && (a - b) / mid > Decimal::new(10, 2) {
            add(
                Level::Warn,
                "spread",
                format!(
                    "The quote is {} wide ({} of the mid): a market order may fill far from it. A limit order is safer.",
                    price_text(a - b),
                    pct_text(((a - b) / mid * Decimal::ONE_HUNDRED).round_dp(0))
                ),
            );
        }
    }

    // Buying power: the premium for what is bought, a sold put's strike.
    if intent == PositionIntent::BuyToOpen {
        buying_power_needed = value.unwrap_or(Decimal::ZERO);
    }
    let mut buying_power_after = None;
    if let Some(a) = cx.account {
        let have = a.options_buying_power.unwrap_or(a.buying_power);
        buying_power_after = Some(have - buying_power_needed);
        if buying_power_needed > have {
            add(
                Level::Block,
                "buying_power",
                format!(
                    "Needs {} of options buying power; the account has {}.",
                    fmt_usd(buying_power_needed, 2),
                    fmt_usd(have, 2)
                ),
            );
        }
    }

    // Day trades below $25,000.
    if cx.day_trade
        && let Some(a) = cx.account
    {
        match a.day_trades_left() {
            Some(0) => add(
                Level::Block,
                "day_trade",
                format!(
                    "This would be a day trade, and all {PDT_DAY_TRADES} allowed in five business days below $25,000 are used."
                ),
            ),
            Some(n) => add(
                Level::Warn,
                "day_trade",
                format!(
                    "This would be a day trade: {n} of {PDT_DAY_TRADES} left in five business days below $25,000."
                ),
            ),
            None => {}
        }
    }

    Review {
        checks,
        price,
        value,
        position_after: after,
        opening,
        buying_power_after,
        reviewed: req.clone(),
    }
}

/// Expired contracts, and the expiry day's cutoff and automatic exercise.
pub(super) fn expiry(
    contract: &OptionContract,
    now: DateTime<Utc>,
    add: &mut impl FnMut(Level, &'static str, String),
) {
    let local = crate::exchange::to_exchange(now);
    let today = local.date_naive();
    let cutoff = expiry_cutoff(&contract.underlying);
    if contract.expiry < today {
        add(
            Level::Block,
            "expiry",
            format!(
                "This contract expired on {}.",
                contract.expiry.format("%b %d")
            ),
        );
    } else if contract.expiry == today {
        if local.time() >= cutoff {
            add(
                Level::Block,
                "expiry",
                format!(
                    "It expires today, and Alpaca takes no orders for expiring {} contracts after {} New York time.",
                    contract.underlying,
                    cutoff.format("%H:%M")
                ),
            );
        } else {
            add(
                Level::Warn,
                "expiry",
                format!(
                    "It expires today at the close. Alpaca takes orders until {} New York time; contracts in the money by a cent or more are exercised automatically, and a position the account cannot afford to exercise may be sold in the last hour.",
                    cutoff.format("%H:%M")
                ),
            );
        }
    }
}

/// A buy limit above the ask, or a sell limit below the bid, pays away the
/// spread: a warning, and beyond the collar a block. Passive limits pass.
fn collar(
    req: &OrderRequest,
    cx: &OptionContext<'_>,
    limits: &Limits,
    add: &mut impl FnMut(Level, &'static str, String),
) {
    let Some(limit) = req.limit_price.filter(|_| req.order_type.needs_limit()) else {
        return;
    };
    let Some(collar) = pct(limits.collar_pct) else {
        return;
    };
    let (quote, beyond, word) = match req.side {
        OrderSide::Buy => (
            positive(cx.ask),
            limit - positive(cx.ask).unwrap_or(limit),
            "above the ask",
        ),
        OrderSide::Sell => (
            positive(cx.bid),
            positive(cx.bid).unwrap_or(limit) - limit,
            "below the bid",
        ),
    };
    let Some(quote) = quote else {
        add(
            Level::Warn,
            "collar",
            format!(
                "No {} to check the limit {} against.",
                match req.side {
                    OrderSide::Buy => "ask",
                    OrderSide::Sell => "bid",
                },
                price_text(limit)
            ),
        );
        return;
    };
    // The collar's width: a percentage of the quote, at least five cents.
    let room = (quote * collar / Decimal::ONE_HUNDRED)
        .round_dp(2)
        .max(Decimal::new(5, 2));
    if beyond <= Decimal::ZERO {
        add(
            Level::Pass,
            "collar",
            format!(
                "The limit {} is within the quote ({} {}).",
                price_text(limit),
                match req.side {
                    OrderSide::Buy => "ask",
                    OrderSide::Sell => "bid",
                },
                price_text(quote)
            ),
        );
    } else if beyond > room {
        add(
            Level::Block,
            "collar",
            format!(
                "The limit {} is {} {word} {} (the collar allows {}).",
                price_text(limit),
                price_text(beyond),
                price_text(quote),
                price_text(room)
            ),
        );
    } else {
        add(
            Level::Warn,
            "collar",
            format!(
                "The limit {} is {} {word} {}: it pays away more than the quote.",
                price_text(limit),
                price_text(beyond),
                price_text(quote)
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::AssetClass;
    use crate::options::Style;
    use crate::order::TimeInForce;
    use crate::order::tests::d;

    const CALL: &str = "XLU261218C00045000";
    const PUT: &str = "XLU261218P00044000";

    fn account() -> Account {
        Account {
            status: "ACTIVE".into(),
            equity: d("100000"),
            last_equity: d("99000"),
            buying_power: d("150000"),
            options_buying_power: Some(d("50000")),
            options_trading_level: Some(3),
            shorting_enabled: true,
            ..Account::default()
        }
    }

    fn info(symbol: &str) -> ContractInfo {
        let contract = OptionContract::parse_occ(symbol).unwrap();
        ContractInfo {
            symbol: symbol.into(),
            root: "XLU".into(),
            underlying: "XLU".into(),
            name: String::new(),
            tradable: true,
            active: true,
            style: Style::American,
            multiplier: MULTIPLIER,
            size: MULTIPLIER,
            open_interest: None,
            open_interest_date: None,
            close_price: None,
            close_price_date: None,
            penny: true,
            contract,
        }
    }

    fn position(symbol: &str, qty: &str) -> Position {
        let q = d(qty);
        Position {
            symbol: symbol.into(),
            class: if symbol.len() > 10 {
                AssetClass::Option
            } else {
                AssetClass::Equity
            },
            exchange: String::new(),
            qty: q,
            qty_available: Some(q),
            avg_entry_price: d("1"),
            cost_basis: q,
            market_value: None,
            current_price: None,
            lastday_price: None,
            change_today: None,
            unrealized_pl: None,
            unrealized_plpc: None,
            unrealized_intraday_pl: None,
            unrealized_intraday_plpc: None,
        }
    }

    fn order(symbol: &str, side: OrderSide, qty: &str, limit: &str) -> OrderRequest {
        OrderRequest {
            client_order_id: "mt-test".into(),
            symbol: symbol.into(),
            side,
            qty: d(qty),
            order_type: OrderType::Limit,
            limit_price: Some(d(limit)),
            stop_price: None,
            tif: TimeInForce::Day,
            extended_hours: false,
            position_intent: None,
            legs: Vec::new(),
        }
    }

    /// Friday Oct 2 2026, 11:00 New York time.
    fn morning() -> DateTime<Utc> {
        "2026-10-02T15:00:00Z".parse().unwrap()
    }

    fn cx<'a>(
        a: &'a Account,
        positions: &'a [Position],
        info: &'a ContractInfo,
    ) -> OptionContext<'a> {
        OptionContext {
            account: Some(a),
            loaded: Loaded::ALL,
            positions,
            info: Some(info),
            bid: Some(d("1.55")),
            ask: Some(d("1.65")),
            last: Some(d("1.60")),
            session: Session::Regular,
            now: morning(),
            today_value: Decimal::ZERO,
            day_trade: false,
        }
    }

    #[test]
    fn buying_a_call_is_valued_per_contract() {
        let (a, i) = (account(), info(CALL));
        let r = review_option(
            &order(CALL, OrderSide::Buy, "2", "1.60"),
            &cx(&a, &[], &i),
            &Limits::default(),
        );
        assert!(!r.blocked() && r.can_confirm(false), "{:#?}", r.checks);
        assert_eq!(r.value, Some(d("320.00")));
        assert_eq!(r.opening, d("2"));
        assert_eq!(
            r.buying_power_after,
            Some(d("49680.00")),
            "options buying power"
        );
        assert!(r.has("collar", Level::Pass));
        // A market buy is valued at the ask.
        let mut m = order(CALL, OrderSide::Buy, "2", "1.60");
        m.order_type = OrderType::Market;
        m.limit_price = None;
        assert_eq!(
            review_option(&m, &cx(&a, &[], &i), &Limits::default()).value,
            Some(d("330.00"))
        );
        // Level 1 cannot buy options.
        let l1 = Account {
            options_trading_level: Some(1),
            ..account()
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "2", "1.60"),
                &cx(&l1, &[], &i),
                &Limits::default()
            )
            .has("options_level", Level::Block)
        );
        // Restricting the stock restricts its options.
        let restricted = Limits {
            restricted: vec!["XLU".into()],
            ..Limits::default()
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "2", "1.60"),
                &cx(&a, &[], &i),
                &restricted
            )
            .has("restricted", Level::Block)
        );
        // The positions and orders must have loaded: the caps count them.
        let waiting = OptionContext {
            loaded: Loaded {
                positions: true,
                orders: false,
            },
            ..cx(&a, &[], &i)
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "2", "1.60"),
                &waiting,
                &Limits::default()
            )
            .has("loaded", Level::Block)
        );
        // Not enough options buying power.
        let poor = Account {
            options_buying_power: Some(d("100")),
            ..account()
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "2", "1.60"),
                &cx(&poor, &[], &i),
                &Limits::default()
            )
            .has("buying_power", Level::Block)
        );
    }

    #[test]
    fn sold_calls_need_shares_and_sold_puts_cash() {
        let a = account();
        let (call, put) = (info(CALL), info(PUT));
        let lim = Limits::default();
        let sell_call = order(CALL, OrderSide::Sell, "2", "1.55");
        // No shares: uncovered, refused.
        let r = review_option(&sell_call, &cx(&a, &[], &call), &lim);
        assert!(r.has("covered", Level::Block), "{:#?}", r.checks);
        // 200 shares cover two contracts…
        let held = [position("XLU", "200")];
        let r = review_option(&sell_call, &cx(&a, &held, &call), &lim);
        assert!(
            r.has("covered", Level::Pass) && !r.blocked(),
            "{:#?}",
            r.checks
        );
        assert_eq!(r.position_after, d("-2"));
        // …unless a call is already sold against them.
        let some_sold = [position("XLU", "200"), position("XLU261120C00046000", "-1")];
        assert!(
            review_option(&sell_call, &cx(&a, &some_sold, &call), &lim)
                .has("covered", Level::Block)
        );
        // A put is secured by buying power: 2 × 44 × 100 less the premium.
        let sell_put = order(PUT, OrderSide::Sell, "2", "1.30");
        let quoted = |a| OptionContext {
            bid: Some(d("1.25")),
            ask: Some(d("1.35")),
            last: Some(d("1.30")),
            ..cx(a, &[], &put)
        };
        let r = review_option(&sell_put, &quoted(&a), &lim);
        assert!(!r.blocked(), "{:#?}", r.checks);
        assert_eq!(
            r.buying_power_after,
            Some(d("50000") - d("8800") + d("260.00"))
        );
        let poor = Account {
            options_buying_power: Some(d("5000")),
            ..account()
        };
        assert!(review_option(&sell_put, &quoted(&poor), &lim).has("buying_power", Level::Block));
    }

    #[test]
    fn closing_needs_a_position_and_never_flips() {
        let a = account();
        let i = info(CALL);
        let lim = Limits::default();
        let long = [position(CALL, "2")];
        // Selling what is held closes it, and needs no level or cover.
        let mut close = order(CALL, OrderSide::Sell, "2", "1.55");
        close.position_intent = Some(PositionIntent::SellToClose);
        let l0 = Account {
            options_trading_level: Some(0),
            ..account()
        };
        let r = review_option(&close, &cx(&l0, &long, &i), &lim);
        assert!(!r.blocked(), "{:#?}", r.checks);
        assert_eq!(
            (r.position_after, r.opening),
            (Decimal::ZERO, Decimal::ZERO)
        );
        // Selling three of two would flip the position.
        let flip = order(CALL, OrderSide::Sell, "3", "1.55");
        assert!(review_option(&flip, &cx(&a, &long, &i), &lim).has("flip", Level::Block));
        // Closing what is not held is refused.
        assert!(review_option(&close, &cx(&a, &[], &i), &lim).has("available", Level::Block));
        // Contracts held by an open order are not free.
        let mut held_back = position(CALL, "2");
        held_back.qty_available = Some(d("1"));
        let busy = [held_back];
        assert!(review_option(&close, &cx(&a, &busy, &i), &lim).has("available", Level::Block));
    }

    #[test]
    fn the_quote_collar_ticks_and_size() {
        let (a, i) = (account(), info(CALL));
        let lim = Limits::default();
        // Ask 1.65; the collar is 5% of it, at least 0.05.
        let warn = review_option(
            &order(CALL, OrderSide::Buy, "1", "1.68"),
            &cx(&a, &[], &i),
            &lim,
        );
        assert!(
            warn.has("collar", Level::Warn) && !warn.blocked(),
            "{:#?}",
            warn.checks
        );
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "1", "1.75"),
                &cx(&a, &[], &i),
                &lim
            )
            .has("collar", Level::Block)
        );
        // Bidding below the market is harmless.
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "1", "0.90"),
                &cx(&a, &[], &i),
                &lim
            )
            .has("collar", Level::Pass)
        );
        // A non-penny contract steps by 0.05 below $3.
        let nickel = ContractInfo {
            penny: false,
            ..info(CALL)
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "1", "1.61"),
                &cx(&a, &[], &nickel),
                &lim
            )
            .has("tick", Level::Warn)
        );
        // No quote: a warning for a limit, a block for a market order.
        let blind = OptionContext {
            bid: None,
            ask: None,
            last: None,
            ..cx(&a, &[], &i)
        };
        assert!(
            review_option(&order(CALL, OrderSide::Buy, "1", "1.60"), &blind, &lim)
                .has("collar", Level::Warn)
        );
        let mut m = order(CALL, OrderSide::Buy, "1", "1.60");
        m.order_type = OrderType::Market;
        assert!(review_option(&m, &blind, &lim).has("price", Level::Block));
        // Contracts per order.
        let few = Limits {
            max_contracts: 5,
            max_order_value: 0,
            ..Limits::default()
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "6", "1.60"),
                &cx(&a, &[], &i),
                &few
            )
            .has("max_contracts", Level::Block)
        );
    }

    #[test]
    fn sessions_and_expiry_day() {
        let a = account();
        let lim = Limits::default();
        let i = info(CALL);
        let pre = OptionContext {
            session: Session::PreMarket,
            ..cx(&a, &[], &i)
        };
        let mut m = order(CALL, OrderSide::Buy, "1", "1.60");
        m.order_type = OrderType::Market;
        m.limit_price = None;
        assert!(review_option(&m, &pre, &lim).has("session", Level::Block));
        assert!(
            review_option(&order(CALL, OrderSide::Buy, "1", "1.60"), &pre, &lim)
                .has("session", Level::Warn)
        );
        // A contract expiring today: a warning before 15:15, a block after.
        let today = "XLU261002C00045000";
        let ti = info(today);
        let r = review_option(
            &order(today, OrderSide::Buy, "1", "0.20"),
            &cx(&a, &[], &ti),
            &lim,
        );
        assert!(r.has("expiry", Level::Warn), "{:#?}", r.checks);
        let late = OptionContext {
            now: "2026-10-02T19:20:00Z".parse().unwrap(),
            ..cx(&a, &[], &ti)
        };
        assert!(
            review_option(&order(today, OrderSide::Buy, "1", "0.20"), &late, &lim)
                .has("expiry", Level::Block)
        );
        let gone = "XLU260918C00046000";
        let gi = info(gone);
        assert!(
            review_option(
                &order(gone, OrderSide::Buy, "1", "0.20"),
                &cx(&a, &[], &gi),
                &lim
            )
            .has("expiry", Level::Block)
        );
        // Untradable contracts are refused.
        let halted = ContractInfo {
            tradable: false,
            ..info(CALL)
        };
        assert!(
            review_option(
                &order(CALL, OrderSide::Buy, "1", "1.60"),
                &cx(&a, &[], &halted),
                &lim
            )
            .has("asset", Level::Block)
        );
    }
}
