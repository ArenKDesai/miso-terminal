//! The guardrails for a multi-leg option order (a spread). Shared with the
//! other tickets: the switch, the restricted list, what the broker would
//! refuse, the account's standing, sessions, the order and daily caps, the
//! fat-finger check and day trades. For spreads: options level 3 to open
//! one; each leg's contract tradable and not past its expiry-day cutoff;
//! each leg opening or closing as the positions allow (never flipping one);
//! every sold leg covered by a bought one in the same order and expiry, as
//! Alpaca requires; the margin Alpaca's universal spread rule holds plus the
//! net premium against options buying power; a collar on the net price
//! against crossing every leg's spread (the natural price); and contracts
//! per order.

use chrono::{DateTime, Utc};

use super::{
    Check, Level, Limits, Loaded, Review, check_loaded, check_price_age, dollars, options::expiry,
    pct, pct_text,
};
use crate::account::{Account, PDT_DAY_TRADES, Position, from_f64};
use crate::exchange::Session;
use crate::money::{Decimal, fmt_qty, fmt_usd};
use crate::options::{
    ContractInfo, Leg, MULTIPLIER, NetPrice, Payoff, PositionIntent, net_price, payoff, requirement,
};
use crate::order::{OrderRequest, OrderType, price_text};

/// One leg's listing and quote.
#[derive(Clone, Debug, Default)]
pub struct LegMarket<'a> {
    pub info: Option<&'a ContractInfo>,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    /// When the leg's quote is from.
    pub quoted_at: Option<DateTime<Utc>>,
}

/// What a spread ticket knows besides the request.
#[derive(Clone, Debug)]
pub struct SpreadContext<'a> {
    pub account: Option<&'a Account>,
    /// Whether the position and order lists have loaded.
    pub loaded: Loaded,
    /// Every position: what each leg opens or closes.
    pub positions: &'a [Position],
    /// Each leg's listing and quote, in the request's order.
    pub legs: Vec<LegMarket<'a>>,
    pub session: Session,
    pub now: DateTime<Utc>,
    pub today_value: Decimal,
    /// Whether any leg would be a day trade.
    pub day_trade: bool,
}

/// The verdicts, and what the ticket shows besides them.
#[derive(Clone, Debug)]
pub struct SpreadReview {
    /// `price` is the net price a unit (positive a debit), `value` what the
    /// order ties up: the margin plus the net premium, for every unit.
    pub review: Review,
    /// What each leg does, in the request's order.
    pub intents: Vec<PositionIntent>,
    pub net: NetPrice,
    /// The margin Alpaca holds for the opening legs, for the whole order.
    pub requirement: Option<Decimal>,
    /// What it pays at expiry, per unit and share (one expiry only).
    pub payoff: Option<Payoff>,
}

fn held(positions: &[Position], symbol: &str) -> (Decimal, Option<Decimal>) {
    positions
        .iter()
        .find(|p| p.symbol.eq_ignore_ascii_case(symbol))
        .map_or((Decimal::ZERO, None), |p| (p.qty, p.qty_available))
}

/// Run every rule on a multi-leg order.
pub fn review_spread(req: &OrderRequest, cx: &SpreadContext<'_>, limits: &Limits) -> SpreadReview {
    let mut checks = Vec::new();
    let mut add = |level: Level, rule: &'static str, message: String| {
        checks.push(Check {
            level,
            rule,
            message,
        });
    };

    // Switches, lists and the account.
    if !limits.enabled {
        add(
            Level::Block,
            "enabled",
            "Trading is off (the kill switch was used). Turn it back on in ORD.".into(),
        );
    }
    let mut underlyings: Vec<String> = req
        .legs
        .iter()
        .filter_map(|l| l.contract().map(|c| c.underlying))
        .collect();
    underlyings.dedup();
    for u in &underlyings {
        if limits.is_restricted(u) {
            add(
                Level::Block,
                "restricted",
                format!("{u} is on your restricted list (SET), and so are its options."),
            );
        }
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

    // Each leg: its contract, and what it does to the position.
    let mut intents = Vec::new();
    let mut opening: Vec<Leg> = Vec::new();
    let mut seen_expiry = Vec::new();
    for (i, leg) in req.legs.iter().enumerate() {
        let n = i + 1;
        let market = cx.legs.get(i).cloned().unwrap_or_default();
        if let Some(info) = market.info {
            if !info.tradable || !info.active {
                add(
                    Level::Block,
                    "asset",
                    format!("Leg {n}: Alpaca does not trade {}.", leg.symbol),
                );
            }
            if !info.is_standard() {
                add(
                    Level::Warn,
                    "asset",
                    format!(
                        "Leg {n} is an adjusted contract (root {}), delivering {} shares: check it before trading it.",
                        info.root,
                        fmt_qty(info.size)
                    ),
                );
            }
        }
        if let Some(c) = leg.contract()
            && !seen_expiry.contains(&(c.underlying.clone(), c.expiry))
        {
            seen_expiry.push((c.underlying.clone(), c.expiry));
            expiry(&c, cx.now, &mut add);
        }
        let (have, available) = held(cx.positions, &leg.symbol);
        let intent = leg
            .intent
            .unwrap_or_else(|| PositionIntent::of(leg.side, have));
        intents.push(intent);
        let contracts = req.qty * Decimal::from(leg.ratio);
        if intent.opens() {
            let against = (intent == PositionIntent::BuyToOpen && have < Decimal::ZERO)
                || (intent == PositionIntent::SellToOpen && have > Decimal::ZERO);
            if against {
                add(
                    Level::Block,
                    "flip",
                    format!(
                        "Leg {n}: {} while you hold {} contracts; close the position instead.",
                        intent.label(),
                        fmt_qty(have)
                    ),
                );
            }
            opening.push(Leg {
                intent: Some(intent),
                ..leg.clone()
            });
        } else if have.is_zero() || contracts > have.abs() {
            add(
                Level::Block,
                "available",
                format!(
                    "Leg {n}: {} {} contracts, but you hold {}. Alpaca refuses an order that flips a position.",
                    intent.label(),
                    fmt_qty(contracts),
                    fmt_qty(have)
                ),
            );
        } else if let Some(free) = available.map(|a| a.abs())
            && contracts > free
        {
            add(
                Level::Block,
                "available",
                format!(
                    "Leg {n}: only {} of the {} contracts are free; open orders hold the rest (cancel them in ORD first).",
                    fmt_qty(free),
                    fmt_qty(have.abs())
                ),
            );
        }
    }

    // The options level: spreads are level 3.
    if !opening.is_empty()
        && let Some(a) = cx.account
        && let Some(have) = a.options_trading_level.or(a.options_approved_level)
        && have < 3
    {
        add(
            Level::Block,
            "options_level",
            format!("Spreads need options level 3; the account has level {have} (ACCT)."),
        );
    }

    // Every sold leg covered, and the margin that holds.
    let units = req.qty.max(Decimal::ZERO);
    let mut margin = None;
    if !opening.is_empty() {
        match requirement(&opening) {
            Ok(per_share) => {
                let m = from_f64(per_share, 2).unwrap_or_default() * MULTIPLIER * units;
                add(
                    Level::Pass,
                    "covered",
                    if m.is_zero() {
                        "Every sold leg is covered; Alpaca holds no margin beyond the premium."
                            .to_owned()
                    } else {
                        format!(
                            "Every sold leg is covered; Alpaca holds {} of margin (the most the legs can lose at expiry).",
                            fmt_usd(m, 2)
                        )
                    },
                );
                margin = Some(m);
            }
            Err(e) => add(Level::Block, "covered", e),
        }
    } else {
        margin = Some(Decimal::ZERO);
    }

    // Sessions: options trade 09:30 to 16:00 New York.
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
    // The net price is only as fresh as its oldest leg.
    let priced_at = cx
        .legs
        .iter()
        .map(|l| l.quoted_at)
        .collect::<Option<Vec<_>>>()
        .and_then(|times| times.into_iter().min());
    check_price_age(
        req.order_type,
        cx.session == Session::Regular,
        priced_at,
        cx.now,
        limits,
        &mut add,
    );

    // Net prices, and what the order ties up.
    let quotes: Vec<(i64, Option<Decimal>, Option<Decimal>)> = req
        .legs
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let m = cx.legs.get(i).cloned().unwrap_or_default();
            (l.signed(), m.bid, m.ask)
        })
        .collect();
    let net = net_price(&quotes);
    let price = req.reference_price(net.natural);
    let premium = price.map(|p| (p * MULTIPLIER * units).round_dp(2));
    let value = premium.zip(margin).map(|(p, m)| (p + m).max(Decimal::ZERO));
    match value {
        None => add(
            Level::Block,
            "price",
            "Not every leg has a quote yet, so the order cannot be valued.".into(),
        ),
        Some(v) => {
            if limits.max_order_value > 0 {
                let cap = dollars(limits.max_order_value);
                add(
                    if v > cap { Level::Block } else { Level::Pass },
                    "order_cap",
                    format!(
                        "Ties up {} (premium and margin), {} the per-order cap of {}.",
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

    // The collar: the net price against crossing every leg's spread.
    if req.order_type == OrderType::Limit
        && let (Some(limit), Some(c)) = (req.limit_price, pct(limits.collar_pct))
    {
        match net.natural {
            None => add(
                Level::Warn,
                "collar",
                "Not every leg has a bid and ask to check the net price against.".into(),
            ),
            Some(natural) => {
                let scale = natural
                    .abs()
                    .max(net.mid.map_or(Decimal::ZERO, |m| m.abs()));
                let room = (scale * c / Decimal::ONE_HUNDRED)
                    .round_dp(2)
                    .max(Decimal::new(5, 2));
                let beyond = limit - natural;
                let said = |p: Decimal| {
                    if p < Decimal::ZERO {
                        format!("{} credit", price_text(-p))
                    } else {
                        format!("{} debit", price_text(p))
                    }
                };
                if beyond <= Decimal::ZERO {
                    add(
                        Level::Pass,
                        "collar",
                        format!(
                            "The net {} is within the natural price ({}).",
                            said(limit),
                            said(natural)
                        ),
                    );
                } else if beyond > room {
                    add(
                        Level::Block,
                        "collar",
                        format!(
                            "The net {} gives up {} more than crossing every leg's spread ({}); the collar allows {}.",
                            said(limit),
                            price_text(beyond),
                            said(natural),
                            price_text(room)
                        ),
                    );
                } else {
                    add(
                        Level::Warn,
                        "collar",
                        format!(
                            "The net {} gives up {} more than crossing every leg's spread ({}).",
                            said(limit),
                            price_text(beyond),
                            said(natural)
                        ),
                    );
                }
            }
        }
    }

    // Size.
    let biggest = req.legs.iter().map(|l| l.ratio).max().unwrap_or(0);
    let contracts = units * Decimal::from(biggest);
    if limits.max_contracts > 0 && contracts > dollars(limits.max_contracts) {
        add(
            Level::Block,
            "max_contracts",
            format!(
                "{} contracts in a leg is more than the {} a single order may be for.",
                fmt_qty(contracts),
                limits.max_contracts
            ),
        );
    }
    if let (Some(v), Some(a), Some(limit)) = (value, cx.account, pct(limits.fat_finger_pct))
        && a.equity > Decimal::ZERO
        && !opening.is_empty()
    {
        let share = (v / a.equity * Decimal::ONE_HUNDRED).round_dp(1);
        if share > limit {
            add(
                Level::Warn,
                "fat_finger",
                format!(
                    "This order ties up {} of equity (more than {}): check the quantity.",
                    pct_text(share),
                    pct_text(limit)
                ),
            );
        }
    }

    // Buying power for what the opening legs tie up.
    let mut buying_power_after = None;
    if let Some(a) = cx.account {
        let have = a.options_buying_power.unwrap_or(a.buying_power);
        let needed = if opening.is_empty() {
            Decimal::ZERO
        } else {
            value.unwrap_or(Decimal::ZERO)
        };
        buying_power_after = Some(have - needed);
        if needed > have {
            add(
                Level::Block,
                "buying_power",
                format!(
                    "Needs {} of options buying power; the account has {}.",
                    fmt_usd(needed, 2),
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

    SpreadReview {
        payoff: price.and_then(|p| payoff(&req.legs, crate::account::to_f64(p))),
        review: Review {
            checks,
            price,
            value,
            position_after: Decimal::ZERO,
            opening: if opening.is_empty() {
                Decimal::ZERO
            } else {
                units
            },
            buying_power_after,
            reviewed: req.clone(),
        },
        intents,
        net,
        requirement: margin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AssetClass, OrderSide};
    use crate::order::TimeInForce;
    use crate::order::tests::d;

    const C45: &str = "XLU261218C00045000";
    const C47: &str = "XLU261218C00047000";

    fn account() -> Account {
        Account {
            status: "ACTIVE".into(),
            equity: d("100000"),
            last_equity: d("99000"),
            buying_power: d("150000"),
            options_buying_power: Some(d("50000")),
            options_trading_level: Some(3),
            ..Account::default()
        }
    }

    fn spread(legs: &[(&str, OrderSide, u32)], qty: &str, limit: Option<&str>) -> OrderRequest {
        OrderRequest {
            client_order_id: "mt-test".into(),
            symbol: String::new(),
            side: OrderSide::Buy,
            qty: d(qty),
            order_type: if limit.is_some() {
                OrderType::Limit
            } else {
                OrderType::Market
            },
            limit_price: limit.map(d),
            stop_price: None,
            tif: TimeInForce::Day,
            extended_hours: false,
            position_intent: None,
            legs: legs
                .iter()
                .map(|(s, side, r)| Leg::new(s, *side, *r))
                .collect(),
        }
    }

    /// The 45 call 1.55/1.65 and the 47 call 0.77/0.84.
    fn cx<'a>(a: &'a Account, positions: &'a [Position], n: usize) -> SpreadContext<'a> {
        let quotes = [
            ("1.55", "1.65"),
            ("0.77", "0.84"),
            ("0.30", "0.35"),
            ("0.10", "0.14"),
        ];
        SpreadContext {
            account: Some(a),
            loaded: Loaded::ALL,
            positions,
            legs: quotes[..n]
                .iter()
                .map(|(b, k)| LegMarket {
                    info: None,
                    bid: Some(d(b)),
                    ask: Some(d(k)),
                    quoted_at: "2026-10-02T14:59:30Z".parse().ok(),
                })
                .collect(),
            session: Session::Regular,
            now: "2026-10-02T15:00:00Z".parse().unwrap(),
            today_value: Decimal::ZERO,
            day_trade: false,
        }
    }

    fn position(symbol: &str, qty: &str) -> Position {
        Position {
            symbol: symbol.into(),
            class: AssetClass::Option,
            exchange: String::new(),
            qty: d(qty),
            qty_available: Some(d(qty)),
            avg_entry_price: d("1"),
            cost_basis: d(qty),
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

    use OrderSide::{Buy, Sell};

    #[test]
    fn a_spread_is_as_fresh_as_its_oldest_leg() {
        let a = account();
        let req = spread(&[(C45, Buy, 1), (C47, Sell, 1)], "2", Some("0.85"));
        let lim = Limits::default();
        let fresh = cx(&a, &[], 2);
        assert!(
            review_spread(&req, &fresh, &lim)
                .review
                .has("price_age", Level::Pass)
        );
        let mut stale = fresh.clone();
        stale.legs[1].quoted_at = "2026-10-02T14:50:00Z".parse().ok();
        let r = review_spread(&req, &stale, &lim).review;
        assert!(r.has("price_age", Level::Warn), "{:#?}", r.checks);
        let mut unknown = fresh.clone();
        unknown.legs[0].quoted_at = None;
        assert!(
            review_spread(&req, &unknown, &lim)
                .review
                .has("price_age", Level::Warn)
        );
    }

    #[test]
    fn a_debit_spread_ties_up_its_premium() {
        let a = account();
        let req = spread(&[(C45, Buy, 1), (C47, Sell, 1)], "2", Some("0.85"));
        let r = review_spread(&req, &cx(&a, &[], 2), &Limits::default());
        assert!(!r.review.blocked(), "{:#?}", r.review.checks);
        assert_eq!(r.net.natural, Some(d("0.88")));
        assert_eq!(r.review.value, Some(d("170.00")), "0.85 × 100 × 2");
        assert_eq!(r.requirement, Some(Decimal::ZERO));
        assert_eq!(
            r.intents,
            [PositionIntent::BuyToOpen, PositionIntent::SellToOpen]
        );
        assert!(r.review.has("collar", Level::Pass) && r.review.has("covered", Level::Pass));
        let p = r.payoff.unwrap();
        assert!((p.max_profit.unwrap() - 1.15).abs() < 1e-9);
        // Paying more than the natural price wants a second look, then a block.
        let over = spread(&[(C45, Buy, 1), (C47, Sell, 1)], "2", Some("0.90"));
        assert!(
            review_spread(&over, &cx(&a, &[], 2), &Limits::default())
                .review
                .has("collar", Level::Warn)
        );
        let way_over = spread(&[(C45, Buy, 1), (C47, Sell, 1)], "2", Some("1.10"));
        assert!(
            review_spread(&way_over, &cx(&a, &[], 2), &Limits::default())
                .review
                .has("collar", Level::Block)
        );
        // Level 2 cannot open spreads.
        let l2 = Account {
            options_trading_level: Some(2),
            ..account()
        };
        assert!(
            review_spread(&req, &cx(&l2, &[], 2), &Limits::default())
                .review
                .has("options_level", Level::Block)
        );
    }

    #[test]
    fn a_credit_spread_ties_up_its_margin_less_the_credit() {
        let a = account();
        // Sell the 45, buy the 47, for a 0.70 credit: margin 200, less 70.
        let req = spread(&[(C45, Sell, 1), (C47, Buy, 1)], "1", Some("-0.70"));
        let r = review_spread(&req, &cx(&a, &[], 2), &Limits::default());
        assert!(!r.review.blocked(), "{:#?}", r.review.checks);
        assert_eq!(r.requirement, Some(d("200.00")));
        assert_eq!(r.review.value, Some(d("130.00")));
        assert_eq!(r.review.buying_power_after, Some(d("49870.00")));
        // Natural: sell 45 at the bid 1.55, buy 47 at the ask 0.84: a 0.71 credit.
        assert_eq!(r.net.natural, Some(d("-0.71")));
        // A sold call on its own is uncovered.
        let naked = spread(
            &[(C45, Sell, 1), ("XLU261218P00044000", Sell, 1)],
            "1",
            Some("-2"),
        );
        assert!(
            review_spread(&naked, &cx(&a, &[], 2), &Limits::default())
                .review
                .has("covered", Level::Block)
        );
    }

    #[test]
    fn closing_legs_need_the_position() {
        let a = account();
        // Roll the held 45 call up to the 47: sell to close, buy to open.
        let held = [position(C45, "2")];
        let roll = spread(&[(C45, Sell, 1), (C47, Buy, 1)], "2", Some("-0.70"));
        let r = review_spread(&roll, &cx(&a, &held, 2), &Limits::default());
        assert_eq!(
            r.intents,
            [PositionIntent::SellToClose, PositionIntent::BuyToOpen]
        );
        assert!(!r.review.blocked(), "{:#?}", r.review.checks);
        // Closing three when two are held flips the position.
        let three = spread(&[(C45, Sell, 1), (C47, Buy, 1)], "3", Some("-0.70"));
        assert!(
            review_spread(&three, &cx(&a, &held, 2), &Limits::default())
                .review
                .has("available", Level::Block)
        );
        // The order must be valid: a stop is not a spread order type.
        let mut stop = roll.clone();
        stop.order_type = OrderType::Stop;
        assert!(
            review_spread(&stop, &cx(&a, &held, 2), &Limits::default())
                .review
                .has("valid", Level::Block)
        );
        // Without quotes it cannot be valued.
        let mut blind = cx(&a, &held, 2);
        blind.legs[1].ask = None;
        let mut market = roll.clone();
        market.order_type = OrderType::Market;
        market.limit_price = None;
        assert!(
            review_spread(&market, &blind, &Limits::default())
                .review
                .has("price", Level::Block)
        );
    }

    #[test]
    fn size_caps_and_sessions() {
        let a = account();
        let lim = Limits {
            max_contracts: 10,
            ..Limits::default()
        };
        let fly = spread(
            &[
                ("XLU261218C00043000", Buy, 1),
                (C45, Sell, 2),
                (C47, Buy, 1),
            ],
            "6",
            Some("0.20"),
        );
        let r = review_spread(&fly, &cx(&a, &[], 3), &lim);
        assert!(
            r.review.has("max_contracts", Level::Block),
            "12 in the middle leg"
        );
        let pre = SpreadContext {
            session: Session::PreMarket,
            ..cx(&a, &[], 2)
        };
        let req = spread(&[(C45, Buy, 1), (C47, Sell, 1)], "1", Some("0.85"));
        assert!(
            review_spread(&req, &pre, &Limits::default())
                .review
                .has("session", Level::Warn)
        );
        let restricted = Limits {
            restricted: vec!["XLU".into()],
            ..Limits::default()
        };
        assert!(
            review_spread(&req, &cx(&a, &[], 2), &restricted)
                .review
                .has("restricted", Level::Block)
        );
        // The positions and orders must have loaded: the caps count them.
        let waiting = SpreadContext {
            loaded: Loaded {
                positions: false,
                orders: true,
            },
            ..cx(&a, &[], 2)
        };
        assert!(
            review_spread(&req, &waiting, &Limits::default())
                .review
                .has("loaded", Level::Block)
        );
    }
}
