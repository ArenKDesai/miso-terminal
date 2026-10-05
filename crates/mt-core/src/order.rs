//! Orders: what a ticket asks the broker for ([`OrderRequest`]) and what the
//! broker says about it ([`Order`], [`OrderStatus`]). The checks a ticket
//! passes before its Confirm is enabled are in [`crate::guard`].
//!
//! Quantities and prices are exact ([`Decimal`]). An order is for a stock or
//! ETF (`symbol` a ticker, quantities in shares) or for an option contract
//! (`symbol` an OCC symbol, quantities in contracts of 100 shares, premiums
//! per share).

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};

use crate::account::{AssetClass, OrderSide};
use crate::instrument::OptionContract;
use crate::money::Decimal;
use crate::options::{MULTIPLIER, PositionIntent, contract_words};

/// Prices at or above a dollar trade in cents; below, in hundredths of a cent.
pub const PENNY: Decimal = Decimal::from_parts(1, 0, 0, false, 2);
pub const SUB_PENNY: Decimal = Decimal::from_parts(1, 0, 0, false, 4);

/// The smallest price step for a price (Rule 612: sub-penny only below $1).
pub fn tick_for(price: Decimal) -> Decimal {
    if price >= Decimal::ONE {
        PENNY
    } else {
        SUB_PENNY
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
    /// Shown when the broker reports one; tickets do not place them.
    TrailingStop,
}

impl OrderType {
    /// What tickets offer.
    pub const TICKET: [Self; 4] = [Self::Market, Self::Limit, Self::Stop, Self::StopLimit];

    /// Alpaca's name: `market`, `limit`, `stop`, `stop_limit`, `trailing_stop`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::Stop => "stop",
            Self::StopLimit => "stop_limit",
            Self::TrailingStop => "trailing_stop",
        }
    }

    /// As the command line writes it: `MKT`, `LMT`, `STP`, `STPLMT`, `TRAIL`.
    pub fn code(self) -> &'static str {
        match self {
            Self::Market => "MKT",
            Self::Limit => "LMT",
            Self::Stop => "STP",
            Self::StopLimit => "STPLMT",
            Self::TrailingStop => "TRAIL",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Market => "Market",
            Self::Limit => "Limit",
            Self::Stop => "Stop",
            Self::StopLimit => "Stop limit",
            Self::TrailingStop => "Trailing stop",
        }
    }

    /// The broker's name or a command-line code.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "MARKET" | "MKT" => Some(Self::Market),
            "LIMIT" | "LMT" => Some(Self::Limit),
            "STOP" | "STP" => Some(Self::Stop),
            "STOP_LIMIT" | "STPLMT" | "STOPLIMIT" | "STP_LMT" => Some(Self::StopLimit),
            "TRAILING_STOP" | "TRAIL" => Some(Self::TrailingStop),
            _ => None,
        }
    }

    pub fn needs_limit(self) -> bool {
        matches!(self, Self::Limit | Self::StopLimit)
    }

    pub fn needs_stop(self) -> bool {
        matches!(self, Self::Stop | Self::StopLimit)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeInForce {
    /// Until the end of the day's regular session (or extended session, with
    /// `extended_hours`).
    #[default]
    Day,
    /// Until filled or cancelled (Alpaca cancels after 90 days).
    Gtc,
    /// The opening auction only.
    Opg,
    /// The closing auction only.
    Cls,
    /// Fill what can be filled at once, cancel the rest.
    Ioc,
    /// Fill all at once or nothing.
    Fok,
}

impl TimeInForce {
    pub const ALL: [Self; 6] = [
        Self::Day,
        Self::Gtc,
        Self::Opg,
        Self::Cls,
        Self::Ioc,
        Self::Fok,
    ];

    /// Alpaca's name: `day`, `gtc`…
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Gtc => "gtc",
            Self::Opg => "opg",
            Self::Cls => "cls",
            Self::Ioc => "ioc",
            Self::Fok => "fok",
        }
    }

    /// `DAY`, `GTC`…
    pub fn code(self) -> &'static str {
        match self {
            Self::Day => "DAY",
            Self::Gtc => "GTC",
            Self::Opg => "OPG",
            Self::Cls => "CLS",
            Self::Ioc => "IOC",
            Self::Fok => "FOK",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "Day",
            Self::Gtc => "Good till cancelled",
            Self::Opg => "At the open",
            Self::Cls => "At the close",
            Self::Ioc => "Immediate or cancel",
            Self::Fok => "Fill or kill",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// The auctions: market and limit orders that wait for the open or close.
    pub fn is_auction(self) -> bool {
        matches!(self, Self::Opg | Self::Cls)
    }
}

/// Where an order stands, in Alpaca's terms.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    DoneForDay,
    Canceled,
    Expired,
    Replaced,
    PendingCancel,
    PendingReplace,
    PendingNew,
    Accepted,
    AcceptedForBidding,
    Stopped,
    Rejected,
    Suspended,
    Calculated,
    Held,
    Other(String),
}

impl OrderStatus {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "new" => Self::New,
            "partially_filled" => Self::PartiallyFilled,
            "filled" => Self::Filled,
            "done_for_day" => Self::DoneForDay,
            "canceled" | "cancelled" => Self::Canceled,
            "expired" => Self::Expired,
            "replaced" => Self::Replaced,
            "pending_cancel" => Self::PendingCancel,
            "pending_replace" => Self::PendingReplace,
            "pending_new" => Self::PendingNew,
            "accepted" => Self::Accepted,
            "accepted_for_bidding" => Self::AcceptedForBidding,
            "stopped" => Self::Stopped,
            "rejected" => Self::Rejected,
            "suspended" => Self::Suspended,
            "calculated" => Self::Calculated,
            "held" => Self::Held,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Finished: nothing more will fill.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Canceled | Self::Expired | Self::Replaced | Self::Rejected
        )
    }

    /// Still working, or may still fill.
    pub fn is_open(&self) -> bool {
        !self.is_final()
    }

    /// Whether a cancel or replace can still be asked for.
    pub fn can_cancel(&self) -> bool {
        self.is_open() && !matches!(self, Self::PendingCancel | Self::Calculated)
    }

    pub fn can_replace(&self) -> bool {
        self.can_cancel() && !matches!(self, Self::PendingReplace | Self::PendingNew)
    }

    /// `Partially filled`, `Done for day`, `Cancelled`.
    pub fn label(&self) -> String {
        let raw = match self {
            Self::Other(s) => s.clone(),
            // Alpaca spells it the American way; the terminal does not.
            Self::Canceled => "Cancelled".to_owned(),
            Self::PendingCancel => "PendingCancellation".to_owned(),
            s => format!("{s:?}"),
        };
        // CamelCase or snake_case to words.
        let mut out = String::new();
        for (i, c) in raw.chars().enumerate() {
            if c == '_' {
                out.push(' ');
            } else if c.is_ascii_uppercase() && i > 0 {
                out.push(' ');
                out.push(c.to_ascii_lowercase());
            } else if i == 0 {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push(c);
            }
        }
        out
    }
}

/// What a ticket sends: one stock, ETF or option order.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderRequest {
    /// The terminal's own id for the order, made before it is sent: if the
    /// answer is lost, the order is looked up by it before anything is
    /// resent, and the broker refuses a second order with the same id.
    pub client_order_id: String,
    /// The ticker (`XLU`), or an option's OCC symbol.
    pub symbol: String,
    pub side: OrderSide,
    /// Shares (fractions only for day orders), or whole contracts.
    pub qty: Decimal,
    pub order_type: OrderType,
    pub limit_price: Option<Decimal>,
    pub stop_price: Option<Decimal>,
    pub tif: TimeInForce,
    /// Allow it to fill before the open and after the close (day limit orders only).
    pub extended_hours: bool,
    /// For an option: whether it opens or closes a position. Sent when set,
    /// so the broker need not work it out.
    pub position_intent: Option<PositionIntent>,
}

impl OrderRequest {
    /// The option contract, when the order is for one.
    pub fn contract(&self) -> Option<OptionContract> {
        OptionContract::parse_occ(&self.symbol)
    }

    pub fn is_option(&self) -> bool {
        self.contract().is_some()
    }

    /// What a unit's price is multiplied by: 100 for an option contract.
    pub fn multiplier(&self) -> Decimal {
        if self.is_option() {
            MULTIPLIER
        } else {
            Decimal::ONE
        }
    }

    /// `share`, `shares`, `contract` or `contracts`, for `n` of them.
    pub fn unit(&self, n: Decimal) -> &'static str {
        match (self.is_option(), n == Decimal::ONE) {
            (true, true) => "contract",
            (true, false) => "contracts",
            (false, true) => "share",
            (false, false) => "shares",
        }
    }

    /// What the broker would refuse: a missing or unneeded price, prices off
    /// the tick, a fractional order that is not a day order, extended hours
    /// on anything but a day limit order.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.symbol.trim().is_empty() {
            out.push("No security.".to_owned());
        }
        let option = self.is_option();
        if let Some(intent) = self.position_intent {
            if !option {
                out.push("Only option orders open or close a position by name.".to_owned());
            } else if intent.side() != self.side {
                out.push(format!(
                    "{} does not match a {} order.",
                    intent.label(),
                    self.side.label().to_ascii_lowercase()
                ));
            }
        }
        if self.qty <= Decimal::ZERO {
            out.push("The quantity must be more than zero.".to_owned());
        }
        if self.order_type == OrderType::TrailingStop {
            out.push("Tickets do not place trailing stops.".to_owned());
        }
        for (name, needed, price) in [
            ("limit", self.order_type.needs_limit(), self.limit_price),
            ("stop", self.order_type.needs_stop(), self.stop_price),
        ] {
            match (needed, price) {
                (true, None) => out.push(format!(
                    "A {} order needs a {name} price.",
                    self.order_type.label().to_ascii_lowercase()
                )),
                (true, Some(p)) if p <= Decimal::ZERO => {
                    out.push(format!("The {name} price must be more than zero."));
                }
                (true, Some(p)) if option && !(p % PENNY).is_zero() => {
                    out.push(format!("The {name} price {p} is finer than a cent."));
                }
                (true, Some(p)) if !(p % tick_for(p)).is_zero() => out.push(format!(
                    "The {name} price {p} is finer than the tick ({}).",
                    tick_for(p).normalize()
                )),
                _ => {}
            }
        }
        if option {
            if self.qty.fract() != Decimal::ZERO {
                out.push("Options trade in whole contracts.".to_owned());
            }
            if !matches!(self.tif, TimeInForce::Day | TimeInForce::Gtc) {
                out.push("Option orders are DAY or GTC.".to_owned());
            }
            if self.extended_hours {
                out.push("Options trade in the regular session only.".to_owned());
            }
            return out;
        }
        if self.qty.fract() != Decimal::ZERO && self.tif != TimeInForce::Day {
            out.push("Fractional shares trade as day orders only.".to_owned());
        }
        if self.extended_hours
            && (self.order_type != OrderType::Limit || self.tif != TimeInForce::Day)
        {
            out.push("Extended hours take day limit orders only.".to_owned());
        }
        if self.tif.is_auction() && !matches!(self.order_type, OrderType::Market | OrderType::Limit)
        {
            out.push(format!(
                "{} orders are market or limit orders.",
                self.tif.code()
            ));
        }
        out
    }

    /// The price the order is valued at for caps and estimates: its limit,
    /// its stop, or for a market order the price it is likely to get.
    pub fn reference_price(&self, market: Option<Decimal>) -> Option<Decimal> {
        match self.order_type {
            OrderType::Limit | OrderType::StopLimit => self.limit_price,
            OrderType::Stop => self.stop_price,
            OrderType::Market | OrderType::TrailingStop => market,
        }
    }

    /// `Buy 10 XLU · limit 82.50 · DAY`, or for an option
    /// `Sell 1 XLU Dec 18 '26 45 call · limit 1.60 · DAY · to close`.
    pub fn describe(&self) -> String {
        let what = self
            .contract()
            .map_or_else(|| self.symbol.clone(), |c| contract_words(&c));
        let mut s = format!(
            "{} {} {what}",
            self.side.label(),
            crate::money::fmt_qty(self.qty)
        );
        match self.order_type {
            OrderType::Market => s.push_str(" · market"),
            OrderType::Limit | OrderType::Stop | OrderType::TrailingStop => {
                let p = self.limit_price.or(self.stop_price);
                s.push_str(&format!(
                    " · {} {}",
                    self.order_type.label().to_ascii_lowercase(),
                    p.map(price_text).unwrap_or_default()
                ));
            }
            OrderType::StopLimit => s.push_str(&format!(
                " · stop {} limit {}",
                self.stop_price.map(price_text).unwrap_or_default(),
                self.limit_price.map(price_text).unwrap_or_default()
            )),
        }
        s.push_str(&format!(" · {}", self.tif.code()));
        if self.extended_hours {
            s.push_str(" · extended hours");
        }
        match self.position_intent {
            Some(i) if i.opens() => s.push_str(" · to open"),
            Some(_) => s.push_str(" · to close"),
            None => {}
        }
        s
    }
}

/// A price with at least two decimals: `82.50`, `0.1925`.
pub fn price_text(v: Decimal) -> String {
    let r = v.normalize();
    if r.scale() < 2 {
        format!("{r:.2}")
    } else {
        r.to_string()
    }
}

/// An order as the broker reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub class: AssetClass,
    pub side: OrderSide,
    /// `None` for a type this terminal does not know (see `type_name`).
    pub order_type: Option<OrderType>,
    pub type_name: String,
    pub tif: Option<TimeInForce>,
    pub qty: Option<Decimal>,
    /// For orders by dollar amount.
    pub notional: Option<Decimal>,
    pub filled_qty: Decimal,
    pub filled_avg_price: Option<Decimal>,
    pub limit_price: Option<Decimal>,
    pub stop_price: Option<Decimal>,
    pub status: OrderStatus,
    pub extended_hours: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub filled_at: Option<DateTime<Utc>>,
    pub canceled_at: Option<DateTime<Utc>>,
    pub expired_at: Option<DateTime<Utc>>,
    /// The order that replaced this one, and the one this replaced.
    pub replaced_by: Option<String>,
    pub replaces: Option<String>,
    /// For an option order: whether it opens or closes a position.
    pub position_intent: Option<PositionIntent>,
}

impl Order {
    /// What a unit's price is multiplied by: 100 for an option contract.
    pub fn multiplier(&self) -> Decimal {
        if self.is_option() {
            MULTIPLIER
        } else {
            Decimal::ONE
        }
    }

    /// Shares still to fill (zero once the order is finished).
    pub fn remaining(&self) -> Decimal {
        if self.status.is_final() {
            return Decimal::ZERO;
        }
        self.qty
            .map_or(Decimal::ZERO, |q| (q - self.filled_qty).max(Decimal::ZERO))
    }

    /// The newest time the broker gives.
    pub fn last_change(&self) -> Option<DateTime<Utc>> {
        [
            self.updated_at,
            self.filled_at,
            self.canceled_at,
            self.expired_at,
            self.submitted_at,
            self.created_at,
        ]
        .into_iter()
        .flatten()
        .max()
    }

    /// The order's own price: limit, else stop.
    pub fn price(&self) -> Option<Decimal> {
        self.limit_price.or(self.stop_price)
    }

    /// What it is worth: what filled, at its fill price, plus what is still
    /// open, at its limit or stop (or `market` for a market order), times
    /// the multiplier for an option.
    pub fn value(&self, market: Option<Decimal>) -> Decimal {
        let filled = self
            .filled_avg_price
            .map_or(Decimal::ZERO, |p| p * self.filled_qty);
        let open = self.remaining();
        let price = self.price().or(market).unwrap_or(Decimal::ZERO);
        (filled + open * price) * self.multiplier()
    }

    pub fn is_option(&self) -> bool {
        self.class == AssetClass::Option || OptionContract::parse_occ(&self.symbol).is_some()
    }
}

/// The REST list and the stream's newer copies, by id: the most recently
/// changed copy of each order wins. Newest first (by creation).
pub fn merge_orders<'a>(
    listed: &[Order],
    streamed: impl IntoIterator<Item = &'a Order>,
) -> Vec<Order> {
    let mut by_id: BTreeMap<&str, &Order> = listed.iter().map(|o| (o.id.as_str(), o)).collect();
    for o in streamed {
        match by_id.get(o.id.as_str()) {
            Some(old) if old.last_change() > o.last_change() => {}
            _ => {
                by_id.insert(o.id.as_str(), o);
            }
        }
    }
    let mut out: Vec<Order> = by_id.into_values().cloned().collect();
    out.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    out
}

/// New York date of an instant.
fn ny_date(t: DateTime<Utc>) -> NaiveDate {
    crate::exchange::to_exchange(t).date_naive()
}

/// The value of the orders sent on `day` (New York date): what filled, plus
/// what is still open. Cancelled remainders do not count. The daily cap is
/// measured against this; `market` prices open market orders.
pub fn day_value(
    orders: &[Order],
    day: NaiveDate,
    market: impl Fn(&str) -> Option<Decimal>,
) -> Decimal {
    orders
        .iter()
        .filter(|o| {
            o.created_at
                .or(o.submitted_at)
                .is_some_and(|t| ny_date(t) == day)
                && o.status != OrderStatus::Replaced
        })
        .map(|o| o.value(market(&o.symbol)).abs())
        .sum()
}

/// Whether an order on `side` in `symbol` today would be a day trade: an
/// order on the other side filled (at least partly) the same New York day.
pub fn is_day_trade(orders: &[Order], day: NaiveDate, symbol: &str, side: OrderSide) -> bool {
    orders.iter().any(|o| {
        o.symbol.eq_ignore_ascii_case(symbol)
            && o.side != side
            && o.filled_qty > Decimal::ZERO
            && o.filled_at
                .or(o.updated_at)
                .is_some_and(|t| ny_date(t) == day)
    })
}

impl fmt::Display for OrderRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::str::FromStr;

    pub(crate) fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    pub(crate) fn request(qty: &str, kind: OrderType, limit: Option<&str>) -> OrderRequest {
        OrderRequest {
            client_order_id: "mt-test-1".into(),
            symbol: "XLU".into(),
            side: OrderSide::Buy,
            qty: d(qty),
            order_type: kind,
            limit_price: limit.map(d),
            stop_price: None,
            tif: TimeInForce::Day,
            extended_hours: false,
            position_intent: None,
        }
    }

    pub(crate) fn order(id: &str, side: OrderSide, qty: &str, filled: &str, status: &str) -> Order {
        Order {
            id: id.into(),
            client_order_id: format!("c-{id}"),
            symbol: "XLU".into(),
            class: AssetClass::Equity,
            side,
            order_type: Some(OrderType::Limit),
            type_name: "limit".into(),
            tif: Some(TimeInForce::Day),
            qty: Some(d(qty)),
            notional: None,
            filled_qty: d(filled),
            filled_avg_price: (d(filled) > Decimal::ZERO).then(|| d("44.00")),
            limit_price: Some(d("44.10")),
            stop_price: None,
            status: OrderStatus::parse(status),
            extended_hours: false,
            created_at: Some("2026-10-02T14:00:00Z".parse().unwrap()),
            submitted_at: None,
            updated_at: Some("2026-10-02T14:05:00Z".parse().unwrap()),
            filled_at: None,
            canceled_at: None,
            expired_at: None,
            replaced_by: None,
            replaces: None,
            position_intent: None,
        }
    }

    #[test]
    fn types_and_statuses_parse() {
        assert_eq!(OrderType::parse("lmt"), Some(OrderType::Limit));
        assert_eq!(OrderType::parse("stop_limit"), Some(OrderType::StopLimit));
        assert_eq!(OrderType::parse("STPLMT"), Some(OrderType::StopLimit));
        assert_eq!(OrderType::parse("x"), None);
        assert_eq!(TimeInForce::parse("GTC"), Some(TimeInForce::Gtc));
        assert_eq!(TimeInForce::parse("week"), None);
        assert!(OrderStatus::parse("partially_filled").is_open());
        assert!(OrderStatus::parse("filled").is_final());
        assert!(!OrderStatus::parse("pending_cancel").can_cancel());
        assert_eq!(OrderStatus::parse("done_for_day").label(), "Done for day");
        assert_eq!(OrderStatus::PartiallyFilled.label(), "Partially filled");
        assert_eq!(OrderStatus::parse("canceled").label(), "Cancelled");
        assert_eq!(OrderStatus::Other("weird_one".into()).label(), "Weird one");
    }

    #[test]
    fn requests_are_checked_for_what_the_broker_refuses() {
        assert!(
            request("10", OrderType::Limit, Some("82.50"))
                .problems()
                .is_empty()
        );
        assert!(request("10", OrderType::Market, None).problems().is_empty());
        let p = request("10", OrderType::Limit, None).problems();
        assert!(p[0].contains("needs a limit price"), "{p:?}");
        let p = request("10", OrderType::Limit, Some("82.505")).problems();
        assert!(p[0].contains("finer than the tick"), "{p:?}");
        assert!(
            request("10", OrderType::Limit, Some("0.5025"))
                .problems()
                .is_empty(),
            "sub-penny below $1"
        );
        assert!(!request("0", OrderType::Market, None).problems().is_empty());
        let mut frac = request("0.5", OrderType::Market, None);
        assert!(frac.problems().is_empty());
        frac.tif = TimeInForce::Gtc;
        assert!(frac.problems()[0].contains("Fractional"));
        let mut ext = request("10", OrderType::Market, None);
        ext.extended_hours = true;
        assert!(ext.problems()[0].contains("Extended hours"));
        let mut stop = request("10", OrderType::StopLimit, Some("80"));
        assert!(stop.problems()[0].contains("stop price"));
        stop.stop_price = Some(d("80.5"));
        assert!(stop.problems().is_empty());
        assert_eq!(stop.reference_price(Some(d("81"))), Some(d("80")));
        assert_eq!(
            request("10", OrderType::Market, None).reference_price(Some(d("81"))),
            Some(d("81"))
        );
        assert_eq!(
            request("10", OrderType::Limit, Some("82.5")).describe(),
            "Buy 10 XLU · limit 82.50 · DAY"
        );
        assert_eq!(stop.describe(), "Buy 10 XLU · stop 80.50 limit 80.00 · DAY");
        assert_eq!(tick_for(d("0.99")), d("0.0001"));
    }

    #[test]
    fn option_requests_follow_the_option_rules() {
        let mut o = request("2", OrderType::Limit, Some("1.60"));
        o.symbol = "XLU261218C00045000".into();
        o.position_intent = Some(PositionIntent::BuyToOpen);
        assert!(o.problems().is_empty(), "{:?}", o.problems());
        assert_eq!(o.multiplier(), d("100"));
        assert_eq!(
            o.describe(),
            "Buy 2 XLU Dec 18 '26 45 call · limit 1.60 · DAY · to open"
        );
        let bad = |f: &dyn Fn(&mut OrderRequest)| {
            let mut x = o.clone();
            f(&mut x);
            x.problems()
        };
        assert!(bad(&|x| x.qty = d("1.5"))[0].contains("whole contracts"));
        assert!(bad(&|x| x.tif = TimeInForce::Ioc)[0].contains("DAY or GTC"));
        assert!(bad(&|x| x.tif = TimeInForce::Gtc).is_empty());
        assert!(bad(&|x| x.extended_hours = true)[0].contains("regular session"));
        assert!(bad(&|x| x.limit_price = Some(d("0.505")))[0].contains("finer than a cent"));
        assert!(
            bad(&|x| x.position_intent = Some(PositionIntent::SellToClose))[0]
                .contains("does not match")
        );
        let mut stock = request("10", OrderType::Limit, Some("82.50"));
        stock.position_intent = Some(PositionIntent::BuyToOpen);
        assert!(stock.problems()[0].contains("Only option orders"));
        // An option order's value is per contract of 100 shares.
        let mut filled = order("o", OrderSide::Buy, "2", "2", "filled");
        filled.symbol = "XLU261218C00045000".into();
        filled.class = AssetClass::Option;
        filled.filled_avg_price = Some(d("1.55"));
        assert_eq!(filled.value(None), d("310.00"));
    }

    #[test]
    fn values_merges_and_day_trades() {
        let buy = order("a", OrderSide::Buy, "10", "4", "partially_filled");
        assert_eq!(buy.remaining(), d("6"));
        // 4 filled at 44.00 plus 6 open at the 44.10 limit.
        assert_eq!(buy.value(None), d("440.60"));
        let cancelled = order("b", OrderSide::Buy, "10", "4", "canceled");
        assert_eq!(
            cancelled.value(None),
            d("176.00"),
            "a cancelled remainder counts nothing"
        );
        let day = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        assert_eq!(
            day_value(&[buy.clone(), cancelled.clone()], day, |_| None),
            d("616.60")
        );
        let other_day = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(
            day_value(std::slice::from_ref(&buy), other_day, |_| None),
            Decimal::ZERO
        );
        // A buy filled today makes today's sell a day trade, not today's buy.
        let filled = Order {
            filled_at: Some("2026-10-02T14:06:00Z".parse().unwrap()),
            ..order("c", OrderSide::Buy, "10", "10", "filled")
        };
        assert!(is_day_trade(
            std::slice::from_ref(&filled),
            day,
            "xlu",
            OrderSide::Sell
        ));
        assert!(!is_day_trade(
            std::slice::from_ref(&filled),
            day,
            "XLU",
            OrderSide::Buy
        ));
        assert!(!is_day_trade(
            std::slice::from_ref(&filled),
            other_day,
            "XLU",
            OrderSide::Sell
        ));

        // The stream's newer copy replaces the listed one; an older one does not.
        let mut newer = buy.clone();
        newer.status = OrderStatus::Filled;
        newer.filled_qty = d("10");
        newer.updated_at = Some("2026-10-02T14:09:00Z".parse().unwrap());
        let mut stale = cancelled.clone();
        stale.status = OrderStatus::New;
        stale.updated_at = Some("2026-10-02T13:00:00Z".parse().unwrap());
        let merged = merge_orders(&[buy, cancelled], [&newer, &stale]);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged.iter().find(|o| o.id == "a").unwrap().status,
            OrderStatus::Filled
        );
        assert_eq!(
            merged.iter().find(|o| o.id == "b").unwrap().status,
            OrderStatus::Canceled
        );
    }
}
