//! Options data: the contracts listed on an underlying
//! (`/v2/options/contracts`, on the trading API) and one expiry's chain of
//! quotes, greeks and implied volatility
//! (`/v1beta1/options/snapshots/{underlying}`, on the market-data API).
//!
//! On the free plan the chain comes from Alpaca's indicative feed: quotes
//! derived from OPRA's and sampled at most once a second, trades fifteen
//! minutes late. Every figure shown from it says so ([`OPTION_FEED_LABEL`]).
//! Alpaca streams options only in MessagePack, so chains are re-read every
//! minute instead of streamed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use mt_core::equity::OptionSnapshot;
use mt_core::instrument::OptionContract;
use mt_core::options::{ContractInfo, ContractList, MULTIPLIER, Style};
use mt_data::{FetchCtx, FetchError, Freshness, Query};
use serde_json::Value;

use crate::account::{decimal, every_minute_while_trading, parse_option_snapshots};
use crate::{Alpaca, OPTION_FEED_LABEL, request};

/// Contracts per page (Alpaca's maximum).
const CONTRACTS_PAGE: u32 = 10_000;
/// Snapshots per page (Alpaca's maximum).
const CHAIN_PAGE: u32 = 1_000;
/// Pages followed before giving up (and saying so).
const MAX_PAGES: usize = 10;
/// How far ahead the contract list reaches (LEAPS run about three years).
const HORIZON_DAYS: i64 = 3 * 366;

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn date(v: &Value, key: &str) -> Option<NaiveDate> {
    let s = v.get(key)?.as_str()?.trim();
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

fn flag(v: &Value, key: &str) -> Option<bool> {
    match v.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(s.eq_ignore_ascii_case("true")),
        _ => None,
    }
}

fn contract(v: &Value) -> Option<ContractInfo> {
    let symbol = text(v, "symbol")?.to_ascii_uppercase();
    let parsed = OptionContract::parse_occ(&symbol)?;
    let dec = |k: &str| v.get(k).and_then(decimal);
    Some(ContractInfo {
        root: text(v, "root_symbol")
            .map(|r| r.to_ascii_uppercase())
            .unwrap_or_else(|| parsed.underlying.clone()),
        underlying: text(v, "underlying_symbol")
            .map(|u| u.to_ascii_uppercase())
            .unwrap_or_else(|| parsed.underlying.clone()),
        name: text(v, "name").unwrap_or_default(),
        tradable: flag(v, "tradable").unwrap_or(false),
        active: text(v, "status").is_none_or(|s| s.eq_ignore_ascii_case("active")),
        style: Style::parse(&text(v, "style").unwrap_or_default()),
        multiplier: dec("multiplier").unwrap_or(MULTIPLIER),
        size: dec("size").unwrap_or(MULTIPLIER),
        open_interest: dec("open_interest").and_then(count),
        open_interest_date: date(v, "open_interest_date"),
        close_price: dec("close_price"),
        close_price_date: date(v, "close_price_date"),
        penny: flag(v, "ppind").unwrap_or(false),
        contract: parsed,
        symbol,
    })
}

/// A count sent as a decimal string (`"1234"`, `"1234.0"`).
fn count(d: mt_core::money::Decimal) -> Option<u64> {
    if d.is_sign_negative() {
        return None;
    }
    d.trunc().normalize().to_string().parse().ok()
}

/// `GET /v2/options/contracts`: the page's contracts and the next page's token.
pub fn parse_contracts(body: &[u8]) -> Result<(Vec<ContractInfo>, Option<String>), FetchError> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| FetchError::parse("Alpaca option contracts", e))?;
    if let (Some(msg), true) = (
        v.get("message").and_then(Value::as_str),
        v.get("code").is_some(),
    ) {
        return Err(FetchError::parse(
            "Alpaca option contracts",
            format!("Alpaca said: {msg}"),
        ));
    }
    let list = v
        .get("option_contracts")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::parse("Alpaca option contracts", "no option_contracts"))?;
    Ok((
        list.iter().filter_map(contract).collect(),
        text(&v, "next_page_token"),
    ))
}

/// Every active contract on one underlying expiring from today on, with its
/// open interest and the latest close. Refreshed hourly (open interest and
/// closes change once a day; new strikes and expiries appear overnight).
#[derive(Clone, Debug)]
pub struct OptionContractsQuery {
    alpaca: Alpaca,
    underlying: String,
}

impl OptionContractsQuery {
    pub(crate) fn new(alpaca: Alpaca, underlying: &str) -> Self {
        Self {
            alpaca,
            underlying: underlying.trim().to_ascii_uppercase(),
        }
    }

    pub fn underlying(&self) -> &str {
        &self.underlying
    }

    /// The underlying goes first so a recording can be picked by it
    /// (`contracts@XLU.json`; see `FixtureTransport`). Without an end date
    /// Alpaca lists only the contracts expiring by the next weekend.
    pub fn url(&self, token: Option<&str>) -> String {
        let today = mt_core::exchange::now_exchange().date_naive();
        let mut url = format!(
            "{}/v2/options/contracts?underlying_symbols={}&status=active&expiration_date_gte={today}\
             &expiration_date_lte={}&limit={CONTRACTS_PAGE}",
            self.alpaca.endpoints.trading,
            self.underlying,
            today + chrono::Duration::days(HORIZON_DAYS),
        );
        if let Some(t) = token {
            url.push_str("&page_token=");
            url.push_str(t);
        }
        url
    }
}

impl Query for OptionContractsQuery {
    type Output = ContractList;

    fn key(&self) -> String {
        format!("alpaca/options/contracts/{}", self.underlying)
    }

    fn label(&self) -> String {
        format!("Alpaca option contracts · {}", self.underlying)
    }

    fn freshness(&self, _: &ContractList) -> Freshness {
        Freshness::Every(Duration::from_secs(60 * 60))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _: Option<Arc<ContractList>>,
    ) -> Result<ContractList, FetchError> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let body = ctx.get(request(&ctx, self.url(token.as_deref()))?).await?;
            let (page, next) = parse_contracts(&body)?;
            let empty = page.is_empty();
            all.extend(page);
            // A replay serves the same page again: stop on a repeat token.
            match next {
                Some(n) if !empty && token.as_deref() != Some(n.as_str()) => token = Some(n),
                _ => return Ok(ContractList::new(&self.underlying, all)),
            }
        }
        Err(FetchError::parse(
            "Alpaca option contracts",
            format!("more than {MAX_PAGES} pages for {}", self.underlying),
        ))
    }
}

/// One expiry of a chain: each contract's quote, latest trade, daily bars,
/// greeks and implied volatility, by OCC symbol.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionChain {
    pub underlying: String,
    pub expiry: Option<NaiveDate>,
    pub by_symbol: BTreeMap<String, OptionSnapshot>,
}

impl OptionChain {
    pub fn get(&self, symbol: &str) -> Option<&OptionSnapshot> {
        self.by_symbol.get(symbol)
    }

    /// Where the prices come from, for labels.
    pub fn feed_label(&self) -> &'static str {
        OPTION_FEED_LABEL
    }
}

/// The chain for one underlying and expiry, from the indicative feed,
/// re-read every minute while the market trades.
#[derive(Clone, Debug)]
pub struct OptionChainQuery {
    alpaca: Alpaca,
    underlying: String,
    expiry: NaiveDate,
}

impl OptionChainQuery {
    pub(crate) fn new(alpaca: Alpaca, underlying: &str, expiry: NaiveDate) -> Self {
        Self {
            alpaca,
            underlying: underlying.trim().to_ascii_uppercase(),
            expiry,
        }
    }

    /// The expiry goes first so a recording can be picked by it
    /// (`XLU@2026-12-18.json`).
    pub fn url(&self, token: Option<&str>) -> String {
        let mut url = format!(
            "{}/v1beta1/options/snapshots/{}?expiration_date={}&feed=indicative&limit={CHAIN_PAGE}",
            self.alpaca.endpoints.data, self.underlying, self.expiry
        );
        if let Some(t) = token {
            url.push_str("&page_token=");
            url.push_str(t);
        }
        url
    }
}

impl Query for OptionChainQuery {
    type Output = OptionChain;

    fn key(&self) -> String {
        format!("alpaca/options/chain/{}/{}", self.underlying, self.expiry)
    }

    fn label(&self) -> String {
        format!(
            "Alpaca option chain · {} {}",
            self.underlying,
            self.expiry.format("%b %d %Y")
        )
    }

    fn freshness(&self, _: &OptionChain) -> Freshness {
        every_minute_while_trading(10 * 60)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _: Option<Arc<OptionChain>>,
    ) -> Result<OptionChain, FetchError> {
        let mut out = OptionChain {
            underlying: self.underlying.clone(),
            expiry: Some(self.expiry),
            by_symbol: BTreeMap::new(),
        };
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let body = ctx.get(request(&ctx, self.url(token.as_deref()))?).await?;
            let (page, next) = parse_option_snapshots(&body)?;
            let before = out.by_symbol.len();
            out.by_symbol.extend(page.into_iter().filter(|(s, _)| {
                OptionContract::parse_occ(s).is_some_and(|c| c.expiry == self.expiry)
            }));
            match next {
                Some(n) if out.by_symbol.len() > before && token.as_deref() != Some(n.as_str()) => {
                    token = Some(n);
                }
                _ => return Ok(out),
            }
        }
        Err(FetchError::parse(
            "Alpaca option chain",
            format!("more than {MAX_PAGES} pages for {}", self.underlying),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use mt_core::money::Decimal;

    #[test]
    fn contracts_parse_leniently() {
        let (list, token) = parse_contracts(
            br#"{"option_contracts":[
                {"id":"a","symbol":"XLU261218C00045000","name":"XLU Dec 18 2026 45 Call","status":"active",
                 "tradable":true,"expiration_date":"2026-12-18","root_symbol":"XLU","underlying_symbol":"XLU",
                 "type":"call","style":"american","strike_price":"45","multiplier":"100","size":"100",
                 "open_interest":"1234","open_interest_date":"2026-10-01","close_price":"1.28",
                 "close_price_date":"2026-10-01","ppind":true},
                {"symbol":"XLU1261218C00045000","root_symbol":"XLU1","underlying_symbol":"XLU","tradable":true,
                 "multiplier":"100","size":"150","open_interest":null},
                {"symbol":"not-an-option"}],
              "next_page_token":"abc"}"#,
        )
        .unwrap();
        assert_eq!(token.as_deref(), Some("abc"));
        assert_eq!(list.len(), 2);
        let c = &list[0];
        assert_eq!(c.open_interest, Some(1234));
        assert_eq!(c.close_price, Some(Decimal::from_str("1.28").unwrap()));
        assert!(c.penny && c.tradable && c.active && c.is_standard());
        assert_eq!(c.contract.strike, Decimal::from(45));
        assert!(!list[1].is_standard(), "150 shares under another root");
        assert_eq!(list[1].open_interest, None);
        assert!(parse_contracts(br#"{"code":40010001,"message":"bad"}"#).is_err());
        assert!(parse_contracts(b"[]").is_err());
    }

    #[test]
    fn urls_pick_a_recording_by_underlying_and_expiry() {
        let a = Alpaca::default();
        let contracts = a.option_contracts("xlu");
        assert!(
            contracts
                .url(None)
                .contains("/v2/options/contracts?underlying_symbols=XLU&status=active"),
            "{}",
            contracts.url(None)
        );
        assert!(contracts.url(None).contains("expiration_date_lte="));
        assert_eq!(contracts.key(), "alpaca/options/contracts/XLU");
        let day = NaiveDate::from_ymd_opt(2026, 12, 18).unwrap();
        let chain = a.option_chain("XLU", day);
        assert!(
            chain
                .url(Some("t"))
                .ends_with("/v1beta1/options/snapshots/XLU?expiration_date=2026-12-18&feed=indicative&limit=1000&page_token=t")
        );
        assert_eq!(chain.key(), "alpaca/options/chain/XLU/2026-12-18");
    }
}
