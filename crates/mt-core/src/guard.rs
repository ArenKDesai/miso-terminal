//! Guardrails: the checks an order ticket passes before its Confirm is
//! enabled. [`review`] runs every rule against a request, the account, the
//! position, the latest prices and the user's [`Limits`], and says for each
//! whether it passes, needs the user's acknowledgement (a warning) or stops
//! the order (a block). Pure, so every rule is tested here.
//!
//! The rules: trading switched on (the kill switch turns it off); the
//! restricted list; what the broker would refuse anyway (prices off the
//! tick, missing prices); the account's standing; the asset is tradable (and
//! shortable, for a short); no market orders outside the regular session;
//! per-order, daily and per-position caps in dollars; a collar on limit and
//! stop prices around the last trade; a fat-finger check on size; buying
//! power; and the pattern-day-trader count.

use serde::{Deserialize, Serialize};

use crate::account::{Account, OrderSide, PDT_DAY_TRADES};
use crate::equity::Asset;
use crate::exchange::Session;
use crate::money::{Decimal, fmt_qty, fmt_usd};
use crate::order::{OrderRequest, OrderType, price_text};

/// The user's limits (`[trading]` in config.toml, edited in SET). A cap of
/// zero is off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Whether tickets may send orders at all. The kill switch turns this
    /// off, and it stays off (across restarts) until turned back on.
    pub enabled: bool,
    /// The most one order may be worth, in dollars.
    pub max_order_value: u64,
    /// The most the day's orders may be worth together (what filled plus
    /// what is still open), in dollars.
    pub max_daily_value: u64,
    /// The largest position in one security an order may build, in dollars
    /// (long or short). Orders that reduce a position always pass.
    pub max_position_value: u64,
    /// Limit and stop prices must be within this percentage of the last trade.
    pub collar_pct: f64,
    /// Orders worth more than this percentage of equity need a second look
    /// (the ticket asks the user to tick that they mean it).
    pub fat_finger_pct: f64,
    /// The most shares one order may be for.
    pub max_shares: u64,
    /// Tickers that cannot be traded (`XEL` or `XEL US`), for an employer's
    /// personal-trading policy or anything else.
    pub restricted: Vec<String>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            enabled: true,
            max_order_value: 10_000,
            max_daily_value: 50_000,
            max_position_value: 25_000,
            collar_pct: 5.0,
            fat_finger_pct: 10.0,
            max_shares: 5_000,
            restricted: Vec::new(),
        }
    }
}

impl Limits {
    /// Whether `symbol` (a ticker) is on the restricted list.
    pub fn is_restricted(&self, symbol: &str) -> bool {
        self.restricted.iter().any(|r| {
            let r = r.trim();
            let ticker = r
                .split_whitespace()
                .next()
                .unwrap_or(r)
                .trim_end_matches(',');
            ticker.eq_ignore_ascii_case(symbol.trim())
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Pass,
    /// The user must acknowledge it before confirming.
    Warn,
    /// The order cannot be sent.
    Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub level: Level,
    /// Which rule, for tests and the audit log: `collar`, `daily_cap`…
    pub rule: &'static str,
    pub message: String,
}

/// What the ticket knows besides the request.
#[derive(Clone, Debug)]
pub struct Context<'a> {
    pub account: Option<&'a Account>,
    /// Shares held now (negative when short).
    pub position: Decimal,
    /// The asset record, when the asset list has loaded.
    pub asset: Option<&'a Asset>,
    pub last: Option<Decimal>,
    pub bid: Option<Decimal>,
    pub ask: Option<Decimal>,
    /// The exchange's session now.
    pub session: Session,
    /// What today's orders are worth already ([`crate::order::day_value`]).
    pub today_value: Decimal,
    /// Whether this order would be a day trade ([`crate::order::is_day_trade`]).
    pub day_trade: bool,
}

/// Every rule's verdict, and the estimate the ticket shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub checks: Vec<Check>,
    /// The price the order is valued at, and its value.
    pub price: Option<Decimal>,
    pub value: Option<Decimal>,
    pub position_after: Decimal,
    /// Shares that open or add to a position (the rest reduce one).
    pub opening: Decimal,
    pub buying_power_after: Option<Decimal>,
}

impl Review {
    pub fn blocked(&self) -> bool {
        self.checks.iter().any(|c| c.level == Level::Block)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| c.level == Level::Warn)
    }

    /// Nothing blocks, and any warnings have been acknowledged.
    pub fn can_confirm(&self, acknowledged: bool) -> bool {
        !self.blocked() && (acknowledged || self.warnings().next().is_none())
    }

    pub fn has(&self, rule: &str, level: Level) -> bool {
        self.checks
            .iter()
            .any(|c| c.rule == rule && c.level == level)
    }
}

fn positive(v: Option<Decimal>) -> Option<Decimal> {
    v.filter(|p| *p > Decimal::ZERO)
}

fn dollars(v: u64) -> Decimal {
    Decimal::from(v)
}

fn pct(v: f64) -> Option<Decimal> {
    crate::account::from_f64(v, 4).filter(|p| *p > Decimal::ZERO)
}

/// `2.4%` (one decimal, or none for whole numbers).
fn pct_text(v: Decimal) -> String {
    let r = v.round_dp(1).normalize();
    format!("{r}%")
}

/// Run every rule.
pub fn review(req: &OrderRequest, cx: &Context<'_>, limits: &Limits) -> Review {
    let mut checks = Vec::new();
    let mut add = |level: Level, rule: &'static str, message: String| {
        checks.push(Check {
            level,
            rule,
            message,
        });
    };
    let symbol = req.symbol.trim().to_ascii_uppercase();

    // Switches and lists.
    if !limits.enabled {
        add(
            Level::Block,
            "enabled",
            "Trading is off (the kill switch was used). Turn it back on in ORD.".into(),
        );
    }
    if limits.is_restricted(&symbol) {
        add(
            Level::Block,
            "restricted",
            format!("{symbol} is on your restricted list (SET)."),
        );
    }
    for p in req.problems() {
        add(Level::Block, "valid", p);
    }

    // The account and the asset.
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
    if let Some(asset) = cx.asset {
        if !asset.tradable {
            add(
                Level::Block,
                "asset",
                format!("Alpaca does not trade {symbol}."),
            );
        }
        if req.qty.fract() != Decimal::ZERO && !asset.fractionable {
            add(
                Level::Block,
                "asset",
                format!("{symbol} does not trade in fractions of a share."),
            );
        }
    }

    // Position after the order, and how much of it opens or adds.
    let before = cx.position;
    let after = before + req.side.sign() * req.qty;
    let crosses = !before.is_zero() && before.is_sign_negative() != after.is_sign_negative();
    let opening = if crosses {
        after.abs()
    } else {
        (after.abs() - before.abs()).max(Decimal::ZERO)
    };
    if after < Decimal::ZERO && opening > Decimal::ZERO {
        let shortable =
            cx.account.is_none_or(|a| a.shorting_enabled) && cx.asset.is_none_or(|a| a.shortable);
        if !shortable {
            add(
                Level::Block,
                "short",
                format!(
                    "This would sell {symbol} short, which the account or asset does not allow."
                ),
            );
        } else if req.qty.fract() != Decimal::ZERO {
            add(
                Level::Block,
                "short",
                "Fractional shares cannot be sold short.".into(),
            );
        } else {
            add(
                Level::Warn,
                "short",
                format!(
                    "This opens a short position of {} shares.",
                    fmt_qty(opening)
                ),
            );
        }
    }

    // Sessions.
    let market_price = match req.side {
        OrderSide::Buy => positive(cx.ask),
        OrderSide::Sell => positive(cx.bid),
    }
    .or(positive(cx.last));
    match (req.order_type, cx.session) {
        (OrderType::Market, s) if s != Session::Regular && !req.tif.is_auction() => add(
            Level::Block,
            "session",
            "Market orders only in the regular session (09:30 to 16:00 New York). Use a limit order."
                .into(),
        ),
        (_, Session::Regular) => {}
        (_, Session::PreMarket | Session::AfterHours) if req.extended_hours => {}
        (_, Session::PreMarket | Session::AfterHours) => add(
            Level::Warn,
            "session",
            "The regular session is closed: this waits for the next one (tick Extended hours to trade now)."
                .into(),
        ),
        (_, Session::Closed) => add(
            Level::Warn,
            "session",
            "The market is closed: this order waits for the next session.".into(),
        ),
    }

    // Value and caps.
    let price = req.reference_price(market_price);
    let value = price.map(|p| (p * req.qty).round_dp(2));
    match value {
        None => add(
            Level::Block,
            "price",
            format!("No price for {symbol} yet, so the order cannot be valued."),
        ),
        Some(v) => {
            let cap = dollars(limits.max_order_value);
            if limits.max_order_value > 0 && v > cap {
                add(
                    Level::Block,
                    "order_cap",
                    format!(
                        "Worth {}: more than the per-order cap of {}.",
                        fmt_usd(v, 2),
                        fmt_usd(cap, 0)
                    ),
                );
            } else if limits.max_order_value > 0 {
                add(
                    Level::Pass,
                    "order_cap",
                    format!(
                        "Worth {}, within the per-order cap of {}.",
                        fmt_usd(v, 2),
                        fmt_usd(cap, 0)
                    ),
                );
            }
            let cap = dollars(limits.max_daily_value);
            let total = cx.today_value + v;
            if limits.max_daily_value > 0 {
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
        let worth = (after.abs() * p).round_dp(2);
        let cap = dollars(limits.max_position_value);
        add(
            if worth > cap {
                Level::Block
            } else {
                Level::Pass
            },
            "position_cap",
            format!(
                "The position would be {} shares, worth {} (the cap is {}).",
                fmt_qty(after),
                fmt_usd(worth, 2),
                fmt_usd(cap, 0)
            ),
        );
    }

    // The collar: limit and stop prices near the last trade.
    let collar = pct(limits.collar_pct);
    for (name, p) in [("limit", req.limit_price), ("stop", req.stop_price)] {
        let Some(p) = p.filter(|_| match name {
            "limit" => req.order_type.needs_limit(),
            _ => req.order_type.needs_stop(),
        }) else {
            continue;
        };
        match (positive(cx.last), collar) {
            (_, None) => {}
            (None, Some(_)) => add(
                Level::Warn,
                "collar",
                format!("No last price to check the {name} price against."),
            ),
            (Some(last), Some(c)) => {
                let off = ((p - last) / last * Decimal::ONE_HUNDRED).round_dp(2);
                let side = if off >= Decimal::ZERO {
                    "above"
                } else {
                    "below"
                };
                add(
                    if off.abs() > c {
                        Level::Block
                    } else {
                        Level::Pass
                    },
                    "collar",
                    format!(
                        "The {name} {} is {} {side} the last price {} (the collar is {}).",
                        price_text(p),
                        pct_text(off.abs()),
                        price_text(last),
                        pct_text(c)
                    ),
                );
            }
        }
    }

    // Size.
    if limits.max_shares > 0 && req.qty > dollars(limits.max_shares) {
        add(
            Level::Block,
            "max_shares",
            format!(
                "{} shares is more than the {} a single order may be for.",
                fmt_qty(req.qty),
                fmt_qty(dollars(limits.max_shares))
            ),
        );
    }
    if let (Some(v), Some(a), Some(limit)) = (value, cx.account, pct(limits.fat_finger_pct))
        && a.equity > Decimal::ZERO
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

    // Buying power for what the order opens.
    let mut buying_power_after = None;
    if let (Some(a), Some(p)) = (cx.account, price) {
        let needed = (opening * p).round_dp(2);
        buying_power_after = Some(a.buying_power - needed);
        if needed > a.buying_power {
            add(
                Level::Block,
                "buying_power",
                format!(
                    "Needs {} of buying power; the account has {}.",
                    fmt_usd(needed, 2),
                    fmt_usd(a.buying_power, 2)
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::TimeInForce;
    use crate::order::tests::{d, request};

    fn account() -> Account {
        Account {
            status: "ACTIVE".into(),
            equity: d("100000"),
            last_equity: d("99000"),
            buying_power: d("150000"),
            shorting_enabled: true,
            ..Account::default()
        }
    }

    fn asset() -> Asset {
        Asset {
            symbol: "XLU".into(),
            tradable: true,
            shortable: true,
            fractionable: true,
            ..Asset::default()
        }
    }

    fn cx<'a>(a: &'a Account, asset: &'a Asset) -> Context<'a> {
        Context {
            account: Some(a),
            position: Decimal::ZERO,
            asset: Some(asset),
            last: Some(d("82.41")),
            bid: Some(d("82.40")),
            ask: Some(d("82.43")),
            session: Session::Regular,
            today_value: Decimal::ZERO,
            day_trade: false,
        }
    }

    #[test]
    fn a_plain_order_passes() {
        let (a, s) = (account(), asset());
        let r = review(
            &request("10", OrderType::Limit, Some("82.50")),
            &cx(&a, &s),
            &Limits::default(),
        );
        assert!(!r.blocked(), "{:#?}", r.checks);
        assert!(r.can_confirm(false));
        assert_eq!(r.value, Some(d("825.00")));
        assert_eq!(r.position_after, d("10"));
        assert_eq!(r.buying_power_after, Some(d("149175.00")));
        assert!(r.has("collar", Level::Pass) && r.has("order_cap", Level::Pass));
        // A market buy is valued at the ask.
        let m = review(
            &request("10", OrderType::Market, None),
            &cx(&a, &s),
            &Limits::default(),
        );
        assert_eq!(m.price, Some(d("82.43")));
        assert!(m.can_confirm(false), "{:#?}", m.checks);
    }

    #[test]
    fn switches_lists_and_the_account() {
        let (a, s) = (account(), asset());
        let req = request("10", OrderType::Limit, Some("82.50"));
        let off = Limits {
            enabled: false,
            ..Limits::default()
        };
        assert!(review(&req, &cx(&a, &s), &off).has("enabled", Level::Block));
        let restricted = Limits {
            restricted: vec!["MGEE".into(), "xlu US".into()],
            ..Limits::default()
        };
        assert!(review(&req, &cx(&a, &s), &restricted).has("restricted", Level::Block));
        assert!(!Limits::default().is_restricted("XLU"));
        let blocked = Account {
            trading_blocked: true,
            ..account()
        };
        assert!(review(&req, &cx(&blocked, &s), &Limits::default()).has("account", Level::Block));
        let none = Context {
            account: None,
            ..cx(&a, &s)
        };
        assert!(review(&req, &none, &Limits::default()).has("account", Level::Block));
        let halted = Asset {
            tradable: false,
            ..asset()
        };
        assert!(review(&req, &cx(&a, &halted), &Limits::default()).has("asset", Level::Block));
        let bad = request("10", OrderType::Limit, Some("82.505"));
        assert!(review(&bad, &cx(&a, &s), &Limits::default()).has("valid", Level::Block));
    }

    #[test]
    fn market_orders_only_in_the_regular_session() {
        let (a, s) = (account(), asset());
        let pre = Context {
            session: Session::PreMarket,
            ..cx(&a, &s)
        };
        let lim = Limits::default();
        assert!(
            review(&request("10", OrderType::Market, None), &pre, &lim)
                .has("session", Level::Block)
        );
        // A limit order waits for the open (a warning), unless extended hours.
        let r = review(&request("10", OrderType::Limit, Some("82.50")), &pre, &lim);
        assert!(r.has("session", Level::Warn) && !r.blocked());
        assert!(!r.can_confirm(false) && r.can_confirm(true));
        let mut ext = request("10", OrderType::Limit, Some("82.50"));
        ext.extended_hours = true;
        assert!(review(&ext, &pre, &lim).can_confirm(false));
        // At-the-open market orders are for exactly this.
        let mut opg = request("10", OrderType::Market, None);
        opg.tif = TimeInForce::Opg;
        assert!(!review(&opg, &pre, &lim).has("session", Level::Block));
        let closed = Context {
            session: Session::Closed,
            ..cx(&a, &s)
        };
        assert!(review(&ext, &closed, &lim).has("session", Level::Warn));
    }

    #[test]
    fn caps_on_orders_days_and_positions() {
        let (a, s) = (account(), asset());
        let lim = Limits::default();
        // $10,000 per order: 122 shares at 82.50 is $10,065.
        let big = request("122", OrderType::Limit, Some("82.50"));
        assert!(review(&big, &cx(&a, &s), &lim).has("order_cap", Level::Block));
        assert!(
            !review(
                &request("121", OrderType::Limit, Some("82.50")),
                &cx(&a, &s),
                &lim
            )
            .blocked()
        );
        // $50,000 a day.
        let busy = Context {
            today_value: d("49500"),
            ..cx(&a, &s)
        };
        assert!(
            review(&request("10", OrderType::Limit, Some("82.50")), &busy, &lim)
                .has("daily_cap", Level::Block)
        );
        // $25,000 a position: 300 held (24,723) plus 10 more is over.
        let held = Context {
            position: d("300"),
            ..cx(&a, &s)
        };
        let add = request("10", OrderType::Limit, Some("82.50"));
        assert!(review(&add, &held, &lim).has("position_cap", Level::Block));
        // Selling from it is always allowed.
        let mut sell = add.clone();
        sell.side = OrderSide::Sell;
        let r = review(&sell, &held, &lim);
        assert!(!r.blocked(), "{:#?}", r.checks);
        assert_eq!(r.opening, Decimal::ZERO);
        // A cap of zero is off.
        let off = Limits {
            max_order_value: 0,
            max_daily_value: 0,
            max_position_value: 0,
            fat_finger_pct: 0.0,
            ..Limits::default()
        };
        let r = review(&big, &cx(&a, &s), &off);
        assert!(!r.blocked(), "{:#?}", r.checks);
        assert!(!r.checks.iter().any(|c| c.rule.ends_with("_cap")));
    }

    #[test]
    fn the_collar_and_fat_fingers() {
        let (a, s) = (account(), asset());
        let lim = Limits::default();
        // 5% around 82.41: 86.53 is 5.0% above, 86.60 is 5.09%.
        assert!(
            !review(
                &request("10", OrderType::Limit, Some("86.53")),
                &cx(&a, &s),
                &lim
            )
            .blocked()
        );
        let r = review(
            &request("10", OrderType::Limit, Some("86.60")),
            &cx(&a, &s),
            &lim,
        );
        assert!(r.has("collar", Level::Block), "{:#?}", r.checks);
        assert!(r.checks.iter().any(|c| c.message.contains("5.1% above")));
        let mut stop = request("10", OrderType::Stop, None);
        stop.stop_price = Some(d("70"));
        assert!(review(&stop, &cx(&a, &s), &lim).has("collar", Level::Block));
        let no_last = Context {
            last: None,
            ..cx(&a, &s)
        };
        assert!(
            review(
                &request("10", OrderType::Limit, Some("82.50")),
                &no_last,
                &lim
            )
            .has("collar", Level::Warn)
        );
        // Without any price a market order cannot be valued.
        let blind = Context {
            last: None,
            bid: None,
            ask: None,
            ..cx(&a, &s)
        };
        assert!(
            review(&request("10", OrderType::Market, None), &blind, &lim)
                .has("price", Level::Block)
        );
        // Over 10% of a $20,000 account wants a second look; too many shares are refused.
        let small = Account {
            equity: d("20000"),
            ..account()
        };
        let r = review(
            &request("30", OrderType::Limit, Some("82.50")),
            &cx(&small, &s),
            &lim,
        );
        assert!(r.has("fat_finger", Level::Warn) && !r.blocked());
        assert!(
            r.checks
                .iter()
                .any(|c| c.message.contains("12.4% of equity")),
            "{:#?}",
            r.checks
        );
        let penny = request("6000", OrderType::Limit, Some("0.50"));
        assert!(review(&penny, &cx(&a, &s), &lim).has("max_shares", Level::Block));
    }

    #[test]
    fn shorts_buying_power_and_day_trades() {
        let (a, s) = (account(), asset());
        let lim = Limits {
            max_position_value: 0,
            max_order_value: 0,
            max_daily_value: 0,
            ..Limits::default()
        };
        // Selling 30 with 20 held: 20 close, 10 open a short.
        let held = Context {
            position: d("20"),
            ..cx(&a, &s)
        };
        let mut sell = request("30", OrderType::Limit, Some("82.40"));
        sell.side = OrderSide::Sell;
        let r = review(&sell, &held, &lim);
        assert_eq!((r.position_after, r.opening), (d("-10"), d("10")));
        assert!(r.has("short", Level::Warn));
        let hard = Asset {
            shortable: false,
            ..asset()
        };
        let held_hard = Context {
            asset: Some(&hard),
            ..held.clone()
        };
        assert!(review(&sell, &held_hard, &lim).has("short", Level::Block));
        // Buying power covers what opens.
        let poor = Account {
            buying_power: d("500"),
            ..account()
        };
        assert!(
            review(
                &request("10", OrderType::Limit, Some("82.50")),
                &cx(&poor, &s),
                &lim
            )
            .has("buying_power", Level::Block)
        );
        // Covering needs none.
        let short = Context {
            position: d("-10"),
            ..cx(&poor, &s)
        };
        assert!(
            !review(
                &request("10", OrderType::Limit, Some("82.50")),
                &short,
                &lim
            )
            .blocked()
        );
        // Day trades under $25,000.
        let pdt = Account {
            equity: d("20000"),
            daytrade_count: 2,
            ..account()
        };
        let dt = Context {
            day_trade: true,
            ..cx(&pdt, &s)
        };
        let r = review(&request("1", OrderType::Limit, Some("82.50")), &dt, &lim);
        assert!(r.has("day_trade", Level::Warn), "{:#?}", r.checks);
        let used = Account {
            daytrade_count: 3,
            ..pdt.clone()
        };
        let dt_used = Context {
            account: Some(&used),
            ..dt.clone()
        };
        assert!(
            review(
                &request("1", OrderType::Limit, Some("82.50")),
                &dt_used,
                &lim
            )
            .has("day_trade", Level::Block)
        );
        let rich = Context {
            account: Some(&a),
            ..dt
        };
        assert!(
            !review(&request("1", OrderType::Limit, Some("82.50")), &rich, &lim)
                .checks
                .iter()
                .any(|c| c.rule == "day_trade")
        );
    }

    #[test]
    fn limits_read_from_toml_with_defaults() {
        let l: Limits =
            serde_json::from_str(r#"{"max_order_value": 2500, "restricted": ["MGEE"]}"#).unwrap();
        assert_eq!(l.max_order_value, 2500);
        assert_eq!(l.max_daily_value, Limits::default().max_daily_value);
        assert!(l.enabled && l.is_restricted("mgee"));
    }
}
