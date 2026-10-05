//! A brokerage account as the broker reports it: balances and margin,
//! positions, the equity curve, the account's activities (fills, dividends,
//! fees, option exercises and expiries) and order events as they stream.
//!
//! Balances, quantities, prices and P&L are exact ([`Decimal`]); the equity
//! curve is for charts and uses `f64`. Times are instants: show them in New
//! York time ([`crate::exchange`]).

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::prelude::ToPrimitive;

use crate::instrument::OptionContract;
use crate::money::Decimal;

/// Below this equity, a margin account may make at most three day trades in
/// five business days (FINRA's pattern-day-trader rule).
pub const PDT_MIN_EQUITY: Decimal = Decimal::from_parts(25_000, 0, 0, false, 0);
/// Day trades allowed in five business days below [`PDT_MIN_EQUITY`].
pub const PDT_DAY_TRADES: u32 = 3;

/// `f64` for display and ratios.
pub fn to_f64(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

/// An `f64` price as an exact decimal, rounded to `dp` places. `None` for
/// NaN and infinities.
pub fn from_f64(v: f64, dp: u32) -> Option<Decimal> {
    Decimal::try_from(v).ok().map(|d| d.round_dp(dp))
}

/// `part / whole` as a fraction, if `whole` is not zero.
fn ratio(part: Decimal, whole: Decimal) -> Option<f64> {
    (!whole.is_zero()).then(|| to_f64(part) / to_f64(whole))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "buy" | "buy_to_open" | "buy_to_close" => Some(Self::Buy),
            "sell" | "sell_short" | "sell_to_open" | "sell_to_close" => Some(Self::Sell),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Buy => "Buy",
            Self::Sell => "Sell",
        }
    }

    /// `+1` for a buy, `-1` for a sell: the sign of the change in shares.
    pub fn sign(self) -> Decimal {
        match self {
            Self::Buy => Decimal::ONE,
            Self::Sell => Decimal::NEGATIVE_ONE,
        }
    }
}

/// What kind of asset a position holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AssetClass {
    Equity,
    Option,
    Crypto,
    Other(String),
}

impl AssetClass {
    /// Alpaca's names: `us_equity`, `us_option`, `crypto`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "us_equity" | "equity" | "" => Self::Equity,
            "us_option" | "option" => Self::Option,
            "crypto" => Self::Crypto,
            other => Self::Other(other.to_owned()),
        }
    }
}

// ------------------------------------------------------------------ account

/// Balances, buying power, margin and the account's standing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    /// As the broker sends it; show it masked ([`mask_number`]).
    pub number: String,
    /// `ACTIVE`, `ACCOUNT_UPDATED`, `ACTION_REQUIRED`…
    pub status: String,
    pub currency: String,
    pub created: Option<DateTime<Utc>>,
    pub cash: Decimal,
    /// Cash plus the market value of every position.
    pub equity: Decimal,
    /// Equity at the previous trading day's close.
    pub last_equity: Decimal,
    pub buying_power: Decimal,
    pub regt_buying_power: Option<Decimal>,
    pub daytrading_buying_power: Option<Decimal>,
    pub non_marginable_buying_power: Option<Decimal>,
    pub options_buying_power: Option<Decimal>,
    pub long_market_value: Decimal,
    /// Negative (or zero).
    pub short_market_value: Decimal,
    pub initial_margin: Option<Decimal>,
    pub maintenance_margin: Option<Decimal>,
    pub last_maintenance_margin: Option<Decimal>,
    /// Special memorandum account (Reg T).
    pub sma: Option<Decimal>,
    /// 1 (cash), 2 (Reg T margin) or 4 (pattern day trader).
    pub multiplier: Option<Decimal>,
    pub accrued_fees: Option<Decimal>,
    pub pending_transfer_in: Option<Decimal>,
    pub pending_transfer_out: Option<Decimal>,
    pub pattern_day_trader: bool,
    /// Day trades in the last five business days.
    pub daytrade_count: u32,
    pub trading_blocked: bool,
    pub transfers_blocked: bool,
    pub account_blocked: bool,
    pub trade_suspended_by_user: bool,
    pub shorting_enabled: bool,
    pub options_approved_level: Option<u8>,
    pub options_trading_level: Option<u8>,
}

impl Account {
    /// Today's change in equity (deposits and withdrawals included, as the
    /// broker counts it).
    pub fn day_pl(&self) -> Decimal {
        self.equity - self.last_equity
    }

    /// Today's change as a fraction of yesterday's equity.
    pub fn day_pl_pct(&self) -> Option<f64> {
        ratio(self.day_pl(), self.last_equity)
    }

    /// Equity above the maintenance requirement.
    pub fn excess_equity(&self) -> Option<Decimal> {
        self.maintenance_margin.map(|m| self.equity - m)
    }

    /// The maintenance requirement as a fraction of equity.
    pub fn margin_used(&self) -> Option<f64> {
        self.maintenance_margin.and_then(|m| ratio(m, self.equity))
    }

    /// Day trades left in the five-day window, or `None` when the account is
    /// above [`PDT_MIN_EQUITY`] and the limit does not apply.
    pub fn day_trades_left(&self) -> Option<u32> {
        (self.equity < PDT_MIN_EQUITY).then(|| PDT_DAY_TRADES.saturating_sub(self.daytrade_count))
    }

    /// Anything that stops trading or transfers.
    pub fn restrictions(&self) -> Vec<&'static str> {
        [
            (self.account_blocked, "account blocked"),
            (self.trading_blocked, "trading blocked"),
            (self.trade_suspended_by_user, "trading suspended by you"),
            (self.transfers_blocked, "transfers blocked"),
        ]
        .into_iter()
        .filter_map(|(on, what)| on.then_some(what))
        .collect()
    }

    pub fn is_active(&self) -> bool {
        self.status.eq_ignore_ascii_case("ACTIVE")
    }

    /// What the multiplier makes the account: cash, margin or day trading.
    pub fn margin_label(&self) -> &'static str {
        match self
            .multiplier
            .map(|m| m.normalize().to_string())
            .as_deref()
        {
            Some("1") => "Cash account (1×)",
            Some("2") => "Reg T margin (2×)",
            Some("4") => "Pattern day trader margin (4×)",
            _ => "Margin",
        }
    }
}

/// `PA3XYZ1234` -> `••••1234`, so screenshots never carry an account number.
pub fn mask_number(number: &str) -> String {
    let n = number.trim();
    let tail: String = n
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if n.chars().count() <= 4 {
        "••••".into()
    } else {
        format!("••••{tail}")
    }
}

/// What an options approval level allows (Alpaca's levels 0 to 3).
pub fn options_level_label(level: u8) -> &'static str {
    match level {
        0 => "Options disabled",
        1 => "Covered calls and cash-secured puts",
        2 => "Level 1, plus buying calls and puts",
        _ => "Level 2, plus spreads",
    }
}

// ---------------------------------------------------------------- positions

/// One open position, as the broker valued it when asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    /// A ticker, or an OCC symbol for an option.
    pub symbol: String,
    pub class: AssetClass,
    pub exchange: String,
    /// Shares or contracts; negative when short.
    pub qty: Decimal,
    /// What is not held for open orders.
    pub qty_available: Option<Decimal>,
    pub avg_entry_price: Decimal,
    /// What the position cost (negative for a short: proceeds).
    pub cost_basis: Decimal,
    pub market_value: Option<Decimal>,
    pub current_price: Option<Decimal>,
    /// The previous close.
    pub lastday_price: Option<Decimal>,
    /// Change since the previous close, as a fraction.
    pub change_today: Option<Decimal>,
    pub unrealized_pl: Option<Decimal>,
    pub unrealized_plpc: Option<Decimal>,
    /// Since the previous close, or since entry for a position opened today.
    pub unrealized_intraday_pl: Option<Decimal>,
    pub unrealized_intraday_plpc: Option<Decimal>,
}

/// A position valued at a price: the broker's, or a newer one from the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Marked {
    pub price: Decimal,
    pub market_value: Decimal,
    pub unrealized_pl: Option<Decimal>,
    pub unrealized_pct: Option<f64>,
    pub day_pl: Option<Decimal>,
    pub day_pct: Option<f64>,
    /// How far the market value moved from the broker's figure.
    pub delta: Decimal,
    /// Whether a newer price than the broker's was used.
    pub live: bool,
}

impl Position {
    pub fn is_short(&self) -> bool {
        self.qty.is_sign_negative() && !self.qty.is_zero()
    }

    /// The contract, for an option.
    pub fn contract(&self) -> Option<OptionContract> {
        match self.class {
            AssetClass::Equity | AssetClass::Crypto => None,
            _ => OptionContract::parse_occ(&self.symbol),
        }
    }

    pub fn is_option(&self) -> bool {
        self.class == AssetClass::Option || self.contract().is_some()
    }

    /// Shares per unit: 100 for a standard option, 1 otherwise.
    pub fn multiplier(&self) -> Decimal {
        if self.is_option() {
            Decimal::ONE_HUNDRED
        } else {
            Decimal::ONE
        }
    }

    /// The stock an option is on, or the position's own symbol.
    pub fn underlying(&self) -> String {
        self.contract()
            .map_or_else(|| self.symbol.clone(), |c| c.underlying)
    }

    /// The broker's own price for the position.
    fn base_price(&self) -> Decimal {
        self.current_price.unwrap_or(self.avg_entry_price)
    }

    /// Value the position at `live` when given, else at the broker's price.
    /// P&L moves by the change in price times the quantity, so the broker's
    /// own reference for today (the previous close, or the entry price for a
    /// position opened today) is kept.
    pub fn mark(&self, live: Option<Decimal>) -> Marked {
        let base = self.base_price();
        let units = self.qty * self.multiplier();
        let price = live.unwrap_or(base);
        let delta = (price - base) * units;
        let base_value = self.market_value.unwrap_or(base * units);
        let market_value = base_value + delta;
        let unrealized_pl = self
            .unrealized_pl
            .or_else(|| self.current_price.map(|_| base_value - self.cost_basis))
            .map(|u| u + delta);
        let unrealized_pct = unrealized_pl.and_then(|u| ratio(u, self.cost_basis.abs()));
        let day_pl = self.unrealized_intraday_pl.map(|d| d + delta);
        // Today's reference value: what the position was worth at the
        // previous close (or cost, if bought today).
        let day_pct = self
            .unrealized_intraday_pl
            .and_then(|d| ratio(day_pl?, (base_value - d).abs()));
        Marked {
            price,
            market_value,
            unrealized_pl,
            unrealized_pct,
            day_pl,
            day_pct,
            delta,
            live: live.is_some_and(|l| l != base),
        }
    }
}

/// Option positions by underlying, in symbol order.
pub fn options_by_underlying(positions: &[Position]) -> BTreeMap<String, Vec<&Position>> {
    let mut out: BTreeMap<String, Vec<&Position>> = BTreeMap::new();
    for p in positions.iter().filter(|p| p.is_option()) {
        out.entry(p.underlying()).or_default().push(p);
    }
    for list in out.values_mut() {
        list.sort_by_key(|a| a.contract());
    }
    out
}

/// Net delta in shares of the underlying: the shares held plus each option's
/// contracts × 100 × its delta. `None` if any option's delta is unknown.
pub fn net_delta(
    shares: Decimal,
    options: impl IntoIterator<Item = (Decimal, Option<f64>)>,
) -> Option<f64> {
    let mut total = to_f64(shares);
    for (contracts, delta) in options {
        total += to_f64(contracts) * 100.0 * delta?;
    }
    Some(total)
}

// ---------------------------------------------------------- equity history

/// One point of the equity curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EquityPoint {
    pub time: DateTime<Utc>,
    pub equity: f64,
    /// Against the history's base value.
    pub profit_loss: f64,
    pub profit_loss_pct: Option<f64>,
}

/// The account's equity over a period.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortfolioHistory {
    pub points: Vec<EquityPoint>,
    /// What P&L is measured from: the closing equity before the period.
    pub base_value: Option<f64>,
    pub base_as_of: Option<NaiveDate>,
    /// `5Min`, `1H`, `1D`.
    pub timeframe: String,
}

impl PortfolioHistory {
    pub fn last(&self) -> Option<&EquityPoint> {
        self.points.last()
    }

    /// The base, or the first point's equity.
    pub fn start_value(&self) -> Option<f64> {
        self.base_value
            .filter(|b| *b != 0.0)
            .or_else(|| self.points.first().map(|p| p.equity))
    }

    /// The change over the period, in dollars and as a fraction.
    pub fn change(&self) -> Option<(f64, Option<f64>)> {
        let (start, end) = (self.start_value()?, self.last()?.equity);
        Some((end - start, (start != 0.0).then(|| end / start - 1.0)))
    }

    /// The deepest fall from a running high, as a fraction (negative or zero).
    pub fn max_drawdown(&self) -> Option<f64> {
        let mut high = self.start_value()?;
        let mut worst = 0.0_f64;
        for p in &self.points {
            high = high.max(p.equity);
            if high > 0.0 {
                worst = worst.min(p.equity / high - 1.0);
            }
        }
        Some(worst)
    }

    pub fn range(&self) -> Option<(f64, f64)> {
        let mut it = self.points.iter().map(|p| p.equity);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }
}

// --------------------------------------------------------------- activities

/// What an activity is, for filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActivityCategory {
    Fill,
    Dividend,
    Interest,
    Fee,
    Transfer,
    Option,
    CorporateAction,
    Other,
}

impl ActivityCategory {
    pub const ALL: [Self; 8] = [
        Self::Fill,
        Self::Dividend,
        Self::Interest,
        Self::Fee,
        Self::Transfer,
        Self::Option,
        Self::CorporateAction,
        Self::Other,
    ];

    /// From the broker's activity code (`FILL`, `DIV`, `OPEXP`…).
    pub fn of(code: &str) -> Self {
        let code = code.trim().to_ascii_uppercase();
        match code.as_str() {
            "FILL" | "PARTIAL_FILL" => Self::Fill,
            c if c.starts_with("DIV") && c != "DIVFEE" => Self::Dividend,
            "CGD" => Self::Dividend,
            c if c.starts_with("INT") => Self::Interest,
            "FEE" | "CFEE" | "DIVFEE" | "PTC" | "PTR" | "REG" | "TAF" => Self::Fee,
            "CSD" | "CSW" | "TRANS" | "JNL" | "JNLC" | "JNLS" | "ACATC" | "ACATS" => Self::Transfer,
            c if c.starts_with("OP") => Self::Option,
            "MA" | "NC" | "SC" | "SPIN" | "SSO" | "SSP" | "SPLIT" | "REORG" | "REO" | "CIL"
            | "VOF" => Self::CorporateAction,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fill => "Fills",
            Self::Dividend => "Dividends",
            Self::Interest => "Interest",
            Self::Fee => "Fees",
            Self::Transfer => "Transfers",
            Self::Option => "Option events",
            Self::CorporateAction => "Corporate actions",
            Self::Other => "Other",
        }
    }
}

/// A plain name for an activity code.
pub fn activity_name(code: &str) -> &'static str {
    match code.trim().to_ascii_uppercase().as_str() {
        "FILL" => "Fill",
        "PARTIAL_FILL" => "Partial fill",
        "DIV" => "Dividend",
        "DIVCGL" => "Dividend (long-term gain)",
        "DIVCGS" => "Dividend (short-term gain)",
        "DIVFEE" => "Dividend fee",
        "DIVFT" => "Dividend (foreign tax withheld)",
        "DIVNRA" => "Dividend (NRA withholding)",
        "DIVROC" => "Dividend (return of capital)",
        "DIVTW" => "Dividend (withholding)",
        "DIVTXEX" => "Dividend (tax exempt)",
        "CGD" => "Capital gain distribution",
        "INT" => "Interest",
        "INTNRA" => "Interest (NRA withholding)",
        "INTTW" => "Interest (withholding)",
        "FEE" => "Fee",
        "CFEE" => "Crypto fee",
        "PTC" => "Pass-through charge",
        "PTR" => "Pass-through rebate",
        "CSD" => "Cash deposit",
        "CSW" => "Cash withdrawal",
        "TRANS" => "Cash transfer",
        "JNL" | "JNLC" => "Cash journal",
        "JNLS" => "Stock journal",
        "ACATC" => "ACATS transfer (cash)",
        "ACATS" => "ACATS transfer (securities)",
        "OPASN" => "Option assigned",
        "OPEXP" => "Option expired",
        "OPEXC" | "OPXRC" => "Option exercised",
        "OPTRD" => "Option trade",
        "OPCA" => "Option corporate action",
        "OPCSH" => "Option cash settlement",
        "MA" => "Merger or acquisition",
        "NC" => "Name change",
        "SC" => "Symbol change",
        "SPIN" | "SSO" => "Spin-off",
        "SPLIT" | "SSP" => "Stock split",
        "REORG" | "REO" => "Reorganisation",
        "CIL" => "Cash in lieu",
        "VOF" => "Voluntary offer",
        "MEM" => "Memo",
        "MISC" => "Miscellaneous",
        _ => "Activity",
    }
}

/// One entry in the account's history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    /// Sortable: the broker's ids start with a timestamp.
    pub id: String,
    /// `FILL`, `DIV`, `OPEXP`…
    pub code: String,
    pub sub_code: Option<String>,
    /// When it happened (fills), or was recorded.
    pub time: Option<DateTime<Utc>>,
    /// The date it applies to (dividends, fees, transfers).
    pub date: Option<NaiveDate>,
    pub symbol: Option<String>,
    pub side: Option<OrderSide>,
    pub qty: Option<Decimal>,
    pub price: Option<Decimal>,
    /// Cash in (positive) or out (negative), when the broker states it.
    pub net_amount: Option<Decimal>,
    pub per_share_amount: Option<Decimal>,
    pub description: String,
    pub status: Option<String>,
    pub order_id: Option<String>,
    pub order_status: Option<String>,
    /// For a fill: what was left of the order, and what had filled in all.
    pub leaves_qty: Option<Decimal>,
    pub cum_qty: Option<Decimal>,
    /// `fill` or `partial_fill`.
    pub fill_type: Option<String>,
}

impl Activity {
    pub fn category(&self) -> ActivityCategory {
        ActivityCategory::of(&self.code)
    }

    pub fn name(&self) -> &'static str {
        if self.category() == ActivityCategory::Fill
            && self.fill_type.as_deref() == Some("partial_fill")
        {
            return activity_name("PARTIAL_FILL");
        }
        activity_name(&self.code)
    }

    /// For ordering: the time, or noon UTC on its date.
    pub fn when(&self) -> Option<DateTime<Utc>> {
        self.time.or_else(|| {
            self.date
                .and_then(|d| d.and_hms_opt(12, 0, 0))
                .map(|t| t.and_utc())
        })
    }

    /// The contract, if the symbol is an option's.
    pub fn contract(&self) -> Option<OptionContract> {
        self.symbol.as_deref().and_then(OptionContract::parse_occ)
    }

    /// The cash it moved: the broker's net amount, or for a fill, what was
    /// paid (negative) or received.
    pub fn amount(&self) -> Option<Decimal> {
        if let Some(n) = self.net_amount {
            return Some(n);
        }
        let (side, qty, price) = (self.side?, self.qty?, self.price?);
        let mult = if self.contract().is_some() {
            Decimal::ONE_HUNDRED
        } else {
            Decimal::ONE
        };
        Some(-side.sign() * qty * price * mult)
    }
}

/// Newer activities merged into older by id, newest first, at most `keep`.
pub fn merge_activities(
    old: &[Activity],
    fresh: impl IntoIterator<Item = Activity>,
    keep: usize,
) -> Vec<Activity> {
    let mut by_id: BTreeMap<String, Activity> =
        old.iter().map(|a| (a.id.clone(), a.clone())).collect();
    for a in fresh {
        by_id.insert(a.id.clone(), a);
    }
    let mut out: Vec<Activity> = by_id.into_values().collect();
    out.sort_by(|a, b| b.when().cmp(&a.when()).then_with(|| b.id.cmp(&a.id)));
    out.truncate(keep);
    out
}

// ------------------------------------------------------------- order events

/// An order's progress as the broker streams it (`new`, `fill`, `canceled`…).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrderEvent {
    pub event: String,
    pub time: Option<DateTime<Utc>>,
    pub order_id: String,
    pub client_order_id: Option<String>,
    pub symbol: String,
    pub side: Option<OrderSide>,
    /// `market`, `limit`, `stop`…
    pub order_type: Option<String>,
    pub qty: Option<Decimal>,
    pub notional: Option<Decimal>,
    pub limit_price: Option<Decimal>,
    pub filled_qty: Option<Decimal>,
    pub filled_avg_price: Option<Decimal>,
    pub status: Option<String>,
    /// This execution's price and size (fills).
    pub price: Option<Decimal>,
    pub fill_qty: Option<Decimal>,
    /// The position after the fill.
    pub position_qty: Option<Decimal>,
}

impl OrderEvent {
    pub fn is_fill(&self) -> bool {
        matches!(self.event.as_str(), "fill" | "partial_fill")
    }

    /// `Partial fill`, `Canceled`, `Done for day`.
    pub fn event_label(&self) -> String {
        let mut s = self.event.replace('_', " ");
        if let Some(first) = s.get(..1) {
            let up = first.to_ascii_uppercase();
            s.replace_range(..1, &up);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn stock(qty: &str, avg: &str, current: &str, lastday: &str) -> Position {
        let (q, a, c, l) = (d(qty), d(avg), d(current), d(lastday));
        Position {
            symbol: "XLU".into(),
            class: AssetClass::Equity,
            exchange: "ARCA".into(),
            qty: q,
            qty_available: Some(q),
            avg_entry_price: a,
            cost_basis: q * a,
            market_value: Some(q * c),
            current_price: Some(c),
            lastday_price: Some(l),
            change_today: Some(c / l - Decimal::ONE),
            unrealized_pl: Some(q * (c - a)),
            unrealized_plpc: None,
            unrealized_intraday_pl: Some(q * (c - l)),
            unrealized_intraday_plpc: None,
        }
    }

    #[test]
    fn marking_moves_pnl_by_the_price_change() {
        let p = stock("200", "43.10", "44.80", "44.16");
        let m = p.mark(None);
        assert!(!m.live);
        assert_eq!(m.market_value, d("8960.00"));
        assert_eq!(m.unrealized_pl, Some(d("340.00")));
        assert_eq!(m.day_pl, Some(d("128.00")));
        let m = p.mark(Some(d("45.00")));
        assert!(m.live);
        assert_eq!(m.delta, d("40.00"));
        assert_eq!(m.market_value, d("9000.00"));
        assert_eq!(m.unrealized_pl, Some(d("380.00")));
        assert_eq!(m.day_pl, Some(d("168.00")));
        // Against the previous close's value, 200 × 44.16.
        assert!((m.day_pct.unwrap() - 168.0 / 8832.0).abs() < 1e-12);

        // A short gains when the price falls.
        let s = stock("-50", "162.40", "156.95", "160.60");
        assert!(s.is_short());
        let m = s.mark(Some(d("155.95")));
        assert_eq!(m.delta, d("50.00"));
        assert_eq!(m.unrealized_pl, Some(d("322.50")));
        assert_eq!(m.day_pl, Some(d("232.50")));
        assert!(m.day_pct.unwrap() > 0.0);
        assert!(m.unrealized_pct.unwrap() > 0.0);
    }

    #[test]
    fn options_group_by_underlying_with_net_delta() {
        let mut call = stock("2", "1.35", "1.60", "1.50");
        call.symbol = "XLU261218C00045000".into();
        call.class = AssetClass::Option;
        call.market_value = Some(d("320"));
        assert_eq!(call.multiplier(), d("100"));
        assert_eq!(call.underlying(), "XLU");
        let m = call.mark(Some(d("1.70")));
        assert_eq!(m.delta, d("20.00"), "per contract, 100 shares");
        let positions = vec![stock("200", "43.10", "44.80", "44.16"), call.clone()];
        let groups = options_by_underlying(&positions);
        assert_eq!(groups.keys().collect::<Vec<_>>(), ["XLU"]);
        assert_eq!(groups["XLU"].len(), 1);
        assert_eq!(net_delta(d("200"), [(d("2"), Some(0.52))]), Some(304.0));
        // A short put has positive delta.
        let nd = net_delta(d("150"), [(d("-1"), Some(-0.30))]).unwrap();
        assert!((nd - 180.0).abs() < 1e-9);
        assert_eq!(net_delta(d("1"), [(d("1"), None)]), None);
    }

    #[test]
    fn account_figures() {
        let a = Account {
            number: "PA3TESTXYZ1234".into(),
            status: "ACTIVE".into(),
            equity: d("24000"),
            last_equity: d("23800"),
            maintenance_margin: Some(d("6000")),
            daytrade_count: 2,
            multiplier: Some(d("2")),
            ..Account::default()
        };
        assert_eq!(a.day_pl(), d("200"));
        assert_eq!(a.excess_equity(), Some(d("18000")));
        assert_eq!(a.margin_used(), Some(0.25));
        assert_eq!(a.day_trades_left(), Some(1));
        assert_eq!(a.margin_label(), "Reg T margin (2×)");
        assert!(a.restrictions().is_empty() && a.is_active());
        let rich = Account {
            equity: d("25000"),
            ..a.clone()
        };
        assert_eq!(rich.day_trades_left(), None, "no limit at $25,000");
        assert_eq!(mask_number("PA3TESTXYZ1234"), "••••1234");
        assert_eq!(mask_number("12"), "••••");
        assert_eq!(options_level_label(3), "Level 2, plus spreads");
    }

    #[test]
    fn history_change_and_drawdown() {
        let t = |m: i64| DateTime::from_timestamp(1_790_000_000 + m * 60, 0).unwrap();
        let h = PortfolioHistory {
            points: [100.0, 110.0, 99.0, 105.0]
                .iter()
                .enumerate()
                .map(|(i, e)| EquityPoint {
                    time: t(i as i64),
                    equity: *e,
                    profit_loss: e - 100.0,
                    profit_loss_pct: None,
                })
                .collect(),
            base_value: Some(100.0),
            base_as_of: None,
            timeframe: "1D".into(),
        };
        let (chg, pct) = h.change().unwrap();
        assert!((chg - 5.0).abs() < 1e-9 && (pct.unwrap() - 0.05).abs() < 1e-9);
        assert!((h.max_drawdown().unwrap() - (99.0 / 110.0 - 1.0)).abs() < 1e-12);
        assert_eq!(h.range(), Some((99.0, 110.0)));
        assert_eq!(PortfolioHistory::default().change(), None);
    }

    #[test]
    fn activities_categorise_and_merge() {
        assert_eq!(ActivityCategory::of("fill"), ActivityCategory::Fill);
        assert_eq!(ActivityCategory::of("DIVNRA"), ActivityCategory::Dividend);
        assert_eq!(ActivityCategory::of("DIVFEE"), ActivityCategory::Fee);
        assert_eq!(ActivityCategory::of("OPEXP"), ActivityCategory::Option);
        assert_eq!(ActivityCategory::of("CSD"), ActivityCategory::Transfer);
        assert_eq!(
            ActivityCategory::of("SPLIT"),
            ActivityCategory::CorporateAction
        );
        assert_eq!(ActivityCategory::of("XYZ"), ActivityCategory::Other);
        let fill = Activity {
            id: "20261002143100000::a".into(),
            code: "FILL".into(),
            time: Some("2026-10-02T18:31:00Z".parse().unwrap()),
            side: Some(OrderSide::Buy),
            qty: Some(d("60")),
            price: Some(d("70.05")),
            symbol: Some("CEG".into()),
            fill_type: Some("partial_fill".into()),
            ..Activity::default()
        };
        assert_eq!(fill.amount(), Some(d("-4203.00")));
        assert_eq!(fill.name(), "Partial fill");
        let opt = Activity {
            symbol: Some("XLU261218C00045000".into()),
            side: Some(OrderSide::Sell),
            qty: Some(d("1")),
            price: Some(d("1.50")),
            ..fill.clone()
        };
        assert_eq!(opt.amount(), Some(d("150.00")));
        let div = Activity {
            id: "20260930000000000::b".into(),
            code: "DIV".into(),
            date: Some(NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()),
            net_amount: Some(d("213.00")),
            ..Activity::default()
        };
        assert_eq!(div.amount(), Some(d("213.00")));
        let merged = merge_activities(std::slice::from_ref(&div), [fill.clone(), fill.clone()], 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, fill.id, "newest first");
        assert_eq!(merge_activities(&merged, [], 1).len(), 1);
    }

    #[test]
    fn order_events() {
        let e = OrderEvent {
            event: "partial_fill".into(),
            ..OrderEvent::default()
        };
        assert!(e.is_fill());
        assert_eq!(e.event_label(), "Partial fill");
        assert_eq!(OrderSide::parse("sell_short"), Some(OrderSide::Sell));
        assert_eq!(from_f64(82.505_000_1, 2), Some(d("82.51")));
        assert_eq!(from_f64(f64::NAN, 2), None);
    }
}
