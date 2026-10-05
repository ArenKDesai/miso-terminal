//! Options: the contracts a broker lists ([`ContractInfo`], [`ContractList`]),
//! a chain by expiry and strike ([`chain_rows`]), the price steps exchanges
//! quote premiums in ([`tick_for`]), the expiry-day rules ([`expiry_cutoff`])
//! and whether an order opens or closes a position ([`PositionIntent`]).
//!
//! A contract is named by its OCC symbol ([`OptionContract`]). A standard
//! contract is for 100 shares and its premium is quoted per share. Quotes and
//! greeks are `f64`, for display; anything that goes into an order is a
//! [`Decimal`].

use chrono::{Datelike, NaiveDate, NaiveTime, Weekday};

use crate::account::OrderSide;
use crate::instrument::{OptionContract, OptionRight};
use crate::money::Decimal;

/// Shares per standard contract.
pub const MULTIPLIER: Decimal = Decimal::from_parts(100, 0, 0, false, 0);

/// Option classes quoted in pennies at any price (the rest step by 0.05 or
/// 0.10, or under the penny program by 0.01 below $3 and 0.05 above).
pub const PENNY_EVERYWHERE: [&str; 3] = ["SPY", "QQQ", "IWM"];

/// Underlyings whose contracts Alpaca takes orders for until 15:30 New York
/// time on their expiry day (broad-based ETFs; the rest stop at 15:15).
pub const LATE_CUTOFF: [&str; 2] = ["SPY", "QQQ"];

/// How a contract may be exercised.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Style {
    /// Any day until expiry (US equity options).
    #[default]
    American,
    /// At expiry only (most index options).
    European,
}

impl Style {
    pub fn parse(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("european") {
            Self::European
        } else {
            Self::American
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::American => "American",
            Self::European => "European",
        }
    }
}

/// A contract as the broker lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ContractInfo {
    /// The OCC symbol, compact: `XLU261218C00045000`.
    pub symbol: String,
    pub contract: OptionContract,
    /// The OCC root: the underlying's ticker, or another root (`XLU1`) for a
    /// contract adjusted after a split or merger.
    pub root: String,
    pub underlying: String,
    pub name: String,
    pub tradable: bool,
    pub active: bool,
    pub style: Style,
    /// What one contract's premium is multiplied by.
    pub multiplier: Decimal,
    /// Shares delivered on exercise.
    pub size: Decimal,
    pub open_interest: Option<u64>,
    pub open_interest_date: Option<NaiveDate>,
    /// The latest session's closing premium.
    pub close_price: Option<Decimal>,
    pub close_price_date: Option<NaiveDate>,
    /// Quoted in the penny program's steps.
    pub penny: bool,
}

impl ContractInfo {
    /// A plain contract: 100 shares of the underlying, under its own root.
    /// Adjusted contracts deliver something else and are priced differently.
    pub fn is_standard(&self) -> bool {
        self.root == self.underlying && self.multiplier == MULTIPLIER && self.size == MULTIPLIER
    }

    /// The step a premium of `price` moves in for this contract.
    pub fn tick(&self, price: Decimal) -> Decimal {
        tick_for(price, &self.underlying, self.penny)
    }
}

/// Every contract listed on one underlying, by OCC symbol (so by root,
/// expiry, right and strike).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContractList {
    pub underlying: String,
    pub contracts: Vec<ContractInfo>,
}

impl ContractList {
    pub fn new(underlying: &str, mut contracts: Vec<ContractInfo>) -> Self {
        contracts.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        contracts.dedup_by(|a, b| a.symbol == b.symbol);
        Self {
            underlying: underlying.trim().to_ascii_uppercase(),
            contracts,
        }
    }

    pub fn get(&self, symbol: &str) -> Option<&ContractInfo> {
        let upper = symbol.trim().to_ascii_uppercase();
        self.contracts
            .binary_search_by(|c| c.symbol.as_str().cmp(&upper))
            .ok()
            .map(|i| &self.contracts[i])
    }

    /// Expiry dates of the standard contracts, soonest first.
    pub fn expiries(&self) -> Vec<NaiveDate> {
        let mut out: Vec<NaiveDate> = self
            .contracts
            .iter()
            .filter(|c| c.is_standard())
            .map(|c| c.contract.expiry)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The standard contracts expiring on `expiry`.
    pub fn on(&self, expiry: NaiveDate) -> impl Iterator<Item = &ContractInfo> {
        self.contracts
            .iter()
            .filter(move |c| c.contract.expiry == expiry && c.is_standard())
    }

    /// How many adjusted (non-standard) contracts there are.
    pub fn adjusted(&self) -> usize {
        self.contracts.iter().filter(|c| !c.is_standard()).count()
    }
}

/// One strike of a chain: its call and its put (OCC symbols), when listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainRow {
    pub strike: Decimal,
    pub call: Option<String>,
    pub put: Option<String>,
}

impl ChainRow {
    pub fn symbol(&self, right: OptionRight) -> Option<&str> {
        match right {
            OptionRight::Call => self.call.as_deref(),
            OptionRight::Put => self.put.as_deref(),
        }
    }
}

/// The chain for one expiry, lowest strike first: every OCC symbol under
/// `root` expiring on `expiry` (from the contract list, a quote source, or
/// both; repeats and other contracts are ignored).
pub fn chain_rows<'a>(
    root: &str,
    expiry: NaiveDate,
    symbols: impl IntoIterator<Item = &'a str>,
) -> Vec<ChainRow> {
    let root = root.trim().to_ascii_uppercase();
    let mut rows: Vec<ChainRow> = Vec::new();
    for s in symbols {
        let Some(c) = OptionContract::parse_occ(s) else {
            continue;
        };
        if c.underlying != root || c.expiry != expiry {
            continue;
        }
        let Some(occ) = c.occ() else { continue };
        let at = match rows.binary_search_by(|r| r.strike.cmp(&c.strike)) {
            Ok(i) => i,
            Err(i) => {
                rows.insert(
                    i,
                    ChainRow {
                        strike: c.strike,
                        call: None,
                        put: None,
                    },
                );
                i
            }
        };
        let slot = match c.right {
            OptionRight::Call => &mut rows[at].call,
            OptionRight::Put => &mut rows[at].put,
        };
        slot.get_or_insert(occ);
    }
    rows
}

/// The row whose strike is nearest `spot` (the higher one on a tie).
pub fn atm_index(rows: &[ChainRow], spot: f64) -> Option<usize> {
    if !spot.is_finite() {
        return None;
    }
    let dist = |r: &ChainRow| (crate::account::to_f64(r.strike) - spot).abs();
    (0..rows.len()).min_by(|&a, &b| {
        dist(&rows[a])
            .total_cmp(&dist(&rows[b]))
            .then_with(|| b.cmp(&a))
    })
}

/// The rows within `n` strikes of the money (all of them without a spot).
pub fn around(rows: &[ChainRow], spot: Option<f64>, n: usize) -> std::ops::Range<usize> {
    match spot.and_then(|s| atm_index(rows, s)) {
        Some(i) => i.saturating_sub(n)..(i + n + 1).min(rows.len()),
        None => 0..rows.len(),
    }
}

/// What a contract would be worth exercised now, per share.
pub fn intrinsic(right: OptionRight, strike: f64, spot: f64) -> f64 {
    match right {
        OptionRight::Call => (spot - strike).max(0.0),
        OptionRight::Put => (strike - spot).max(0.0),
    }
}

/// Whether a contract is in the money at `spot`.
pub fn is_itm(c: &OptionContract, spot: f64) -> bool {
    intrinsic(c.right, crate::account::to_f64(c.strike), spot) > 0.0
}

/// Calendar days from `today` to expiry (0 on the day, negative after).
pub fn days_to_expiry(expiry: NaiveDate, today: NaiveDate) -> i64 {
    (expiry - today).num_days()
}

/// The third Friday of its month: the standard monthly expiry.
pub fn is_monthly(d: NaiveDate) -> bool {
    d.weekday() == Weekday::Fri && (15..=21).contains(&d.day())
}

/// When Alpaca stops taking orders for contracts on `underlying` expiring
/// that day, New York time: 15:15, or 15:30 for broad-based ETFs.
pub fn expiry_cutoff(underlying: &str) -> NaiveTime {
    let late = LATE_CUTOFF
        .iter()
        .any(|u| u.eq_ignore_ascii_case(underlying.trim()));
    if late {
        NaiveTime::from_hms_opt(15, 30, 0).unwrap_or_default()
    } else {
        NaiveTime::from_hms_opt(15, 15, 0).unwrap_or_default()
    }
}

/// The step exchanges quote a premium of `price` in: pennies for the
/// classes quoted in pennies throughout (`SPY`, `QQQ`, `IWM`); under the
/// penny program 0.01 below $3 and 0.05 from $3; otherwise 0.05 below $3
/// and 0.10 from $3. An order off the step may be routed elsewhere or
/// refused.
pub fn tick_for(price: Decimal, underlying: &str, penny: bool) -> Decimal {
    let cents = |n: i64| Decimal::new(n, 2);
    let everywhere = PENNY_EVERYWHERE
        .iter()
        .any(|u| u.eq_ignore_ascii_case(underlying.trim()));
    let below_three = price < Decimal::from(3);
    match (everywhere, penny, below_three) {
        (true, _, _) => cents(1),
        (false, true, true) => cents(1),
        (false, true, false) => cents(5),
        (false, false, true) => cents(5),
        (false, false, false) => cents(10),
    }
}

/// What an option order does to a position (Alpaca's `position_intent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PositionIntent {
    BuyToOpen,
    BuyToClose,
    SellToOpen,
    SellToClose,
}

impl PositionIntent {
    pub const ALL: [Self; 4] = [
        Self::BuyToOpen,
        Self::BuyToClose,
        Self::SellToOpen,
        Self::SellToClose,
    ];

    /// Alpaca's name: `buy_to_open`…
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuyToOpen => "buy_to_open",
            Self::BuyToClose => "buy_to_close",
            Self::SellToOpen => "sell_to_open",
            Self::SellToClose => "sell_to_close",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|i| i.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// `Buy to open`…
    pub fn label(self) -> &'static str {
        match self {
            Self::BuyToOpen => "Buy to open",
            Self::BuyToClose => "Buy to close",
            Self::SellToOpen => "Sell to open",
            Self::SellToClose => "Sell to close",
        }
    }

    pub fn side(self) -> OrderSide {
        match self {
            Self::BuyToOpen | Self::BuyToClose => OrderSide::Buy,
            Self::SellToOpen | Self::SellToClose => OrderSide::Sell,
        }
    }

    pub fn opens(self) -> bool {
        matches!(self, Self::BuyToOpen | Self::SellToOpen)
    }

    /// What an order on `side` does to a position of `held` contracts: it
    /// closes a position on the other side, else it opens one.
    pub fn of(side: OrderSide, held: Decimal) -> Self {
        match side {
            OrderSide::Buy if held.is_sign_negative() && !held.is_zero() => Self::BuyToClose,
            OrderSide::Buy => Self::BuyToOpen,
            OrderSide::Sell if held > Decimal::ZERO => Self::SellToClose,
            OrderSide::Sell => Self::SellToOpen,
        }
    }
}

/// `XLU 45 call`, `VST 35 put`.
pub fn right_word(right: OptionRight) -> &'static str {
    match right {
        OptionRight::Call => "call",
        OptionRight::Put => "put",
    }
}

/// `XLU 45 call · Dec 18 '26`.
pub fn contract_name(c: &OptionContract) -> String {
    format!(
        "{} {} {} · {}",
        c.underlying,
        c.strike.normalize(),
        right_word(c.right),
        c.expiry.format("%b %d '%y")
    )
}

/// `XLU Dec 18 '26 45 call`, for one-line order descriptions.
pub fn contract_words(c: &OptionContract) -> String {
    format!(
        "{} {} {} {}",
        c.underlying,
        c.expiry.format("%b %d '%y"),
        c.strike.normalize(),
        right_word(c.right)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn intents_follow_the_position() {
        use PositionIntent as I;
        assert_eq!(I::of(OrderSide::Buy, Decimal::ZERO), I::BuyToOpen);
        assert_eq!(I::of(OrderSide::Buy, Decimal::from(2)), I::BuyToOpen);
        assert_eq!(I::of(OrderSide::Buy, Decimal::from(-1)), I::BuyToClose);
        assert_eq!(I::of(OrderSide::Sell, Decimal::from(2)), I::SellToClose);
        assert_eq!(I::of(OrderSide::Sell, Decimal::ZERO), I::SellToOpen);
        for i in I::ALL {
            assert_eq!(I::parse(i.as_str()), Some(i));
        }
        assert!(I::SellToOpen.opens() && !I::BuyToClose.opens());
        assert_eq!(I::SellToClose.side(), OrderSide::Sell);
        assert_eq!(I::BuyToClose.label(), "Buy to close");
    }

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn info(symbol: &str) -> ContractInfo {
        let contract = OptionContract::parse_occ(symbol).unwrap();
        ContractInfo {
            symbol: symbol.into(),
            root: contract.underlying.clone(),
            underlying: contract.underlying.clone(),
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
            penny: false,
            contract,
        }
    }

    #[test]
    fn lists_find_expiries_and_hide_adjusted_contracts() {
        let mut adjusted = info("XLU1261218C00045000");
        adjusted.underlying = "XLU".into();
        let list = ContractList::new(
            "xlu",
            vec![
                info("XLU261218P00045000"),
                info("XLU261016C00044000"),
                info("XLU261218C00045000"),
                info("XLU261016C00044000"),
                adjusted,
            ],
        );
        assert_eq!(list.contracts.len(), 4, "repeats dropped");
        assert_eq!(
            list.expiries(),
            [date("2026-10-16"), date("2026-12-18")],
            "the adjusted root is left out"
        );
        assert_eq!(list.adjusted(), 1);
        assert!(list.get("xlu261218c00045000").is_some());
        assert!(list.get("XLU261218C00046000").is_none());
        assert_eq!(list.on(date("2026-12-18")).count(), 2);
    }

    #[test]
    fn chains_pair_calls_and_puts_by_strike() {
        let rows = chain_rows(
            "XLU",
            date("2026-12-18"),
            [
                "XLU261218P00045000",
                "XLU261218C00046000",
                "XLU261218C00045000",
                "XLU261218C00044500",
                "XLU261016C00045000",
                "XEL261218C00045000",
                "XLU1261218C00045000",
                "junk",
            ],
        );
        let strikes: Vec<String> = rows.iter().map(|r| r.strike.to_string()).collect();
        assert_eq!(strikes, ["44.5", "45", "46"]);
        assert_eq!(rows[1].call.as_deref(), Some("XLU261218C00045000"));
        assert_eq!(rows[1].put.as_deref(), Some("XLU261218P00045000"));
        assert!(rows[0].put.is_none());
        assert_eq!(atm_index(&rows, 45.2), Some(1));
        assert_eq!(atm_index(&rows, 45.5), Some(2), "a tie goes up");
        assert_eq!(around(&rows, Some(46.0), 1), 1..3);
        assert_eq!(around(&rows, None, 1), 0..3);
        assert_eq!(atm_index(&[], 45.0), None);
    }

    #[test]
    fn steps_cutoffs_and_value() {
        assert_eq!(tick_for(d("2.95"), "XLU", false), d("0.05"));
        assert_eq!(tick_for(d("3.00"), "XLU", false), d("0.10"));
        assert_eq!(tick_for(d("2.95"), "XLU", true), d("0.01"));
        assert_eq!(tick_for(d("3.10"), "XLU", true), d("0.05"));
        assert_eq!(tick_for(d("12.34"), "spy", false), d("0.01"));
        assert_eq!(
            expiry_cutoff("XLU"),
            NaiveTime::from_hms_opt(15, 15, 0).unwrap()
        );
        assert_eq!(
            expiry_cutoff("QQQ"),
            NaiveTime::from_hms_opt(15, 30, 0).unwrap()
        );
        assert!(is_monthly(date("2026-10-16")));
        assert!(!is_monthly(date("2026-10-09")));
        assert!(!is_monthly(date("2026-10-15")), "a Thursday");
        assert_eq!(days_to_expiry(date("2026-10-16"), date("2026-10-02")), 14);
        let call = OptionContract::parse_occ("XLU261218C00045000").unwrap();
        assert!(is_itm(&call, 45.5) && !is_itm(&call, 45.0));
        assert_eq!(intrinsic(OptionRight::Put, 45.0, 40.0), 5.0);
        assert_eq!(contract_name(&call), "XLU 45 call · Dec 18 '26");
        assert_eq!(contract_words(&call), "XLU Dec 18 '26 45 call");
        assert_eq!(Style::parse("EUROPEAN"), Style::European);
    }
}
