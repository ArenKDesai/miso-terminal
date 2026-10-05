//! The paper account, read-only: balances and margin (`/v2/account`),
//! positions (`/v2/positions`), the equity curve
//! (`/v2/account/portfolio/history`), activities (`/v2/account/activities`)
//! and option snapshots for the greeks of option positions
//! (`/v1beta1/options/snapshots`). Parsers are pure and read fields loosely:
//! the trading API sends amounts as strings, which become exact decimals.
//!
//! Every query refreshes at least once a minute (the full re-sync), and the
//! trade-updates stream ([`crate::TradeStream`]) asks for an early one after
//! each order event and reconnect.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use mt_core::account::{
    Account, Activity, AssetClass, EquityPoint, OrderEvent, OrderSide, PortfolioHistory, Position,
    merge_activities,
};
use mt_core::equity::{Greeks, OptionSnapshot};
use mt_core::money::Decimal;
use mt_data::{FetchCtx, FetchError, Freshness, Query};
use serde_json::Value;

use crate::{Alpaca, parse, request};

/// Activities fetched per page (Alpaca's maximum).
const ACTIVITY_PAGE: usize = 100;
/// Pages followed on the first load.
const ACTIVITY_PAGES: usize = 3;
/// Activities kept per query.
const ACTIVITIES_KEEP: usize = 1000;
/// Option symbols per snapshot request.
const OPTION_CHUNK: usize = 100;

// ------------------------------------------------------------------ parsing

fn json(what: &str, body: &[u8]) -> Result<Value, FetchError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| FetchError::parse(what, e))?;
    if let (Some(msg), true) = (
        v.get("message").and_then(Value::as_str),
        v.get("code").is_some(),
    ) {
        return Err(FetchError::parse(what, format!("Alpaca said: {msg}")));
    }
    Ok(v)
}

/// A string or a number, exactly. `None` for blanks, nulls and junk.
pub fn decimal(v: &Value) -> Option<Decimal> {
    let s = match v {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if s.is_empty() {
        return None;
    }
    Decimal::from_str(&s)
        .or_else(|_| Decimal::from_scientific(&s))
        .ok()
}

fn dec(v: &Value, key: &str) -> Option<Decimal> {
    v.get(key).and_then(decimal)
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn int(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(parse::parse_time)
}

fn date(v: &Value, key: &str) -> Option<NaiveDate> {
    let s = v.get(key)?.as_str()?.trim();
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// `GET /v2/account`.
pub fn parse_account(body: &[u8]) -> Result<Account, FetchError> {
    let v = json("Alpaca account", body)?;
    let need =
        |k: &str| dec(&v, k).ok_or_else(|| FetchError::parse("Alpaca account", format!("no {k}")));
    Ok(Account {
        number: text(&v, "account_number").unwrap_or_default(),
        status: text(&v, "status").unwrap_or_default(),
        currency: text(&v, "currency").unwrap_or_else(|| "USD".into()),
        created: time(&v, "created_at"),
        cash: need("cash")?,
        equity: need("equity")?,
        last_equity: dec(&v, "last_equity").unwrap_or_default(),
        buying_power: dec(&v, "buying_power").unwrap_or_default(),
        regt_buying_power: dec(&v, "regt_buying_power"),
        daytrading_buying_power: dec(&v, "daytrading_buying_power"),
        non_marginable_buying_power: dec(&v, "non_marginable_buying_power"),
        options_buying_power: dec(&v, "options_buying_power"),
        long_market_value: dec(&v, "long_market_value").unwrap_or_default(),
        short_market_value: dec(&v, "short_market_value").unwrap_or_default(),
        initial_margin: dec(&v, "initial_margin"),
        maintenance_margin: dec(&v, "maintenance_margin"),
        last_maintenance_margin: dec(&v, "last_maintenance_margin"),
        sma: dec(&v, "sma"),
        multiplier: dec(&v, "multiplier"),
        accrued_fees: dec(&v, "accrued_fees"),
        pending_transfer_in: dec(&v, "pending_transfer_in"),
        pending_transfer_out: dec(&v, "pending_transfer_out"),
        pattern_day_trader: flag(&v, "pattern_day_trader"),
        daytrade_count: int(&v, "daytrade_count").unwrap_or(0) as u32,
        trading_blocked: flag(&v, "trading_blocked"),
        transfers_blocked: flag(&v, "transfers_blocked"),
        account_blocked: flag(&v, "account_blocked"),
        trade_suspended_by_user: flag(&v, "trade_suspended_by_user"),
        shorting_enabled: flag(&v, "shorting_enabled"),
        options_approved_level: int(&v, "options_approved_level").map(|l| l.min(9) as u8),
        options_trading_level: int(&v, "options_trading_level").map(|l| l.min(9) as u8),
    })
}

fn position(v: &Value) -> Option<Position> {
    let symbol = text(v, "symbol")?.to_ascii_uppercase();
    let mut qty = dec(v, "qty")?;
    // Shorts come with a negative quantity; make sure of it.
    if text(v, "side").as_deref() == Some("short") && qty > Decimal::ZERO {
        qty = -qty;
    }
    let avg = dec(v, "avg_entry_price").unwrap_or_default();
    Some(Position {
        class: AssetClass::parse(&text(v, "asset_class").unwrap_or_default()),
        exchange: text(v, "exchange").unwrap_or_default(),
        qty,
        qty_available: dec(v, "qty_available"),
        avg_entry_price: avg,
        cost_basis: dec(v, "cost_basis").unwrap_or(avg * qty),
        market_value: dec(v, "market_value"),
        current_price: dec(v, "current_price"),
        lastday_price: dec(v, "lastday_price"),
        change_today: dec(v, "change_today"),
        unrealized_pl: dec(v, "unrealized_pl"),
        unrealized_plpc: dec(v, "unrealized_plpc"),
        unrealized_intraday_pl: dec(v, "unrealized_intraday_pl"),
        unrealized_intraday_plpc: dec(v, "unrealized_intraday_plpc"),
        symbol,
    })
}

/// `GET /v2/positions`: open positions, largest first.
pub fn parse_positions(body: &[u8]) -> Result<Vec<Position>, FetchError> {
    let v = json("Alpaca positions", body)?;
    let list = v
        .as_array()
        .ok_or_else(|| FetchError::parse("Alpaca positions", "not a list"))?;
    let mut out: Vec<Position> = list.iter().filter_map(position).collect();
    out.sort_by(|a, b| {
        let value = |p: &Position| p.market_value.unwrap_or_default().abs();
        value(b)
            .cmp(&value(a))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    Ok(out)
}

/// `GET /v2/account/portfolio/history`. Points with no equity (before the
/// account existed, or missing data) are left out.
pub fn parse_portfolio_history(body: &[u8]) -> Result<PortfolioHistory, FetchError> {
    let v = json("Alpaca portfolio history", body)?;
    let list = |k: &str| {
        v.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let stamps = v
        .get("timestamp")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::parse("Alpaca portfolio history", "no timestamps"))?;
    let (equity, pl, pct) = (list("equity"), list("profit_loss"), list("profit_loss_pct"));
    let num = |a: &[Value], i: usize| {
        a.get(i)
            .and_then(|x| {
                x.as_f64()
                    .or_else(|| decimal(x).map(mt_core::account::to_f64))
            })
            .filter(|x| x.is_finite())
    };
    let points = stamps
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let time = DateTime::from_timestamp(t.as_i64()?, 0)?;
            let e = num(&equity, i)?;
            Some(EquityPoint {
                time,
                equity: e,
                profit_loss: num(&pl, i).unwrap_or(0.0),
                profit_loss_pct: num(&pct, i),
            })
        })
        .collect();
    Ok(PortfolioHistory {
        points,
        base_value: v.get("base_value").and_then(|b| {
            b.as_f64()
                .or_else(|| decimal(b).map(mt_core::account::to_f64))
        }),
        base_as_of: date(&v, "base_value_asof"),
        timeframe: text(&v, "timeframe").unwrap_or_default(),
    })
}

fn activity(v: &Value) -> Option<Activity> {
    let code = text(v, "activity_type")?.to_ascii_uppercase();
    Some(Activity {
        id: text(v, "id")?,
        sub_code: text(v, "activity_sub_type"),
        time: time(v, "transaction_time").or_else(|| time(v, "created_at")),
        date: date(v, "date").or_else(|| date(v, "transaction_time")),
        symbol: text(v, "symbol").map(|s| s.to_ascii_uppercase()),
        side: text(v, "side").and_then(|s| OrderSide::parse(&s)),
        qty: dec(v, "qty"),
        price: dec(v, "price"),
        net_amount: dec(v, "net_amount"),
        per_share_amount: dec(v, "per_share_amount"),
        description: text(v, "description").unwrap_or_default(),
        status: text(v, "status"),
        order_id: text(v, "order_id"),
        order_status: text(v, "order_status"),
        leaves_qty: dec(v, "leaves_qty"),
        cum_qty: dec(v, "cum_qty"),
        fill_type: text(v, "type"),
        code,
    })
}

/// `GET /v2/account/activities`: newest first.
pub fn parse_activities(body: &[u8]) -> Result<Vec<Activity>, FetchError> {
    let v = json("Alpaca activities", body)?;
    let list = v
        .as_array()
        .ok_or_else(|| FetchError::parse("Alpaca activities", "not a list"))?;
    Ok(merge_activities(
        &[],
        list.iter().filter_map(activity),
        usize::MAX,
    ))
}

/// `GET /v1beta1/options/snapshots`: per OCC symbol, with the next page's token.
pub fn parse_option_snapshots(
    body: &[u8],
) -> Result<(BTreeMap<String, OptionSnapshot>, Option<String>), FetchError> {
    let v = json("Alpaca option snapshots", body)?;
    let map = v.get("snapshots").unwrap_or(&v);
    let obj = map
        .as_object()
        .ok_or_else(|| FetchError::parse("Alpaca option snapshots", "not an object"))?;
    let snaps = obj
        .iter()
        .filter(|(_, s)| s.is_object())
        .map(|(sym, s)| {
            let part = |k: &str| s.get(k).filter(|p| p.is_object());
            let g = part("greeks");
            let greek = |k: &str| g.and_then(|g| g.get(k)).and_then(Value::as_f64);
            let snap = OptionSnapshot {
                latest_trade: part("latestTrade").and_then(parse::trade),
                latest_quote: part("latestQuote").and_then(parse::quote),
                greeks: Greeks {
                    delta: greek("delta"),
                    gamma: greek("gamma"),
                    theta: greek("theta"),
                    vega: greek("vega"),
                    rho: greek("rho"),
                },
                implied_volatility: s.get("impliedVolatility").and_then(Value::as_f64),
            };
            (sym.trim().to_ascii_uppercase(), snap)
        })
        .collect();
    Ok((snaps, text(&v, "next_page_token")))
}

/// One `trade_updates` message's `data`.
pub fn order_event(data: &Value) -> Option<OrderEvent> {
    let order = data.get("order")?;
    Some(OrderEvent {
        event: text(data, "event")?,
        time: time(data, "timestamp").or_else(|| time(order, "updated_at")),
        order_id: text(order, "id").unwrap_or_default(),
        client_order_id: text(order, "client_order_id"),
        symbol: text(order, "symbol")
            .unwrap_or_default()
            .to_ascii_uppercase(),
        side: text(order, "side").and_then(|s| OrderSide::parse(&s)),
        order_type: text(order, "type").or_else(|| text(order, "order_type")),
        qty: dec(order, "qty"),
        notional: dec(order, "notional"),
        limit_price: dec(order, "limit_price"),
        filled_qty: dec(order, "filled_qty"),
        filled_avg_price: dec(order, "filled_avg_price"),
        status: text(order, "status"),
        price: dec(data, "price"),
        fill_qty: dec(data, "qty"),
        position_qty: dec(data, "position_qty"),
    })
}

// ------------------------------------------------------------------ queries

fn trading_hours() -> bool {
    mt_core::exchange::market_status(mt_core::time::now_utc(), &[]).session
        != mt_core::exchange::Session::Closed
}

/// A minute while the market trades (the full re-sync), less often when closed.
fn every_minute_while_trading(closed_secs: u64) -> Freshness {
    Freshness::Every(Duration::from_secs(if trading_hours() {
        60
    } else {
        closed_secs
    }))
}

#[derive(Clone, Debug)]
pub struct AccountQuery {
    alpaca: Alpaca,
}

impl AccountQuery {
    pub(crate) fn new(alpaca: Alpaca) -> Self {
        Self { alpaca }
    }
}

impl Query for AccountQuery {
    type Output = Account;

    fn key(&self) -> String {
        format!("alpaca/{}/account", self.alpaca.mode().key())
    }

    fn label(&self) -> String {
        format!("Alpaca {} account", self.alpaca.mode().name())
    }

    fn freshness(&self, _: &Account) -> Freshness {
        every_minute_while_trading(5 * 60)
    }

    async fn fetch(&self, ctx: FetchCtx, _: Option<Arc<Account>>) -> Result<Account, FetchError> {
        let url = format!("{}/v2/account", self.alpaca.endpoints.trading);
        parse_account(&ctx.get(request(&ctx, url)?).await?)
    }
}

#[derive(Clone, Debug)]
pub struct PositionsQuery {
    alpaca: Alpaca,
}

impl PositionsQuery {
    pub(crate) fn new(alpaca: Alpaca) -> Self {
        Self { alpaca }
    }
}

impl Query for PositionsQuery {
    type Output = Vec<Position>;

    fn key(&self) -> String {
        format!("alpaca/{}/positions", self.alpaca.mode().key())
    }

    fn label(&self) -> String {
        format!("Alpaca {} positions", self.alpaca.mode().name())
    }

    fn freshness(&self, _: &Vec<Position>) -> Freshness {
        every_minute_while_trading(5 * 60)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _: Option<Arc<Vec<Position>>>,
    ) -> Result<Vec<Position>, FetchError> {
        let url = format!("{}/v2/positions", self.alpaca.endpoints.trading);
        parse_positions(&ctx.get(request(&ctx, url)?).await?)
    }
}

/// The equity curve's span.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HistoryPeriod {
    /// The latest session at five minutes.
    #[default]
    Day,
    /// A week, hourly.
    Week,
    Month,
    Quarter,
    Year,
}

impl HistoryPeriod {
    pub const ALL: [Self; 5] = [
        Self::Day,
        Self::Week,
        Self::Month,
        Self::Quarter,
        Self::Year,
    ];

    /// As the command line and the buttons write it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "1D",
            Self::Week => "1W",
            Self::Month => "1M",
            Self::Quarter => "3M",
            Self::Year => "1Y",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "1D" | "D" | "DAY" | "TODAY" => Some(Self::Day),
            "1W" | "W" | "5D" | "WEEK" => Some(Self::Week),
            "1M" | "M" | "MONTH" => Some(Self::Month),
            "3M" | "Q" | "QUARTER" => Some(Self::Quarter),
            "1Y" | "1A" | "Y" | "YEAR" | "12M" => Some(Self::Year),
            _ => None,
        }
    }

    /// Alpaca's `period`.
    fn param(self) -> &'static str {
        match self {
            Self::Day => "1D",
            Self::Week => "1W",
            Self::Month => "1M",
            Self::Quarter => "3M",
            Self::Year => "1A",
        }
    }

    /// Alpaca's `timeframe` (longer than 30 days takes only daily points).
    fn timeframe(self) -> &'static str {
        match self {
            Self::Day => "5Min",
            Self::Week => "1H",
            Self::Month | Self::Quarter | Self::Year => "1D",
        }
    }

    pub fn is_intraday(self) -> bool {
        matches!(self, Self::Day | Self::Week)
    }
}

#[derive(Clone, Debug)]
pub struct PortfolioHistoryQuery {
    alpaca: Alpaca,
    period: HistoryPeriod,
}

impl PortfolioHistoryQuery {
    pub(crate) fn new(alpaca: Alpaca, period: HistoryPeriod) -> Self {
        Self { alpaca, period }
    }

    pub fn url(&self) -> String {
        // The period goes first so a recording can be picked by it
        // (`history@1D.json`; see FixtureTransport). P&L runs from the
        // period's start rather than resetting each day.
        let mut url = format!(
            "{}/v2/account/portfolio/history?period={}&timeframe={}",
            self.alpaca.endpoints.trading,
            self.period.param(),
            self.period.timeframe()
        );
        if self.period.is_intraday() {
            url.push_str("&intraday_reporting=market_hours&pnl_reset=no_reset");
        }
        url
    }
}

impl Query for PortfolioHistoryQuery {
    type Output = PortfolioHistory;

    fn key(&self) -> String {
        format!(
            "alpaca/{}/history/{}",
            self.alpaca.mode().key(),
            self.period.label()
        )
    }

    fn label(&self) -> String {
        format!(
            "Alpaca {} equity history · {}",
            self.alpaca.mode().name(),
            self.period.label()
        )
    }

    fn freshness(&self, _: &PortfolioHistory) -> Freshness {
        match self.period {
            HistoryPeriod::Day => every_minute_while_trading(15 * 60),
            HistoryPeriod::Week => Freshness::Every(Duration::from_secs(15 * 60)),
            _ => Freshness::Every(Duration::from_secs(60 * 60)),
        }
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _: Option<Arc<PortfolioHistory>>,
    ) -> Result<PortfolioHistory, FetchError> {
        parse_portfolio_history(&ctx.get(request(&ctx, self.url())?).await?)
    }
}

/// The account's activities, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activities {
    pub items: Vec<Activity>,
    /// Older activities exist beyond what was fetched.
    pub more: bool,
}

#[derive(Clone, Debug)]
pub struct ActivitiesQuery {
    alpaca: Alpaca,
}

impl ActivitiesQuery {
    pub(crate) fn new(alpaca: Alpaca) -> Self {
        Self { alpaca }
    }

    fn url(&self, token: Option<&str>) -> String {
        let mut url = format!(
            "{}/v2/account/activities?direction=desc&page_size={ACTIVITY_PAGE}",
            self.alpaca.endpoints.trading
        );
        if let Some(t) = token {
            url.push_str("&page_token=");
            url.push_str(t);
        }
        url
    }
}

impl Query for ActivitiesQuery {
    type Output = Activities;

    fn key(&self) -> String {
        format!("alpaca/{}/activities", self.alpaca.mode().key())
    }

    fn label(&self) -> String {
        format!("Alpaca {} activities", self.alpaca.mode().name())
    }

    fn freshness(&self, _: &Activities) -> Freshness {
        Freshness::Every(Duration::from_secs(if trading_hours() { 120 } else { 600 }))
    }

    /// The newest page, merged into what was there; on the first load, a few
    /// pages back.
    async fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<Activities>>,
    ) -> Result<Activities, FetchError> {
        let pages = if prev.is_some() { 1 } else { ACTIVITY_PAGES };
        let mut seen: HashSet<String> = HashSet::new();
        let mut fresh = Vec::new();
        let mut token: Option<String> = None;
        let mut more = false;
        for _ in 0..pages {
            let body = ctx.get(request(&ctx, self.url(token.as_deref()))?).await?;
            let page = parse_activities(&body)?;
            let full = page.len() >= ACTIVITY_PAGE;
            let before = seen.len();
            seen.extend(page.iter().map(|a| a.id.clone()));
            // A page with nothing new (a replay serves the same one) ends it.
            if seen.len() == before {
                break;
            }
            token = page.last().map(|a| a.id.clone());
            fresh.extend(page);
            more = full;
            if !full {
                break;
            }
        }
        let old = prev.as_deref().map_or(&[][..], |p| p.items.as_slice());
        Ok(Activities {
            items: merge_activities(old, fresh, ACTIVITIES_KEEP),
            more: more || prev.is_some_and(|p| p.more),
        })
    }
}

/// Option snapshots by OCC symbol.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionSnapshots {
    pub by_symbol: BTreeMap<String, OptionSnapshot>,
}

impl OptionSnapshots {
    pub fn get(&self, symbol: &str) -> Option<&OptionSnapshot> {
        self.by_symbol.get(symbol)
    }
}

/// Greeks and quotes for the options an account holds. On the free plan the
/// prices are Alpaca's indicative feed, not OPRA.
#[derive(Clone, Debug)]
pub struct OptionSnapshotsQuery {
    alpaca: Alpaca,
    symbols: Vec<String>,
}

/// What the free plan's option prices are called beside each figure.
pub const OPTION_FEED_LABEL: &str = "indicative";

impl OptionSnapshotsQuery {
    pub(crate) fn new<S: AsRef<str>>(alpaca: Alpaca, symbols: impl IntoIterator<Item = S>) -> Self {
        let mut symbols: Vec<String> = symbols
            .into_iter()
            .map(|s| s.as_ref().trim().to_ascii_uppercase())
            .filter(|s| mt_core::instrument::OptionContract::parse_occ(s).is_some())
            .collect();
        symbols.sort();
        symbols.dedup();
        Self { alpaca, symbols }
    }

    pub fn symbols(&self) -> &[String] {
        &self.symbols
    }
}

impl Query for OptionSnapshotsQuery {
    type Output = OptionSnapshots;

    fn key(&self) -> String {
        format!("alpaca/options/snapshots/{}", self.symbols.join(","))
    }

    fn label(&self) -> String {
        format!("Alpaca option snapshots · {} contracts", self.symbols.len())
    }

    fn freshness(&self, _: &OptionSnapshots) -> Freshness {
        every_minute_while_trading(10 * 60)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _: Option<Arc<OptionSnapshots>>,
    ) -> Result<OptionSnapshots, FetchError> {
        let wanted: HashSet<&str> = self.symbols.iter().map(String::as_str).collect();
        let mut out = OptionSnapshots::default();
        for chunk in self.symbols.chunks(OPTION_CHUNK) {
            let url = format!(
                "{}/v1beta1/options/snapshots?symbols={}&feed=indicative",
                self.alpaca.endpoints.data,
                chunk.join(",")
            );
            let (snaps, _) = parse_option_snapshots(&ctx.get(request(&ctx, url)?).await?)?;
            out.by_symbol.extend(
                snaps
                    .into_iter()
                    .filter(|(s, _)| wanted.contains(s.as_str())),
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn account_amounts_are_exact() {
        let a = parse_account(
            br#"{"id":"x","account_number":"PA3TEST00001","status":"ACTIVE","currency":"USD",
                "cash":"35281.26","equity":"100512.79","last_equity":"100110.31","buying_power":"170846.68",
                "regt_buying_power":"170846.68","daytrading_buying_power":"0","non_marginable_buying_power":"85423.34",
                "options_buying_power":"85423.34","long_market_value":"35310.28","short_market_value":"-7847.50",
                "initial_margin":"21578.89","maintenance_margin":"12947.33","last_maintenance_margin":"12901.07",
                "sma":"0","multiplier":"2","pattern_day_trader":false,"daytrade_count":1,"trading_blocked":false,
                "transfers_blocked":false,"account_blocked":false,"trade_suspended_by_user":false,"shorting_enabled":true,
                "options_approved_level":3,"options_trading_level":3,"created_at":"2026-04-01T14:02:11.12Z"}"#,
        )
        .unwrap();
        assert_eq!(a.equity, d("100512.79"));
        assert_eq!(a.day_pl(), d("402.48"));
        assert_eq!(a.short_market_value, d("-7847.50"));
        assert_eq!(a.multiplier, Some(d("2")));
        assert_eq!(a.options_trading_level, Some(3));
        assert!(a.shorting_enabled && !a.pattern_day_trader);
        assert!(matches!(
            parse_account(br#"{"code":40110000,"message":"request is not authorized"}"#),
            Err(FetchError::Parse { detail, .. }) if detail.contains("not authorized")
        ));
        assert!(
            parse_account(br#"{"status":"ACTIVE"}"#).is_err(),
            "no equity"
        );
    }

    #[test]
    fn positions_shorts_and_options() {
        let list = parse_positions(
            br#"[{"asset_id":"a","symbol":"UNG","exchange":"ARCA","asset_class":"us_equity","qty":"50","side":"short",
                   "avg_entry_price":"162.4","market_value":"-7847.5","cost_basis":"-8120","current_price":"156.95",
                   "lastday_price":"160.6","unrealized_pl":"272.5","unrealized_intraday_pl":"182.5"},
                  {"symbol":"XLU261218C00045000","asset_class":"us_option","qty":"2","side":"long","avg_entry_price":"1.35",
                   "market_value":"320","cost_basis":"270","current_price":"1.6"},
                  {"symbol":"XLU","asset_class":"us_equity","qty":"200","side":"long","avg_entry_price":"43.1",
                   "market_value":"8960","cost_basis":"8620","current_price":"44.8"},
                  {"no_symbol":true}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].symbol, "XLU", "largest first");
        let ung = list.iter().find(|p| p.symbol == "UNG").unwrap();
        assert_eq!(ung.qty, d("-50"), "a short's quantity is negative");
        let call = list.iter().find(|p| p.is_option()).unwrap();
        assert_eq!(call.contract().unwrap().underlying, "XLU");
        assert_eq!(call.class, AssetClass::Option);
    }

    #[test]
    fn history_activities_and_options() {
        let h = parse_portfolio_history(
            br#"{"timestamp":[1790947800,1790948100,1790948400],"equity":[null,100200.5,100300],
                "profit_loss":[0,90.19,189.69],"profit_loss_pct":[0,0.0009,null],"base_value":100110.31,
                "base_value_asof":"2026-10-01","timeframe":"5Min"}"#,
        )
        .unwrap();
        assert_eq!(h.points.len(), 2, "no equity, no point");
        assert_eq!(h.points[1].profit_loss_pct, None);
        assert_eq!(h.base_as_of, NaiveDate::from_ymd_opt(2026, 10, 1));
        assert!((h.change().unwrap().0 - 189.69).abs() < 1e-6);

        let acts = parse_activities(
            br#"[{"id":"20260930000000000::d","activity_type":"DIV","date":"2026-09-30","net_amount":"213","symbol":"AEE",
                   "qty":"300","per_share_amount":"0.71","status":"executed"},
                  {"id":"20261002143100000::f","activity_type":"FILL","transaction_time":"2026-10-02T18:31:00.12Z",
                   "type":"fill","price":"70.05","qty":"60","side":"buy","symbol":"CEG","leaves_qty":"0",
                   "order_id":"o1","cum_qty":"60","order_status":"filled"},
                  {"activity_type":"FILL"}]"#,
        )
        .unwrap();
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0].code, "FILL", "newest first");
        assert_eq!(acts[0].amount(), Some(d("-4203.00")));
        assert_eq!(acts[1].per_share_amount, Some(d("0.71")));

        let (snaps, token) = parse_option_snapshots(
            br#"{"snapshots":{"XLU261218C00045000":{"latestQuote":{"t":"2026-10-02T19:59:00Z","bp":1.55,"ap":1.65,"bs":5,"as":7},
                 "greeks":{"delta":0.52,"gamma":0.08,"theta":-0.01,"vega":0.07,"rho":0.02},"impliedVolatility":0.21}},
                 "next_page_token":null}"#,
        )
        .unwrap();
        assert!(token.is_none());
        let s = &snaps["XLU261218C00045000"];
        assert_eq!(s.greeks.delta, Some(0.52));
        assert!((s.mark().unwrap() - 1.60).abs() < 1e-9);
    }

    #[test]
    fn order_events_read_the_order() {
        let v: Value = serde_json::from_str(
            r#"{"event":"partial_fill","timestamp":"2026-10-02T18:31:00Z","price":"70.05","qty":"20","position_qty":"20",
                "order":{"id":"o1","client_order_id":"c1","symbol":"ceg","side":"buy","type":"limit","qty":"60",
                "filled_qty":"20","filled_avg_price":"70.05","limit_price":"70.10","status":"partially_filled"}}"#,
        )
        .unwrap();
        let e = order_event(&v).unwrap();
        assert!(e.is_fill());
        assert_eq!(e.symbol, "CEG");
        assert_eq!(e.fill_qty, Some(d("20")));
        assert_eq!(e.limit_price, Some(d("70.10")));
        assert_eq!(decimal(&serde_json::json!(1e-5)), Some(d("0.00001")));
        assert_eq!(decimal(&serde_json::json!("")), None);
    }

    #[test]
    fn history_urls_pick_a_recording_by_period() {
        let q = Alpaca::default().portfolio_history(HistoryPeriod::Year);
        assert!(
            q.url().contains("history?period=1A&timeframe=1D"),
            "{}",
            q.url()
        );
        let day = Alpaca::default().portfolio_history(HistoryPeriod::Day);
        assert!(day.url().ends_with("pnl_reset=no_reset"));
        assert_ne!(q.key(), day.key());
        assert_eq!(HistoryPeriod::parse("1a"), Some(HistoryPeriod::Year));
    }
}
