//! Settings for market data (`[markets]` in config.toml) and the security
//! lists `Q` shows. The lists are built in, so a release can fix one; the
//! config adds lists or replaces a built-in one by name.

use mt_core::equity::Feed;
use mt_core::instrument::{Security, is_ticker};
use serde::{Deserialize, Serialize};

/// What the free plan allows: trades and quotes for 30 symbols at a time.
pub const FREE_PLAN_STREAM_SYMBOLS: usize = 30;

/// The list `Q` opens with.
pub const DEFAULT_LIST: &str = "POWER";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketsConfig {
    /// Where live prices come from: `iex` (real time, IEX only; the
    /// default), `delayed_sip` (every exchange, 15 minutes late) or `sip`
    /// (every exchange, real time; a paid Alpaca plan). Daily history always
    /// comes from every exchange.
    pub feed: Feed,
    /// Symbols that stream every trade and quote; the rest update with
    /// minute bars and snapshots. The free plan allows 30.
    pub stream_symbols: usize,
    /// Lists for `Q`, as `[[markets.lists]]` (name, title, symbols). A
    /// built-in list's name replaces it.
    pub lists: Vec<SecurityList>,
}

impl Default for MarketsConfig {
    fn default() -> Self {
        Self {
            feed: Feed::Iex,
            stream_symbols: FREE_PLAN_STREAM_SYMBOLS,
            lists: Vec::new(),
        }
    }
}

/// A named list of US securities, for `Q <name>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityList {
    /// What `Q` takes, e.g. `POWER`. Matched ignoring case.
    pub name: String,
    #[serde(default)]
    pub title: String,
    /// Tickers (`XEL`) or securities (`XEL US`).
    pub symbols: Vec<String>,
}

impl SecurityList {
    fn new(name: &str, title: &str, symbols: &[&str]) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            symbols: symbols.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// The list's securities, upper case, without duplicates or anything
    /// that is not a ticker.
    pub fn securities(&self) -> Vec<Security> {
        let mut out: Vec<Security> = Vec::new();
        for s in &self.symbols {
            if let Some(sym) = normalize_symbol(s)
                && !out.iter().any(|o| o.ticker == sym)
            {
                out.push(Security::us(&sym));
            }
        }
        out
    }

    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            &self.name
        } else {
            &self.title
        }
    }
}

/// `xel`, `XEL US` or `XEL US Equity` as Alpaca's symbol, `XEL`.
pub fn normalize_symbol(s: &str) -> Option<String> {
    let s = s.trim();
    let ticker = match Security::parse(s) {
        Some(sec) => sec.ticker,
        None => s.to_ascii_uppercase(),
    };
    is_ticker(&ticker).then_some(ticker)
}

const UTILITIES: &[&str] = &[
    "AEE", "XEL", "LNT", "WEC", "DTE", "CMS", "ETR", "CNP", "MGEE", "NI", "OTTR",
];
const GENERATORS: &[&str] = &["VST", "NRG", "CEG", "TLN"];
const ETFS: &[&str] = &["XLU", "XLE", "UNG"];
const GAS: &[&str] = &["EQT", "AR", "RRC", "CTRA", "EXE"];

/// The built-in lists. `POWER` is the energy desk's default: utilities in
/// MISO's footprint, independent generators, sector ETFs and gas producers.
pub fn builtin_lists() -> Vec<SecurityList> {
    let power: Vec<&str> = [UTILITIES, GENERATORS, ETFS, GAS].concat();
    let mut gas = vec!["UNG"];
    gas.extend_from_slice(GAS);
    vec![
        SecurityList::new(DEFAULT_LIST, "Power & gas", &power),
        SecurityList::new("UTILITIES", "Utilities in the MISO footprint", UTILITIES),
        SecurityList::new("GENERATORS", "Independent power producers", GENERATORS),
        SecurityList::new("GAS", "Natural gas", &gas),
        SecurityList::new("ETFS", "Energy and utility ETFs", ETFS),
    ]
}

impl MarketsConfig {
    /// Built-in lists (replaced by any configured list of the same name),
    /// then the configured additions.
    pub fn all_lists(&self) -> Vec<SecurityList> {
        let mut out: Vec<SecurityList> = builtin_lists()
            .into_iter()
            .map(|b| {
                self.lists
                    .iter()
                    .find(|l| l.name.eq_ignore_ascii_case(&b.name))
                    .cloned()
                    .unwrap_or(b)
            })
            .collect();
        for l in &self.lists {
            if !out.iter().any(|o| o.name.eq_ignore_ascii_case(&l.name)) {
                out.push(l.clone());
            }
        }
        out
    }

    pub fn list(&self, name: &str) -> Option<SecurityList> {
        self.all_lists()
            .into_iter()
            .find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The stream limit, never zero.
    pub fn stream_symbols(&self) -> usize {
        self.stream_symbols.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_built_in_and_replaceable() {
        let cfg: MarketsConfig = serde_json::from_str(
            r#"{"feed":"delayed_sip","lists":[
                {"name":"power","title":"Mine","symbols":["xel us","XEL","bad ticker!","aee"]},
                {"name":"BANKS","symbols":["JPM","BAC"]}]}"#,
        )
        .unwrap();
        assert_eq!(cfg.feed, Feed::DelayedSip);
        assert_eq!(cfg.stream_symbols, FREE_PLAN_STREAM_SYMBOLS);
        let power = cfg.list("POWER").unwrap();
        assert_eq!(power.display_title(), "Mine");
        let tickers: Vec<String> = power.securities().into_iter().map(|s| s.ticker).collect();
        assert_eq!(tickers, ["XEL", "AEE"]);
        assert!(cfg.list("banks").is_some());
        assert_eq!(cfg.all_lists().len(), builtin_lists().len() + 1);
        let default = MarketsConfig::default().list(DEFAULT_LIST).unwrap();
        assert_eq!(default.securities().len(), 23);
        assert!(
            default.securities().len() <= FREE_PLAN_STREAM_SYMBOLS,
            "the default list streams in full"
        );
        assert_eq!(normalize_symbol("brk.b"), Some("BRK.B".into()));
        assert_eq!(normalize_symbol("XLU US Equity"), Some("XLU".into()));
        assert_eq!(normalize_symbol(""), None);
    }
}
