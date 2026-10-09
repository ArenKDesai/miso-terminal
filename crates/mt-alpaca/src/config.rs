//! Settings for market data (`[markets]` in config.toml), the security
//! lists `Q` shows and the benchmarks BETA compares with. The lists are built
//! in, so a release can fix one; the config adds lists or replaces a built-in
//! one by name.

use std::collections::BTreeMap;

use mt_core::equity::Feed;
use mt_core::instrument::{Security, is_ticker};
use mt_core::studies::Study;
use serde::{Deserialize, Serialize};

/// What the free plan allows: 30 trade and quote subscriptions in all (a
/// symbol's trades count one, its quotes another). Minute bars are unlimited.
pub const FREE_PLAN_STREAM_LIMIT: usize = 30;

/// The list `Q` opens with.
pub const DEFAULT_LIST: &str = "POWER";

/// What BETA always compares with: Alpaca carries ETFs, not indexes, so the
/// S&P 500 is the largest ETF that tracks it.
pub const MARKET_BENCHMARK: &str = "SPY";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketsConfig {
    /// Where live prices come from: `iex` (real time, IEX only; the
    /// default), `delayed_sip` (every exchange, 15 minutes late) or `sip`
    /// (every exchange, real time; a paid Alpaca plan). Daily history always
    /// comes from every exchange.
    pub feed: Feed,
    /// Trade and quote subscriptions on the stream: every symbol's trades
    /// first, then quotes while room remains; the rest update with minute
    /// bars and snapshots. The free plan allows 30 in all.
    pub stream_limit: usize,
    /// The studies a new GP chart of a security starts with, written as on
    /// the command line: `["SMA50", "SMA200", "RSI14"]`. GP's Studies menu
    /// sets it (*Use for new charts*). Left out of the file when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub studies: Vec<String>,
    /// Industry benchmarks for BETA by security, as `[markets.benchmarks]`:
    /// `"VST US" = "XLU US"`, or a list's name for an equal-weighted basket
    /// of it (`VST = "GENERATORS"`). Ahead of the lists' own benchmarks.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub benchmarks: BTreeMap<String, String>,
    /// Lists for `Q`, as `[[markets.lists]]` (name, title, symbols). A
    /// built-in list's name replaces it. Left out of the file when empty, so
    /// a hand-written `[[markets.lists]]` never clashes with `lists = []`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<SecurityList>,
}

impl Default for MarketsConfig {
    fn default() -> Self {
        Self {
            feed: Feed::Iex,
            stream_limit: FREE_PLAN_STREAM_LIMIT,
            studies: Vec::new(),
            benchmarks: BTreeMap::new(),
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
    /// The industry benchmark for BETA of the list's securities: a security
    /// (`XLU US`) or a list's name for an equal-weighted basket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<String>,
}

impl SecurityList {
    fn new(name: &str, title: &str, symbols: &[&str]) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            symbols: symbols.iter().map(|s| (*s).to_owned()).collect(),
            benchmark: None,
        }
    }

    fn benchmark(mut self, benchmark: &str) -> Self {
        self.benchmark = Some(benchmark.into());
        self
    }

    /// Whether `security` is on the list.
    pub fn contains(&self, security: &Security) -> bool {
        self.securities().contains(security)
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
// Coterra (CTRA) stopped trading in May 2026.
const GAS: &[&str] = &["EQT", "AR", "RRC", "EXE"];

/// The built-in lists. `POWER` is the energy desk's default: utilities in
/// MISO's footprint, independent generators, sector ETFs and gas producers.
/// Their benchmarks are the sectors' ETFs (independent producers are in the
/// utilities sector); `POWER` mixes sectors and `ETFS` are benchmarks, so
/// neither has one.
pub fn builtin_lists() -> Vec<SecurityList> {
    let power: Vec<&str> = [UTILITIES, GENERATORS, ETFS, GAS].concat();
    let mut gas = vec!["UNG"];
    gas.extend_from_slice(GAS);
    vec![
        SecurityList::new(DEFAULT_LIST, "Power & gas", &power),
        SecurityList::new("UTILITIES", "Utilities in the MISO footprint", UTILITIES)
            .benchmark("XLU US"),
        SecurityList::new("GENERATORS", "Independent power producers", GENERATORS)
            .benchmark("XLU US"),
        SecurityList::new("GAS", "Natural gas", &gas).benchmark("XLE US"),
        SecurityList::new("ETFS", "Energy and utility ETFs", ETFS),
    ]
}

/// What BETA compares a security with besides the S&P 500.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Benchmark {
    /// One security, usually a sector ETF.
    Security(Security),
    /// An equal-weighted basket of a list, without the security compared.
    Basket {
        list: String,
        members: Vec<Security>,
    },
}

impl Benchmark {
    /// `XLU US`, or `GENERATORS basket`.
    pub fn label(&self) -> String {
        match self {
            Self::Security(s) => s.to_string(),
            Self::Basket { list, .. } => format!("{list} basket"),
        }
    }

    /// The tickers whose bars it needs.
    pub fn tickers(&self) -> Vec<String> {
        match self {
            Self::Security(s) => vec![s.ticker.clone()],
            Self::Basket { members, .. } => members.iter().map(|m| m.ticker.clone()).collect(),
        }
    }
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

    /// The studies new GP charts start with, in order, without repeats or
    /// anything that does not read as a study.
    pub fn studies(&self) -> Vec<Study> {
        let mut out: Vec<Study> = Vec::new();
        for s in self.studies.iter().filter_map(|s| Study::parse(s)) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// A benchmark for `security` written as a list's name (`GENERATORS`,
    /// an equal-weighted basket of it) or a security (`XLU US`, `XLU`).
    /// `None` if it is neither, or would compare the security with itself.
    pub fn benchmark(&self, spec: &str, security: &Security) -> Option<Benchmark> {
        if let Some(list) = self.list(spec) {
            let members: Vec<Security> = list
                .securities()
                .into_iter()
                .filter(|s| s != security)
                .collect();
            return (!members.is_empty()).then(|| Benchmark::Basket {
                list: list.name.to_ascii_uppercase(),
                members,
            });
        }
        let ticker = normalize_symbol(spec)?;
        (ticker != security.ticker).then(|| Benchmark::Security(Security::us(&ticker)))
    }

    /// The industry benchmark for `security`: its `[markets.benchmarks]`
    /// entry, else the benchmark of the first list holding it that has one.
    pub fn industry(&self, security: &Security) -> Option<Benchmark> {
        let configured = self
            .benchmarks
            .iter()
            .find(|(k, _)| normalize_symbol(k).as_deref() == Some(security.ticker.as_str()));
        if let Some((_, spec)) = configured {
            return self.benchmark(spec, security);
        }
        self.all_lists()
            .iter()
            .filter(|l| l.contains(security))
            .find_map(|l| self.benchmark(l.benchmark.as_deref()?, security))
    }

    /// The stream limit, never zero.
    pub fn stream_limit(&self) -> usize {
        self.stream_limit.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn industry_benchmarks_come_from_config_then_lists() {
        let cfg = MarketsConfig::default();
        let xlu = Benchmark::Security(Security::us("XLU"));
        assert_eq!(cfg.industry(&Security::us("XEL")), Some(xlu.clone()));
        assert_eq!(cfg.industry(&Security::us("VST")), Some(xlu));
        assert_eq!(
            cfg.industry(&Security::us("EQT")),
            Some(Benchmark::Security(Security::us("XLE")))
        );
        // ETFs, and anything on no list with a benchmark, have none.
        assert_eq!(cfg.industry(&Security::us("XLU")), None);
        assert_eq!(cfg.industry(&Security::us("AAPL")), None);

        let cfg: MarketsConfig = serde_json::from_str(
            r#"{"benchmarks":{"vst us":"generators","AAPL":"QQQ US","XEL":"XEL"},
                "lists":[{"name":"UTILITIES","symbols":["XEL","WEC"],"benchmark":"IDU"}]}"#,
        )
        .unwrap();
        let basket = cfg.industry(&Security::us("VST")).unwrap();
        assert_eq!(basket.label(), "GENERATORS basket");
        assert_eq!(basket.tickers(), ["NRG", "CEG", "TLN"], "without VST");
        assert_eq!(
            cfg.industry(&Security::us("AAPL")).unwrap().label(),
            "QQQ US"
        );
        assert_eq!(
            cfg.industry(&Security::us("WEC")).unwrap().label(),
            "IDU US",
            "a configured list's benchmark"
        );
        // Compared with itself: no benchmark.
        assert_eq!(cfg.industry(&Security::us("XEL")), None);
        assert_eq!(cfg.benchmark("not a ticker!", &Security::us("XEL")), None);
    }

    #[test]
    fn studies_read_as_on_the_command_line() {
        let cfg: MarketsConfig =
            serde_json::from_str(r#"{"studies":["sma50","SMA 50","BB20,2.5","nonsense","RSI"]}"#)
                .unwrap();
        assert_eq!(
            cfg.studies(),
            [
                Study::Sma { period: 50 },
                Study::Bollinger {
                    period: 20,
                    width: 2.5
                },
                Study::Rsi { period: 14 }
            ]
        );
    }

    #[test]
    fn lists_are_built_in_and_replaceable() {
        let cfg: MarketsConfig = serde_json::from_str(
            r#"{"feed":"delayed_sip","lists":[
                {"name":"power","title":"Mine","symbols":["xel us","XEL","bad ticker!","aee"]},
                {"name":"BANKS","symbols":["JPM","BAC"]}]}"#,
        )
        .unwrap();
        assert_eq!(cfg.feed, Feed::DelayedSip);
        assert_eq!(cfg.stream_limit, FREE_PLAN_STREAM_LIMIT);
        let power = cfg.list("POWER").unwrap();
        assert_eq!(power.display_title(), "Mine");
        let tickers: Vec<String> = power.securities().into_iter().map(|s| s.ticker).collect();
        assert_eq!(tickers, ["XEL", "AEE"]);
        assert!(cfg.list("banks").is_some());
        assert_eq!(cfg.all_lists().len(), builtin_lists().len() + 1);
        let default = MarketsConfig::default().list(DEFAULT_LIST).unwrap();
        assert_eq!(default.securities().len(), 22);
        assert!(
            default.securities().len() <= FREE_PLAN_STREAM_LIMIT,
            "every trade streams for the whole default list"
        );
        assert_eq!(normalize_symbol("brk.b"), Some("BRK.B".into()));
        assert!(cfg.studies().is_empty());
        assert_eq!(normalize_symbol("XLU US Equity"), Some("XLU".into()));
        assert_eq!(normalize_symbol(""), None);
    }
}
