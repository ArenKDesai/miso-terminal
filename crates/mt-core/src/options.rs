//! Options: the contracts a broker lists ([`ContractInfo`], [`ContractList`]),
//! a chain by expiry and strike ([`chain_rows`]), the price steps exchanges
//! quote premiums in ([`tick_for`]), the expiry-day rules ([`expiry_cutoff`]),
//! whether an order opens or closes a position ([`PositionIntent`]), and
//! strategies of several legs ([`Leg`]): what they are called
//! ([`strategy_name`]), their net prices ([`net_price`]), what they pay at
//! expiry ([`payoff`]) and the margin Alpaca holds for them ([`requirement`]).
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

// --------------------------------------------------------------- strategies

/// The most legs a multi-leg order may have (Alpaca's limit).
pub const MAX_LEGS: usize = 4;

/// One leg of a multi-leg order: a contract, a side and its ratio.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Leg {
    /// The OCC symbol.
    pub symbol: String,
    pub side: OrderSide,
    /// Contracts of this leg in one unit of the strategy.
    pub ratio: u32,
    /// Opening or closing; worked out from the position when not set.
    pub intent: Option<PositionIntent>,
}

impl Leg {
    pub fn new(symbol: &str, side: OrderSide, ratio: u32) -> Self {
        Self {
            symbol: symbol.trim().to_ascii_uppercase(),
            side,
            ratio,
            intent: None,
        }
    }

    pub fn contract(&self) -> Option<OptionContract> {
        OptionContract::parse_occ(&self.symbol)
    }

    /// Contracts per unit, signed: positive bought, negative sold.
    pub fn signed(&self) -> i64 {
        match self.side {
            OrderSide::Buy => i64::from(self.ratio),
            OrderSide::Sell => -i64::from(self.ratio),
        }
    }

    /// `+1 XLU Dec 18 '26 45 call`.
    pub fn describe(&self) -> String {
        let what = self
            .contract()
            .map_or_else(|| self.symbol.clone(), |c| contract_words(&c));
        format!("{:+} {what}", self.signed())
    }
}

pub fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The ratios' common factor (1 when they are in lowest terms, as Alpaca
/// requires: 1:2, not 2:4).
pub fn common_factor(legs: &[Leg]) -> u32 {
    legs.iter().fold(0, |g, l| gcd(g, l.ratio))
}

/// What a combination of legs is called: `Bull call spread`, `Iron condor`,
/// `Long straddle`; `Custom` for anything else.
pub fn strategy_name(legs: &[Leg]) -> String {
    let parsed: Option<Vec<(OptionContract, i64)>> = legs
        .iter()
        .map(|l| Some((l.contract()?, l.signed())))
        .collect();
    let Some(all) = parsed.filter(|v| !v.is_empty()) else {
        return "Custom".into();
    };
    let first = &all[0].0;
    if all.iter().any(|(c, _)| c.underlying != first.underlying) {
        return "Custom".into();
    }
    let same_expiry = all.iter().all(|(c, _)| c.expiry == first.expiry);
    let side = |right: OptionRight| {
        let mut v: Vec<&(OptionContract, i64)> =
            all.iter().filter(|(c, _)| c.right == right).collect();
        v.sort_by(|a, b| {
            a.0.strike
                .cmp(&b.0.strike)
                .then_with(|| a.0.expiry.cmp(&b.0.expiry))
        });
        v
    };
    let (calls, puts) = (side(OptionRight::Call), side(OptionRight::Put));
    let word = |r: OptionRight| right_word(r);
    let capital = |r: OptionRight| match r {
        OptionRight::Call => "Call",
        OptionRight::Put => "Put",
    };
    let long_short = |r: i64| if r > 0 { "Long" } else { "Short" };
    match (calls.len(), puts.len()) {
        (1, 0) | (0, 1) => {
            let (c, r) = calls.first().or(puts.first()).copied().unwrap_or(&all[0]);
            format!("{} {}", long_short(*r), word(c.right))
        }
        (2, 0) | (0, 2) => {
            let v = if calls.len() == 2 { &calls } else { &puts };
            let ((a, ra), (b, rb)) = (v[0], v[1]);
            let right = a.right;
            if !same_expiry {
                if a.strike == b.strike {
                    format!("{} calendar spread", capital(right))
                } else {
                    format!("{} diagonal spread", capital(right))
                }
            } else if ra.signum() == rb.signum() {
                "Custom".into()
            } else if ra.abs() != rb.abs() {
                format!("{} ratio spread", capital(right))
            } else {
                // `a` has the lower strike.
                match (right, *ra > 0) {
                    (OptionRight::Call, true) => "Bull call spread".into(),
                    (OptionRight::Call, false) => "Bear call spread".into(),
                    (OptionRight::Put, true) => "Bull put spread".into(),
                    (OptionRight::Put, false) => "Bear put spread".into(),
                }
            }
        }
        (1, 1) if same_expiry => {
            let ((c, rc), (p, rp)) = (calls[0], puts[0]);
            if rc.signum() == rp.signum() && rc.abs() == rp.abs() {
                if c.strike == p.strike {
                    format!("{} straddle", long_short(*rc))
                } else {
                    format!("{} strangle", long_short(*rc))
                }
            } else if rc.abs() == rp.abs() {
                "Risk reversal".into()
            } else {
                "Custom".into()
            }
        }
        (3, 0) | (0, 3) if same_expiry => {
            let v = if calls.len() == 3 { &calls } else { &puts };
            let r: Vec<i64> = v.iter().map(|x| x.1).collect();
            let k: Vec<Decimal> = v.iter().map(|x| x.0.strike).collect();
            let shape = r[1] == -2 * r[0] && r[2] == r[0];
            if !shape {
                "Custom".into()
            } else if k[1] - k[0] == k[2] - k[1] {
                format!("{} {} butterfly", long_short(r[0]), word(v[0].0.right))
            } else {
                format!("Broken-wing {} butterfly", word(v[0].0.right))
            }
        }
        (4, 0) | (0, 4) if same_expiry => {
            let v = if calls.len() == 4 { &calls } else { &puts };
            let r: Vec<i64> = v.iter().map(|x| x.1).collect();
            if r[0] == r[3] && r[1] == r[2] && r[1] == -r[0] {
                format!("{} {} condor", long_short(r[0]), word(v[0].0.right))
            } else {
                "Custom".into()
            }
        }
        (2, 2) if same_expiry => {
            // Puts below calls: long, short, short, long is an iron condor.
            let r = [puts[0].1, puts[1].1, calls[0].1, calls[1].1];
            let equal = r.iter().all(|x| x.abs() == r[0].abs());
            let iron = r[0] > 0 && r[1] < 0 && r[2] < 0 && r[3] > 0;
            let reverse = r[0] < 0 && r[1] > 0 && r[2] > 0 && r[3] < 0;
            let body = if puts[1].0.strike == calls[0].0.strike {
                "iron butterfly"
            } else {
                "iron condor"
            };
            match (equal, iron, reverse) {
                (true, true, _) => format!("{}{}", body[..1].to_ascii_uppercase(), &body[1..]),
                (true, _, true) => format!("Reverse {body}"),
                _ => "Custom".into(),
            }
        }
        _ => "Custom".into(),
    }
}

/// A strategy's prices per unit, positive for a debit: what crossing the
/// spread pays (`natural`: buy at the ask, sell at the bid), the mid, and the
/// far side (buy at the bid, sell at the ask).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetPrice {
    pub natural: Option<Decimal>,
    pub mid: Option<Decimal>,
    pub far: Option<Decimal>,
}

/// Net prices from each leg's signed ratio, bid and ask.
pub fn net_price(legs: &[(i64, Option<Decimal>, Option<Decimal>)]) -> NetPrice {
    let mut natural = Some(Decimal::ZERO);
    let mut far = Some(Decimal::ZERO);
    for (r, bid, ask) in legs {
        let n = Decimal::from(r.unsigned_abs());
        let (pay, rest) = if *r > 0 { (*ask, *bid) } else { (*bid, *ask) };
        let sign = if *r > 0 {
            Decimal::ONE
        } else {
            Decimal::NEGATIVE_ONE
        };
        natural = natural.zip(pay).map(|(t, p)| t + sign * n * p);
        far = far.zip(rest).map(|(t, p)| t + sign * n * p);
    }
    if legs.is_empty() {
        return NetPrice::default();
    }
    NetPrice {
        natural,
        mid: natural
            .zip(far)
            .map(|(a, b)| ((a + b) / Decimal::TWO).round_dp(4)),
        far,
    }
}

/// What a strategy pays at expiry, per unit of it and per share (times the
/// multiplier for dollars): the net premium (positive paid), the most it can
/// make and lose (`None` when unlimited), where it breaks even, and the
/// strikes where the payoff bends.
#[derive(Clone, Debug, PartialEq)]
pub struct Payoff {
    pub net: f64,
    pub max_profit: Option<f64>,
    /// Zero or negative.
    pub max_loss: Option<f64>,
    pub breakevens: Vec<f64>,
    pub strikes: Vec<f64>,
    /// Each leg: right, strike, contracts per unit (signed).
    legs: Vec<(OptionRight, f64, f64)>,
}

const EPS: f64 = 1e-9;

impl Payoff {
    /// Profit or loss at expiry with the underlying at `spot`.
    pub fn at(&self, spot: f64) -> f64 {
        self.legs
            .iter()
            .map(|(right, k, r)| r * intrinsic(*right, *k, spot))
            .sum::<f64>()
            - self.net
    }

    /// The payoff from `lo` to `hi` as chart points: the ends and every
    /// strike between (it is straight in between).
    pub fn points(&self, lo: f64, hi: f64) -> Vec<[f64; 2]> {
        let mut xs = vec![lo];
        xs.extend(self.strikes.iter().copied().filter(|k| *k > lo && *k < hi));
        xs.push(hi);
        xs.into_iter().map(|x| [x, self.at(x)]).collect()
    }
}

/// The payoff at expiry of `legs` bought for `net` a unit (negative for a
/// credit). `None` unless every leg is an option on one underlying expiring
/// the same day.
pub fn payoff(legs: &[Leg], net: f64) -> Option<Payoff> {
    let parsed: Vec<(OptionContract, f64)> = legs
        .iter()
        .map(|l| Some((l.contract()?, l.signed() as f64)))
        .collect::<Option<_>>()?;
    let first = parsed.first()?.0.clone();
    if parsed
        .iter()
        .any(|(c, _)| c.expiry != first.expiry || c.underlying != first.underlying)
    {
        return None;
    }
    let legs: Vec<(OptionRight, f64, f64)> = parsed
        .iter()
        .map(|(c, r)| (c.right, crate::account::to_f64(c.strike), *r))
        .collect();
    let mut strikes: Vec<f64> = legs.iter().map(|l| l.1).collect();
    strikes.sort_by(f64::total_cmp);
    strikes.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let mut p = Payoff {
        net,
        max_profit: None,
        max_loss: None,
        breakevens: Vec::new(),
        strikes,
        legs,
    };
    // Beyond the highest strike only the calls move it.
    let slope: f64 = p
        .legs
        .iter()
        .filter(|l| l.0 == OptionRight::Call)
        .map(|l| l.2)
        .sum();
    let mut xs = vec![0.0];
    xs.extend(p.strikes.iter().copied().filter(|k| *k > 0.0));
    let ys: Vec<f64> = xs.iter().map(|x| p.at(*x)).collect();
    let hi = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lo = ys.iter().copied().fold(f64::INFINITY, f64::min);
    p.max_profit = (slope <= EPS).then_some(hi);
    p.max_loss = (slope >= -EPS).then_some(lo.min(0.0));
    // Where it crosses zero between the points, and beyond the last.
    let sign = |y: f64| {
        if y > EPS {
            1
        } else if y < -EPS {
            -1
        } else {
            0
        }
    };
    let n = xs.len();
    // Just past the last strike, by the slope (for a crossing exactly there).
    let after_last = ys[n - 1] + slope;
    for i in 0..n - 1 {
        let (x0, y0, x1, y1) = (xs[i], ys[i], xs[i + 1], ys[i + 1]);
        if sign(y0) * sign(y1) < 0 {
            p.breakevens.push(x0 + (-y0) * (x1 - x0) / (y1 - y0));
        } else if sign(y1) == 0 {
            // Touching zero at a strike counts when it crosses there.
            let next = ys.get(i + 2).copied().unwrap_or(after_last);
            if sign(y0) * sign(next) < 0 {
                p.breakevens.push(x1);
            }
        }
    }
    // Beyond the last strike the payoff is a straight line.
    let (xl, yl) = (xs[n - 1], ys[n - 1]);
    if slope.abs() > EPS && sign(yl) * sign(slope) < 0 {
        p.breakevens.push(xl - yl / slope);
    }
    p.breakevens.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    Some(p)
}

/// The margin Alpaca's universal spread rule holds for opening `legs`, per
/// unit and per share (times the multiplier for dollars): for each expiry,
/// the largest loss of those legs' payoff at expiry, premiums left out; the
/// largest of those. Alpaca wants every sold leg covered within the order,
/// so a sold call without a bought call of the same expiry (an unlimited
/// loss), or more puts sold than bought, is an error naming it.
pub fn requirement(legs: &[Leg]) -> Result<f64, String> {
    let parsed: Vec<(OptionContract, f64)> = legs
        .iter()
        .map(|l| {
            l.contract()
                .map(|c| (c, l.signed() as f64))
                .ok_or_else(|| format!("{} is not an option contract.", l.symbol))
        })
        .collect::<Result<_, _>>()?;
    let mut expiries: Vec<NaiveDate> = parsed.iter().map(|(c, _)| c.expiry).collect();
    expiries.sort();
    expiries.dedup();
    let mut worst = 0.0_f64;
    for day in expiries {
        let group: Vec<(OptionRight, f64, f64)> = parsed
            .iter()
            .filter(|(c, _)| c.expiry == day)
            .map(|(c, r)| (c.right, crate::account::to_f64(c.strike), *r))
            .collect();
        for right in [OptionRight::Call, OptionRight::Put] {
            let net: f64 = group.iter().filter(|g| g.0 == right).map(|g| g.2).sum();
            if net < -EPS {
                return Err(format!(
                    "More {}s are sold than bought for {}: Alpaca wants every sold leg covered by a bought one in the same order.",
                    right_word(right),
                    day.format("%b %d")
                ));
            }
        }
        let at = |x: f64| -> f64 {
            group
                .iter()
                .map(|(right, k, r)| r * intrinsic(*right, *k, x))
                .sum()
        };
        let mut xs: Vec<f64> = vec![0.0];
        xs.extend(group.iter().map(|g| g.1));
        let low = xs.iter().map(|x| at(*x)).fold(0.0_f64, f64::min);
        worst = worst.max(-low);
    }
    Ok(worst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use OrderSide::{Buy, Sell};

    fn leg(s: &str, side: OrderSide, r: u32) -> Leg {
        Leg::new(s, side, r)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    const C45: &str = "XLU261218C00045000";
    const C47: &str = "XLU261218C00047000";

    fn iron_condor() -> Vec<Leg> {
        vec![
            leg("XLU261218P00040000", Buy, 1),
            leg("XLU261218P00042000", Sell, 1),
            leg(C47, Sell, 1),
            leg("XLU261218C00049000", Buy, 1),
        ]
    }

    #[test]
    fn strategies_are_named() {
        let name = |legs: &[Leg]| strategy_name(legs);
        assert_eq!(
            name(&[leg(C45, Buy, 1), leg(C47, Sell, 1)]),
            "Bull call spread"
        );
        assert_eq!(
            name(&[leg(C45, Sell, 1), leg(C47, Buy, 1)]),
            "Bear call spread"
        );
        let (p42, p44) = ("XLU261218P00042000", "XLU261218P00044000");
        assert_eq!(
            name(&[leg(p42, Buy, 1), leg(p44, Sell, 1)]),
            "Bull put spread"
        );
        assert_eq!(
            name(&[leg(p42, Sell, 1), leg(p44, Buy, 1)]),
            "Bear put spread"
        );
        let p45 = "XLU261218P00045000";
        assert_eq!(name(&[leg(C45, Buy, 1), leg(p45, Buy, 1)]), "Long straddle");
        assert_eq!(
            name(&[leg(C47, Sell, 1), leg(p44, Sell, 1)]),
            "Short strangle"
        );
        assert_eq!(name(&iron_condor()), "Iron condor");
        let fly = [
            leg("XLU261218P00040000", Buy, 1),
            leg(p45, Sell, 1),
            leg(C45, Sell, 1),
            leg("XLU261218C00050000", Buy, 1),
        ];
        assert_eq!(name(&fly), "Iron butterfly");
        let butterfly = [
            leg("XLU261218C00043000", Buy, 1),
            leg(C45, Sell, 2),
            leg(C47, Buy, 1),
        ];
        assert_eq!(name(&butterfly), "Long call butterfly");
        let calendar = [leg("XLU261120C00045000", Sell, 1), leg(C45, Buy, 1)];
        assert_eq!(name(&calendar), "Call calendar spread");
        assert_eq!(
            name(&[leg(C45, Buy, 1), leg(C47, Sell, 2)]),
            "Call ratio spread"
        );
        assert_eq!(
            name(&[leg(C45, Buy, 1), leg("VST261120P00035000", Buy, 1)]),
            "Custom"
        );
        assert_eq!(name(&[leg(C45, Buy, 1)]), "Long call");
        assert_eq!(leg(C47, Sell, 2).describe(), "-2 XLU Dec 18 '26 47 call");
    }

    #[test]
    fn payoffs_at_expiry() {
        // A 45/47 bull call spread for 0.85.
        let p = payoff(&[leg(C45, Buy, 1), leg(C47, Sell, 1)], 0.85).unwrap();
        assert!(close(p.max_profit.unwrap(), 1.15) && close(p.max_loss.unwrap(), -0.85));
        assert_eq!(p.breakevens.len(), 1);
        assert!(close(p.breakevens[0], 45.85));
        assert!(close(p.at(40.0), -0.85) && close(p.at(50.0), 1.15));
        assert_eq!(p.points(40.0, 50.0).len(), 4, "the ends and both strikes");
        // A bought call gains without limit; a sold one loses without limit.
        let long = payoff(&[leg(C45, Buy, 1)], 1.6).unwrap();
        assert_eq!(long.max_profit, None);
        assert!(close(long.max_loss.unwrap(), -1.6) && close(long.breakevens[0], 46.6));
        let short = payoff(&[leg(C45, Sell, 1)], -1.55).unwrap();
        assert_eq!(short.max_loss, None);
        assert!(close(short.max_profit.unwrap(), 1.55) && close(short.breakevens[0], 46.55));
        // An iron condor for a 0.60 credit.
        let ic = payoff(&iron_condor(), -0.60).unwrap();
        assert!(close(ic.max_profit.unwrap(), 0.60) && close(ic.max_loss.unwrap(), -1.40));
        assert_eq!(ic.breakevens.len(), 2);
        assert!(close(ic.breakevens[0], 41.40) && close(ic.breakevens[1], 47.60));
        // A long straddle breaks even either side.
        let straddle =
            payoff(&[leg(C45, Buy, 1), leg("XLU261218P00045000", Buy, 1)], 3.40).unwrap();
        assert_eq!(straddle.max_profit, None);
        assert!(close(straddle.breakevens[0], 41.6) && close(straddle.breakevens[1], 48.4));
        // Different expiries have no single payoff.
        assert!(payoff(&[leg("XLU261120C00045000", Sell, 1), leg(C45, Buy, 1)], 0.3).is_none());
    }

    #[test]
    fn margin_ratios_and_net_prices() {
        assert!(close(
            requirement(&[leg(C45, Buy, 1), leg(C47, Sell, 1)]).unwrap(),
            0.0
        ));
        assert!(close(
            requirement(&[leg(C45, Sell, 1), leg(C47, Buy, 1)]).unwrap(),
            2.0
        ));
        assert!(
            close(requirement(&iron_condor()).unwrap(), 2.0),
            "the wider wing"
        );
        let butterfly = [
            leg("XLU261218C00043000", Buy, 1),
            leg(C45, Sell, 2),
            leg(C47, Buy, 1),
        ];
        assert!(close(requirement(&butterfly).unwrap(), 0.0));
        assert!(
            requirement(&[leg(C45, Sell, 1)])
                .unwrap_err()
                .contains("calls are sold")
        );
        assert!(
            requirement(&[leg("XLU261218P00044000", Sell, 1)])
                .unwrap_err()
                .contains("puts")
        );
        // A calendar's sold leg expires alone, so it is uncovered.
        assert!(requirement(&[leg("XLU261120C00045000", Sell, 1), leg(C45, Buy, 1)]).is_err());
        assert_eq!(common_factor(&[leg(C45, Buy, 2), leg(C47, Sell, 4)]), 2);
        assert_eq!(common_factor(&butterfly), 1);
        let d = |s: &str| Decimal::from_str(s).unwrap();
        let n = net_price(&[
            (1, Some(d("1.55")), Some(d("1.65"))),
            (-1, Some(d("0.77")), Some(d("0.84"))),
        ]);
        assert_eq!(n.natural, Some(d("0.88")));
        assert_eq!(n.far, Some(d("0.71")));
        assert_eq!(n.mid, Some(d("0.795")));
        assert_eq!(net_price(&[(1, None, Some(d("1")))]).natural, Some(d("1")));
        assert_eq!(net_price(&[(1, None, Some(d("1")))]).mid, None);
    }

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
