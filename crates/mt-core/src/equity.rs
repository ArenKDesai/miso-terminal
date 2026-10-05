//! Stocks and ETFs: trades, quotes, bars and snapshots as a market-data source
//! delivers them, the tradable asset list, and the exchange's clock.
//!
//! Prices here are for display and charts (`f64`). Anything that feeds an
//! order or a position uses [`crate::money`]. Times are instants (`Utc`);
//! show them in New York time with [`crate::exchange`], never in MISO's
//! market time.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::instrument::Security;

/// Which feed a price came from, so every figure can say so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feed {
    /// The IEX exchange alone, real time (Alpaca's free plan). IEX carries a
    /// few percent of volume, so thinly traded names can lag.
    #[default]
    Iex,
    /// Every exchange (the consolidated tape), fifteen minutes late (free).
    DelayedSip,
    /// Every exchange, real time (a paid Alpaca plan).
    Sip,
}

impl Feed {
    pub const ALL: [Self; 3] = [Self::Iex, Self::DelayedSip, Self::Sip];

    /// The label printed next to prices.
    pub fn label(self) -> &'static str {
        match self {
            Self::Iex => "IEX",
            Self::DelayedSip => "SIP 15m delayed",
            Self::Sip => "SIP",
        }
    }

    /// A longer description, for settings.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Iex => "IEX, real time (free; thin names can lag)",
            Self::DelayedSip => "All exchanges, 15 minutes delayed (free)",
            Self::Sip => "All exchanges, real time (paid plan)",
        }
    }

    /// Whether figures lag the market on purpose.
    pub fn is_delayed(self) -> bool {
        self == Self::DelayedSip
    }
}

/// A last sale.
#[derive(Clone, Debug, PartialEq)]
pub struct Trade {
    pub time: DateTime<Utc>,
    pub price: f64,
    pub size: f64,
    /// The venue's one-letter code (`V` is IEX); see [`venue_name`].
    pub exchange: Option<String>,
}

/// The best bid and offer.
#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub time: DateTime<Utc>,
    pub bid: f64,
    pub bid_size: f64,
    pub ask: f64,
    pub ask_size: f64,
    pub bid_exchange: Option<String>,
    pub ask_exchange: Option<String>,
}

impl Quote {
    /// Both sides present (a zero price means that side is empty).
    pub fn is_two_sided(&self) -> bool {
        self.bid > 0.0 && self.ask > 0.0
    }

    pub fn mid(&self) -> Option<f64> {
        self.is_two_sided().then(|| (self.bid + self.ask) / 2.0)
    }

    pub fn spread(&self) -> Option<f64> {
        self.is_two_sided().then_some(self.ask - self.bid)
    }
}

/// One bar: open, high, low and close over a period starting at `time`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub time: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    /// Number of trades.
    pub trades: Option<u64>,
    /// Volume-weighted average price.
    pub vwap: Option<f64>,
}

/// Everything about one security right now: its latest trade, quote and
/// minute bar, today's (or the latest session's) daily bar and the one before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub latest_trade: Option<Trade>,
    pub latest_quote: Option<Quote>,
    pub minute_bar: Option<Bar>,
    pub daily_bar: Option<Bar>,
    pub prev_daily_bar: Option<Bar>,
}

impl Snapshot {
    /// The last price: the latest trade, else the latest bar's close.
    pub fn last(&self) -> Option<f64> {
        self.latest_trade
            .as_ref()
            .map(|t| t.price)
            .or_else(|| self.minute_bar.as_ref().map(|b| b.close))
            .or_else(|| self.daily_bar.as_ref().map(|b| b.close))
            .filter(|p| p.is_finite() && *p > 0.0)
    }

    /// When [`Snapshot::last`] traded.
    pub fn last_time(&self) -> Option<DateTime<Utc>> {
        self.latest_trade
            .as_ref()
            .map(|t| t.time)
            .or_else(|| self.minute_bar.as_ref().map(|b| b.time))
    }

    /// The close changes are measured from: the session before the last trade's.
    pub fn prev_close(&self) -> Option<f64> {
        self.reference_close(self.last_time())
    }

    /// The close before the session `at` falls in. Normally the previous daily
    /// bar; but a trade from a newer session than the daily bar (the first
    /// trades of a day, before the snapshot catches up) is measured from the
    /// daily bar itself.
    pub fn reference_close(&self, at: Option<DateTime<Utc>>) -> Option<f64> {
        let session = |t: DateTime<Utc>| crate::exchange::to_exchange(t).date_naive();
        let newer = match (at, &self.daily_bar) {
            (Some(at), Some(d)) => session(at) > session(d.time),
            _ => false,
        };
        let bar = if newer {
            self.daily_bar.as_ref()
        } else {
            self.prev_daily_bar.as_ref()
        };
        bar.map(|b| b.close).filter(|p| p.is_finite() && *p > 0.0)
    }

    pub fn change(&self) -> Option<f64> {
        Some(self.last()? - self.prev_close()?)
    }

    /// Change in percent.
    pub fn change_pct(&self) -> Option<f64> {
        Some(self.change()? / self.prev_close()? * 100.0)
    }
}

/// An option's sensitivities, per share of the underlying.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Greeks {
    pub delta: Option<f64>,
    pub gamma: Option<f64>,
    pub theta: Option<f64>,
    pub vega: Option<f64>,
    pub rho: Option<f64>,
}

/// One option contract right now: its latest trade and quote, and the
/// greeks and implied volatility the source derives from them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionSnapshot {
    pub latest_trade: Option<Trade>,
    pub latest_quote: Option<Quote>,
    pub greeks: Greeks,
    pub implied_volatility: Option<f64>,
}

impl OptionSnapshot {
    /// The mid of a two-sided quote, else the last trade.
    pub fn mark(&self) -> Option<f64> {
        self.latest_quote
            .as_ref()
            .and_then(Quote::mid)
            .or_else(|| self.latest_trade.as_ref().map(|t| t.price))
            .filter(|p| p.is_finite() && *p >= 0.0)
    }
}

/// A security that can be looked up (and, with an account, traded).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// As the exchange lists it: `XLU`, `BRK.B`.
    pub symbol: String,
    #[serde(default)]
    pub name: String,
    /// Primary listing: `NYSE`, `NASDAQ`, `ARCA`, `AMEX`, `BATS`, `OTC`.
    #[serde(default)]
    pub exchange: String,
    /// `us_equity`.
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub tradable: bool,
    #[serde(default)]
    pub marginable: bool,
    #[serde(default)]
    pub shortable: bool,
    #[serde(default)]
    pub easy_to_borrow: bool,
    #[serde(default)]
    pub fractionable: bool,
    /// Margin required to hold it, in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_margin: Option<f64>,
    /// Broker flags such as `has_options` or `ptp_no_exception`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
}

impl Asset {
    pub fn security(&self) -> Security {
        Security::us(&self.symbol)
    }

    /// The listing exchange's full name.
    pub fn exchange_name(&self) -> &str {
        match self.exchange.as_str() {
            "NYSE" => "New York Stock Exchange",
            "NASDAQ" => "Nasdaq",
            "ARCA" => "NYSE Arca",
            "AMEX" => "NYSE American",
            "BATS" => "Cboe BZX",
            "OTC" => "Over the counter",
            other => other,
        }
    }

    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.iter().any(|a| a.eq_ignore_ascii_case(name))
    }

    /// The name without a share-class boilerplate tail: `Xcel Energy Inc.`
    /// for `Xcel Energy Inc. Common Stock`.
    pub fn short_name(&self) -> &str {
        const TAILS: [&str; 5] = [
            " Common Stock",
            " Common Shares",
            " Ordinary Shares",
            " Common Units",
            " Shares",
        ];
        let name = self.name.trim();
        TAILS
            .iter()
            .find_map(|t| name.strip_suffix(t))
            .filter(|n| !n.is_empty())
            .unwrap_or(name)
    }
}

/// Every listed asset, sorted by symbol, with when it was fetched.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetList {
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub fetched: Option<DateTime<Utc>>,
}

impl AssetList {
    pub fn new(mut assets: Vec<Asset>, fetched: Option<DateTime<Utc>>) -> Self {
        assets.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        assets.dedup_by(|a, b| a.symbol == b.symbol);
        Self { assets, fetched }
    }

    /// The asset with exactly this symbol (any case).
    pub fn get(&self, symbol: &str) -> Option<&Asset> {
        let upper = symbol.to_ascii_uppercase();
        self.assets
            .binary_search_by(|a| a.symbol.as_str().cmp(&upper))
            .ok()
            .map(|i| &self.assets[i])
    }

    /// Assets matching what is being typed, best first: the exact symbol,
    /// then symbols starting with it, then (from three characters) names
    /// containing every word. Tradable listings rank before the rest.
    pub fn search(&self, text: &str, limit: usize) -> Vec<&Asset> {
        let upper = text.trim().to_ascii_uppercase();
        if upper.is_empty() || limit == 0 {
            return Vec::new();
        }
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let mut ranked: Vec<(u8, usize, &Asset)> = Vec::new();
        for a in &self.assets {
            let rank = if a.symbol == upper {
                0
            } else if a.symbol.starts_with(&upper) {
                1
            } else if upper.len() >= 3 && {
                let name = a.name.to_lowercase();
                words.iter().all(|w| name.contains(w.as_str()))
            } {
                2
            } else {
                continue;
            };
            ranked.push((rank, usize::from(!a.tradable), a));
        }
        ranked.sort_by(|x, y| {
            (x.0, x.1, x.2.symbol.len(), &x.2.symbol).cmp(&(
                y.0,
                y.1,
                y.2.symbol.len(),
                &y.2.symbol,
            ))
        });
        ranked.into_iter().take(limit).map(|(_, _, a)| a).collect()
    }
}

/// The market clock: whether the regular session is open and when it next
/// opens and closes.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketClock {
    /// When the source answered.
    pub at: DateTime<Utc>,
    pub is_open: bool,
    pub next_open: DateTime<Utc>,
    pub next_close: DateTime<Utc>,
}

/// A venue's name from the one-letter code trades and quotes carry.
pub fn venue_name(code: &str) -> &'static str {
    match code {
        "A" => "NYSE American",
        "B" => "Nasdaq BX",
        "C" => "NYSE National",
        "D" => "FINRA ADF",
        "E" => "Market independent",
        "H" => "MIAX Pearl",
        "I" => "ISE",
        "J" => "Cboe EDGA",
        "K" => "Cboe EDGX",
        "L" => "LTSE",
        "M" => "NYSE Chicago",
        "N" => "NYSE",
        "P" => "NYSE Arca",
        "Q" => "Nasdaq",
        "S" => "Nasdaq small cap",
        "T" => "Nasdaq Int",
        "U" => "MEMX",
        "V" => "IEX",
        "W" => "Cboe",
        "X" => "Nasdaq PSX",
        "Y" => "Cboe BYX",
        "Z" => "Cboe BZX",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn bar(close: f64) -> Bar {
        Bar {
            time: t("2026-10-02T04:00:00Z"),
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
            trades: None,
            vwap: None,
        }
    }

    #[test]
    fn changes_are_from_the_previous_close() {
        let mut s = Snapshot {
            daily_bar: Some(bar(81.0)),
            prev_daily_bar: Some(bar(80.0)),
            ..Snapshot::default()
        };
        assert_eq!(s.last(), Some(81.0), "no trade: the daily close");
        s.latest_trade = Some(Trade {
            time: t("2026-10-02T19:59:00Z"),
            price: 82.0,
            size: 100.0,
            exchange: Some("V".into()),
        });
        assert_eq!(s.last(), Some(82.0));
        assert_eq!(s.change(), Some(2.0));
        assert_eq!(s.change_pct(), Some(2.5));
        assert_eq!(Snapshot::default().change(), None);
        // Monday's first trade, before the snapshot has a Monday daily bar:
        // measured from Friday's close (the daily bar), not Thursday's.
        assert_eq!(
            s.reference_close(Some(t("2026-10-05T12:00:00Z"))),
            Some(81.0)
        );
        let zero = Snapshot {
            prev_daily_bar: Some(bar(0.0)),
            ..s
        };
        assert_eq!(zero.change_pct(), None, "no division by a zero close");
    }

    #[test]
    fn quotes() {
        let q = Quote {
            time: t("2026-10-02T19:59:00Z"),
            bid: 82.0,
            bid_size: 2.0,
            ask: 82.1,
            ask_size: 3.0,
            bid_exchange: None,
            ask_exchange: None,
        };
        assert!((q.mid().unwrap() - 82.05).abs() < 1e-9);
        let one_sided = Quote { bid: 0.0, ..q };
        assert_eq!(one_sided.mid(), None);
        assert_eq!(venue_name("V"), "IEX");
    }

    #[test]
    fn asset_search_ranks_symbols_before_names() {
        let asset = |symbol: &str, name: &str, tradable: bool| Asset {
            symbol: symbol.into(),
            name: name.into(),
            exchange: "NYSE".into(),
            class: "us_equity".into(),
            active: true,
            tradable,
            marginable: true,
            shortable: true,
            easy_to_borrow: true,
            fractionable: true,
            maintenance_margin: None,
            attributes: vec![],
        };
        let list = AssetList::new(
            vec![
                asset("XLU", "Utilities Select Sector SPDR Fund", true),
                asset("XEL", "Xcel Energy Inc.", true),
                asset("XELB", "Xcel Brands", false),
                asset("XELA", "Exela Technologies", true),
                asset("UTG", "Reaves Utility Income Fund", true),
                asset("XEL", "duplicate", true),
            ],
            None,
        );
        assert_eq!(list.assets.len(), 5, "sorted and de-duplicated");
        assert_eq!(
            list.get("xel").map(|a| a.name.as_str()),
            Some("Xcel Energy Inc.")
        );
        let symbols = |q: &str| -> Vec<&str> {
            list.search(q, 10)
                .iter()
                .map(|a| a.symbol.as_str())
                .collect()
        };
        assert_eq!(symbols("xel"), ["XEL", "XELA", "XELB"]);
        assert_eq!(symbols("utilit"), ["UTG", "XLU"]);
        assert_eq!(symbols("energy xcel"), ["XEL"]);
        assert!(symbols("").is_empty());
        assert_eq!(list.get("XLU").unwrap().security().to_string(), "XLU US");
        let named = |n: &str| Asset {
            name: n.into(),
            ..list.assets[0].clone()
        };
        assert_eq!(
            named("Xcel Energy Inc. Common Stock").short_name(),
            "Xcel Energy Inc."
        );
        assert_eq!(
            named("Alphabet Inc. Class A Common Stock").short_name(),
            "Alphabet Inc. Class A"
        );
        assert_eq!(
            named("SPDR S&P 500 ETF Trust").short_name(),
            "SPDR S&P 500 ETF Trust"
        );
    }
}
