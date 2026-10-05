//! Alpaca as a data source and a broker: US stock and ETF prices
//! (snapshots, bars, live trades and quotes), the asset list behind ticker
//! completion, the market clock and calendar, company news (Benzinga's,
//! through Alpaca), and the paper account: balances, positions, the equity
//! curve, activities, orders and order events ([`account`], [`orders`],
//! [`TradeStream`]); and options: the contracts listed on an underlying and
//! each expiry's chain of quotes and greeks ([`options`]). Orders are placed, replaced and cancelled only by the
//! [`OrderDesk`], never by a query, and every request it sends is written to
//! the [`AuditLog`].
//!
//! Built for Alpaca's free plan: real-time prices from IEX alone, or every
//! exchange fifteen minutes late ([`Feed`]); 200 requests a minute per key,
//! which the shared [`budgets`] keeps to 180; one stream connection per
//! endpoint and account, with 30 trade and quote subscriptions in all (minute
//! bars have no limit). Every price says which feed it came from.
//!
//! The keys are the user's own, read from the secret store ([`KEY_ID`],
//! [`SECRET_KEY`]) for each request and never written anywhere else; a replay
//! needs none. Like `mt-nws`, this crate depends only on `mt-core` and
//! `mt-data`.

pub mod account;
mod audit;
pub mod board;
pub mod config;
pub mod desk;
pub mod options;
pub mod orders;
pub mod parse;
mod queries;
mod stream;
mod text;
mod trades;

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use mt_core::equity::Feed;
use mt_data::{Budget, FetchCtx, FetchError, Request, SecretStore};

pub use account::{
    AccountQuery, Activities, ActivitiesQuery, HistoryPeriod, OPTION_FEED_LABEL, OptionSnapshots,
    OptionSnapshotsQuery, PortfolioHistoryQuery, PositionsQuery,
};
pub use audit::AuditLog;
pub use config::{MarketsConfig, SecurityList, builtin_lists, normalize_symbol};
pub use desk::{ActionState, OrderDesk, Outcome, new_client_order_id};
pub use options::{OptionChain, OptionChainQuery, OptionContractsQuery};
pub use orders::{OrdersQuery, Replacement};
pub use queries::{
    ASSETS_KEY, AssetsQuery, BarSet, BarSource, BarsQuery, CalendarQuery, ClockQuery, NewsList,
    NewsQuery, Snapshots, SnapshotsQuery, Timeframe, assets_from_bytes, assets_to_bytes,
};
pub use stream::{LiveMarket, LiveNews, MarketStream, NewsStream, market_topics};
pub use trades::{LiveTrades, TRADE_UPDATES, TradeStream};

/// The paper account's key ID in the secret store.
pub const KEY_ID: &str = "alpaca/paper/key-id";
/// The paper account's secret key in the secret store.
pub const SECRET_KEY: &str = "alpaca/paper/secret-key";

/// Alpaca allows 200 requests a minute per key, across its APIs.
pub const REQUESTS_PER_MINUTE: u32 = 180;

/// Which account the keys open. Only paper exists until live trading
/// (the markets plan's last phase); the status bar's band says which.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AccountMode {
    #[default]
    Paper,
    Live,
}

impl AccountMode {
    /// For keys and URLs: `paper`, `live`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Live => "live",
        }
    }

    pub fn name(self) -> &'static str {
        self.key()
    }

    /// What the status bar's band says.
    pub fn band(self) -> &'static str {
        match self {
            Self::Paper => "PAPER",
            Self::Live => "LIVE",
        }
    }
}

/// Where Alpaca's APIs live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// Market data: bars, snapshots, news.
    pub data: String,
    /// The paper trading API: the account, positions, activities, orders,
    /// and the assets, clock and calendar.
    pub trading: String,
    /// Market data streams.
    pub stream: String,
    /// The account's order events.
    pub trading_stream: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            data: "https://data.alpaca.markets".into(),
            trading: "https://paper-api.alpaca.markets".into(),
            stream: "wss://stream.data.alpaca.markets".into(),
            trading_stream: "wss://paper-api.alpaca.markets/stream".into(),
        }
    }
}

/// The hosts every Alpaca request goes to, for the request budget.
pub const HOSTS: [&str; 3] = [
    "data.alpaca.markets",
    "paper-api.alpaca.markets",
    "api.alpaca.markets",
];

/// One budget for every Alpaca host: the per-key limit is shared.
pub fn budgets() -> Vec<Budget> {
    vec![Budget::new(
        "Alpaca (200 a minute per key)",
        HOSTS,
        REQUESTS_PER_MINUTE,
        Duration::from_secs(60),
    )]
}

/// Whether both paper keys are in `store`.
pub fn has_keys(store: &dyn SecretStore) -> bool {
    [KEY_ID, SECRET_KEY]
        .iter()
        .all(|k| matches!(store.get(k), Ok(Some(s)) if !s.is_empty()))
}

/// A GET with the account's keys as headers (`***` in the log). A replay
/// needs none, so recordings never meet a key.
pub(crate) fn request(ctx: &FetchCtx, url: String) -> Result<Request, FetchError> {
    authed(ctx, Request::get(url))
}

/// Any request with the account's keys as headers (none for a replay).
pub(crate) fn authed(ctx: &FetchCtx, req: Request) -> Result<Request, FetchError> {
    if !ctx.is_live() {
        return Ok(req);
    }
    Ok(req
        .secret_header("APCA-API-KEY-ID", ctx.secret(KEY_ID)?)
        .secret_header("APCA-API-SECRET-KEY", ctx.secret(SECRET_KEY)?))
}

/// Entry point for building Alpaca queries and streams.
#[derive(Clone, Debug)]
pub struct Alpaca {
    endpoints: Arc<Endpoints>,
    feed: Feed,
    stream_limit: usize,
    ready: bool,
}

impl Default for Alpaca {
    fn default() -> Self {
        Self::new(&MarketsConfig::default(), false)
    }
}

impl Alpaca {
    /// `ready`: keys are stored (or the data is a replay, which needs none).
    /// Panels show a prompt to add keys instead of failing requests when not.
    pub fn new(config: &MarketsConfig, ready: bool) -> Self {
        Self {
            endpoints: Arc::new(Endpoints::default()),
            feed: config.feed,
            stream_limit: config.stream_limit(),
            ready,
        }
    }

    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = Arc::new(endpoints);
        self
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Where live prices come from.
    pub fn feed(&self) -> Feed {
        self.feed
    }

    /// Which account the keys open: paper, until live trading exists.
    pub fn mode(&self) -> AccountMode {
        AccountMode::Paper
    }

    /// The latest trade, quote and bars for each symbol, refreshed every minute.
    pub fn snapshots<S: AsRef<str>>(&self, symbols: impl IntoIterator<Item = S>) -> SnapshotsQuery {
        SnapshotsQuery::new(self.clone(), symbols)
    }

    /// Bars for `symbols` from the start of `start` (New York date) to the end
    /// of `end`, or to now. Daily bars come from every exchange (on the free
    /// plan, up to fifteen minutes ago); shorter ones from the live feed.
    pub fn bars<S: AsRef<str>>(
        &self,
        symbols: impl IntoIterator<Item = S>,
        timeframe: Timeframe,
        start: NaiveDate,
        end: Option<NaiveDate>,
    ) -> BarsQuery {
        let source = if timeframe == Timeframe::Day1 && self.feed != Feed::Sip {
            BarSource::DelayedSip
        } else {
            BarSource::from_feed(self.feed)
        };
        BarsQuery::new(self.clone(), symbols, timeframe, start, end, source)
    }

    /// Active US stocks and ETFs, refreshed daily and kept across restarts.
    pub fn assets(&self) -> AssetsQuery {
        AssetsQuery::new(self.clone())
    }

    pub fn clock(&self) -> ClockQuery {
        ClockQuery::new(self.clone())
    }

    /// Trading days (holidays left out, early closes included) from a week
    /// before `around` to six weeks after.
    pub fn calendar(&self, around: NaiveDate) -> CalendarQuery {
        CalendarQuery::new(
            self.clone(),
            around - chrono::Duration::days(7),
            around + chrono::Duration::days(42),
        )
    }

    /// Company news for `symbols` (every story when empty), newest first.
    pub fn news<S: AsRef<str>>(&self, symbols: impl IntoIterator<Item = S>) -> NewsQuery {
        NewsQuery::new(self.clone(), symbols)
    }

    /// Live trades, quotes and minute bars. Topics are `trades:XLU`,
    /// `quotes:XLU` and `bars:XLU` (see [`market_topics`]).
    pub fn market_stream(&self) -> MarketStream {
        MarketStream::new(
            format!("{}/v2/{}", self.endpoints.stream, self.feed_path()),
            self.feed,
            self.stream_limit,
        )
    }

    /// New stories as they are published. Topics are `news:XLU` or `news:*`.
    pub fn news_stream(&self) -> NewsStream {
        NewsStream::new(format!("{}/v1beta1/news", self.endpoints.stream))
    }

    /// Balances, buying power, margin and the account's standing.
    pub fn account(&self) -> AccountQuery {
        AccountQuery::new(self.clone())
    }

    /// Open positions, valued by Alpaca.
    pub fn positions(&self) -> PositionsQuery {
        PositionsQuery::new(self.clone())
    }

    /// The equity curve over `period`.
    pub fn portfolio_history(&self, period: HistoryPeriod) -> PortfolioHistoryQuery {
        PortfolioHistoryQuery::new(self.clone(), period)
    }

    /// Fills, dividends, fees, transfers and option events, newest first.
    pub fn activities(&self) -> ActivitiesQuery {
        ActivitiesQuery::new(self.clone())
    }

    /// Quotes and greeks for option contracts (OCC symbols).
    pub fn option_snapshots<S: AsRef<str>>(
        &self,
        symbols: impl IntoIterator<Item = S>,
    ) -> OptionSnapshotsQuery {
        OptionSnapshotsQuery::new(self.clone(), symbols)
    }

    /// Every active contract listed on `underlying` (a ticker), with open
    /// interest and the latest close; the expiries OMON offers.
    pub fn option_contracts(&self, underlying: &str) -> OptionContractsQuery {
        OptionContractsQuery::new(self.clone(), underlying)
    }

    /// One expiry's chain on `underlying`: quotes, greeks and implied
    /// volatility for every contract (Alpaca's indicative feed).
    pub fn option_chain(&self, underlying: &str, expiry: NaiveDate) -> OptionChainQuery {
        OptionChainQuery::new(self.clone(), underlying, expiry)
    }

    /// The account's orders, open and recent, newest first.
    pub fn orders(&self) -> OrdersQuery {
        OrdersQuery::new(self.clone())
    }

    /// The account's order events. The topic is [`TRADE_UPDATES`].
    pub fn trade_stream(&self) -> TradeStream {
        TradeStream::new(self.endpoints.trading_stream.clone(), self.mode())
    }

    /// The market-data stream's path segment for the feed.
    fn feed_path(&self) -> &'static str {
        match self.feed {
            Feed::Iex => "iex",
            Feed::DelayedSip => "delayed_sip",
            Feed::Sip => "sip",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_data::{EventLog, FetchCtxOptions, MemorySecrets, Secret, Transport};

    struct Live;
    impl Transport for Live {
        fn send<'a>(
            &'a self,
            _req: &'a Request,
        ) -> mt_data::BoxFuture<'a, Result<mt_data::Response, FetchError>> {
            Box::pin(async { Err(FetchError::Network("unused".into())) })
        }
        fn describe(&self) -> String {
            "live".into()
        }
    }

    #[test]
    fn keys_go_in_headers_and_only_when_live() {
        let secrets = Arc::new(MemorySecrets::default());
        let ctx = FetchCtx::new(
            Arc::new(Live),
            None,
            FetchCtxOptions {
                secrets: secrets.clone(),
                ..FetchCtxOptions::default()
            },
            EventLog::default(),
        );
        assert!(!has_keys(secrets.as_ref()));
        assert!(matches!(
            request(&ctx, "https://data.alpaca.markets/v2/x".into()),
            Err(FetchError::Auth(m)) if m.contains(KEY_ID)
        ));
        secrets.set(KEY_ID, &Secret::new("PKTEST")).unwrap();
        secrets.set(SECRET_KEY, &Secret::new("shh")).unwrap();
        assert!(has_keys(secrets.as_ref()));
        let req = request(&ctx, "https://data.alpaca.markets/v2/x".into()).unwrap();
        assert_eq!(
            req.header_value("APCA-API-KEY-ID").map(|v| v.expose()),
            Some("PKTEST")
        );
        assert!(!req.describe().contains("shh"));
        let replay = FetchCtx::new(
            Arc::new(mt_data::FixtureTransport::new("unused")),
            None,
            FetchCtxOptions::default(),
            EventLog::default(),
        );
        let req = request(&replay, "https://data.alpaca.markets/v2/x".into()).unwrap();
        assert!(req.headers.is_empty(), "a replay sends no keys");
    }

    #[test]
    fn feeds_choose_streams_and_bar_sources() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        let iex = Alpaca::default();
        assert!(iex.market_stream().url().ends_with("/v2/iex"));
        assert_eq!(
            iex.bars(["XLU"], Timeframe::Min1, today, None).source(),
            BarSource::Iex
        );
        assert_eq!(
            iex.bars(["XLU"], Timeframe::Day1, today, None).source(),
            BarSource::DelayedSip,
            "daily history from every exchange, even on the free plan"
        );
        let cfg = MarketsConfig {
            feed: Feed::DelayedSip,
            ..MarketsConfig::default()
        };
        let delayed = Alpaca::new(&cfg, true);
        assert!(delayed.market_stream().url().ends_with("/v2/delayed_sip"));
        assert_eq!(
            delayed.bars(["XLU"], Timeframe::Min1, today, None).source(),
            BarSource::DelayedSip
        );
        let sip = Alpaca::new(
            &MarketsConfig {
                feed: Feed::Sip,
                ..MarketsConfig::default()
            },
            true,
        );
        assert_eq!(
            sip.bars(["XLU"], Timeframe::Day1, today, None).source(),
            BarSource::Sip
        );
    }
}
