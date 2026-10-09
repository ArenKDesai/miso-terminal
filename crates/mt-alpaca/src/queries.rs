//! Alpaca's REST datasets as hub queries.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use mt_core::equity::{AssetList, Bar, Feed, MarketClock, Snapshot};
use mt_core::exchange::{self, Session, TradingDay};
use mt_core::news::Headline;
use mt_data::{FetchCtx, FetchError, Freshness, Query};

use crate::{Alpaca, parse, request};

/// Symbols per snapshot request (they go in the URL).
const SNAPSHOT_CHUNK: usize = 100;
/// Bars per page, Alpaca's maximum.
const PAGE_LIMIT: u32 = 10_000;
/// Pages followed before giving up (and saying so).
const MAX_PAGES: usize = 20;
/// The free plan does not serve the consolidated tape's latest 15 minutes.
const SIP_DELAY: chrono::Duration = chrono::Duration::minutes(16);
/// How old a saved asset list may be and still be used at launch.
const ASSETS_REUSE: chrono::Duration = chrono::Duration::hours(20);
/// Headlines kept per news query.
const NEWS_KEEP: usize = 200;

/// Where the asset list is kept between sessions.
pub const ASSETS_KEY: &str = "local://alpaca/assets";

pub fn assets_to_bytes(list: &AssetList) -> Vec<u8> {
    serde_json::to_vec(list).unwrap_or_default()
}

pub fn assets_from_bytes(bytes: &[u8]) -> Option<AssetList> {
    serde_json::from_slice(bytes).ok()
}

/// Upper case, de-duplicated and sorted, so the same set is the same query.
fn symbol_set<S: AsRef<str>>(symbols: impl IntoIterator<Item = S>) -> Vec<String> {
    let mut out: Vec<String> = symbols
        .into_iter()
        .filter_map(|s| crate::normalize_symbol(s.as_ref()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `XLU, XEL, AEE` or `XLU, XEL, AEE +20`, for LOG.
fn short_list(symbols: &[String]) -> String {
    if symbols.is_empty() {
        return "all".into();
    }
    let shown = symbols
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match symbols.len() {
        0..=3 => shown,
        n => format!("{shown} +{}", n - 3),
    }
}

/// Whether US markets are in any session now (standard hours; good enough
/// for choosing how often to refresh).
fn trading_hours() -> bool {
    exchange::market_status(mt_core::time::now_utc(), &[]).session != Session::Closed
}

fn exchange_today() -> NaiveDate {
    exchange::now_exchange().date_naive()
}

/// The start of a New York date as an instant.
fn day_start(d: NaiveDate) -> DateTime<Utc> {
    exchange::exchange_to_utc(d.and_time(NaiveTime::MIN))
        .unwrap_or_else(|| d.and_time(NaiveTime::MIN).and_utc())
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Percent-encode a page token (base64: `+`, `/` and `=` need it).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------- snapshots

/// Snapshots for a set of symbols.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshots {
    pub feed: Feed,
    pub by_symbol: BTreeMap<String, Snapshot>,
}

impl Snapshots {
    pub fn get(&self, symbol: &str) -> Option<&Snapshot> {
        self.by_symbol.get(symbol)
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotsQuery {
    alpaca: Alpaca,
    symbols: Vec<String>,
}

impl SnapshotsQuery {
    pub(crate) fn new<S: AsRef<str>>(alpaca: Alpaca, symbols: impl IntoIterator<Item = S>) -> Self {
        Self {
            alpaca,
            symbols: symbol_set(symbols),
        }
    }

    pub fn symbols(&self) -> &[String] {
        &self.symbols
    }

    fn feed_param(&self) -> &'static str {
        match self.alpaca.feed {
            Feed::Iex => "iex",
            Feed::DelayedSip => "delayed_sip",
            Feed::Sip => "sip",
        }
    }
}

impl Query for SnapshotsQuery {
    type Output = Snapshots;

    fn key(&self) -> String {
        format!(
            "alpaca/snapshots/{}/{}",
            self.feed_param(),
            self.symbols.join(",")
        )
    }

    fn label(&self) -> String {
        format!("Alpaca snapshots · {}", short_list(&self.symbols))
    }

    fn freshness(&self, _: &Snapshots) -> Freshness {
        Freshness::Every(if trading_hours() {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(10 * 60)
        })
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Snapshots>>,
    ) -> Result<Snapshots, FetchError> {
        let wanted: HashSet<&str> = self.symbols.iter().map(String::as_str).collect();
        let mut out = Snapshots {
            feed: self.alpaca.feed,
            by_symbol: BTreeMap::new(),
        };
        for chunk in self.symbols.chunks(SNAPSHOT_CHUNK) {
            let url = format!(
                "{}/v2/stocks/snapshots?symbols={}&feed={}",
                self.alpaca.endpoints.data,
                chunk.join(","),
                self.feed_param()
            );
            let body = ctx.get(request(&ctx, url)?).await?;
            out.by_symbol.extend(
                parse::parse_snapshots(&body)?
                    .into_iter()
                    .filter(|(s, _)| wanted.contains(s.as_str())),
            );
        }
        Ok(out)
    }
}

// --------------------------------------------------------------------- bars

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Timeframe {
    Min1,
    Min5,
    Min15,
    Hour1,
    Day1,
}

impl Timeframe {
    /// As Alpaca writes it.
    pub fn param(self) -> &'static str {
        match self {
            Self::Min1 => "1Min",
            Self::Min5 => "5Min",
            Self::Min15 => "15Min",
            Self::Hour1 => "1Hour",
            Self::Day1 => "1Day",
        }
    }

    pub fn seconds(self) -> i64 {
        match self {
            Self::Min1 => 60,
            Self::Min5 => 300,
            Self::Min15 => 900,
            Self::Hour1 => 3600,
            Self::Day1 => 86_400,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Min1 => "1-minute",
            Self::Min5 => "5-minute",
            Self::Min15 => "15-minute",
            Self::Hour1 => "hourly",
            Self::Day1 => "daily",
        }
    }
}

/// Where bars come from, and so how fresh they can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarSource {
    /// IEX, up to now.
    Iex,
    /// Every exchange, up to sixteen minutes ago (the free plan's limit).
    DelayedSip,
    /// Every exchange, up to now (a paid plan).
    Sip,
}

impl BarSource {
    pub fn from_feed(feed: Feed) -> Self {
        match feed {
            Feed::Iex => Self::Iex,
            Feed::DelayedSip => Self::DelayedSip,
            Feed::Sip => Self::Sip,
        }
    }

    fn param(self) -> &'static str {
        match self {
            Self::Iex => "iex",
            Self::DelayedSip | Self::Sip => "sip",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Iex => "IEX",
            Self::DelayedSip => "SIP 15m delayed",
            Self::Sip => "SIP",
        }
    }
}

/// How bars are adjusted for corporate actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Adjustment {
    /// For splits: prices as traded, comparable across a split. Charts use it.
    #[default]
    Split,
    /// For splits and dividends: total return, as if every dividend were
    /// reinvested. Utilities pay large ones, so BETA uses it.
    All,
}

impl Adjustment {
    fn param(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::All => "all",
        }
    }
}

/// Bars per symbol, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub struct BarSet {
    pub bars: BTreeMap<String, Vec<Bar>>,
    pub timeframe: Timeframe,
    pub source: BarSource,
    /// More pages existed than were fetched.
    pub truncated: bool,
}

impl BarSet {
    pub fn get(&self, symbol: &str) -> &[Bar] {
        self.bars.get(symbol).map_or(&[], Vec::as_slice)
    }
}

#[derive(Clone, Debug)]
pub struct BarsQuery {
    alpaca: Alpaca,
    symbols: Vec<String>,
    timeframe: Timeframe,
    start: NaiveDate,
    end: Option<NaiveDate>,
    source: BarSource,
    adjustment: Adjustment,
}

impl BarsQuery {
    pub(crate) fn new<S: AsRef<str>>(
        alpaca: Alpaca,
        symbols: impl IntoIterator<Item = S>,
        timeframe: Timeframe,
        start: NaiveDate,
        end: Option<NaiveDate>,
        source: BarSource,
    ) -> Self {
        Self {
            alpaca,
            symbols: symbol_set(symbols),
            timeframe,
            start,
            end,
            source,
            adjustment: Adjustment::Split,
        }
    }

    /// The same bars adjusted for dividends too, or only for splits again.
    pub fn adjusted(mut self, adjustment: Adjustment) -> Self {
        self.adjustment = adjustment;
        self
    }

    pub fn source(&self) -> BarSource {
        self.source
    }

    /// The window as instants: `[start, end)`.
    fn window(&self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let start = day_start(self.start);
        let mut end = self
            .end
            .map_or(now, |e| day_start(e + chrono::Duration::days(1)).min(now));
        if self.source == BarSource::DelayedSip {
            end = end.min(now - SIP_DELAY);
        }
        (start, end)
    }

    fn url(&self, start: DateTime<Utc>, end: DateTime<Utc>, page: Option<&str>) -> String {
        // The timeframe goes first so a recording can be picked by it
        // (`bars@1Day.json`; see FixtureTransport).
        let mut url = format!(
            "{}/v2/stocks/bars?timeframe={}&symbols={}&start={}&end={}&limit={PAGE_LIMIT}&adjustment={}&feed={}&sort=asc",
            self.alpaca.endpoints.data,
            self.timeframe.param(),
            self.symbols.join(","),
            rfc3339(start),
            rfc3339(end),
            self.adjustment.param(),
            self.source.param(),
        );
        if let Some(token) = page {
            url.push_str("&page_token=");
            url.push_str(&encode(token));
        }
        url
    }
}

impl Query for BarsQuery {
    type Output = BarSet;

    fn key(&self) -> String {
        format!(
            "alpaca/bars/{}/{:?}/{}/{}..{}/{}",
            self.timeframe.param(),
            self.source,
            self.adjustment.param(),
            self.start,
            self.end.map_or_else(|| "now".into(), |e| e.to_string()),
            self.symbols.join(",")
        )
    }

    fn label(&self) -> String {
        format!(
            "Alpaca {} bars · {}",
            self.timeframe.label(),
            short_list(&self.symbols)
        )
    }

    fn freshness(&self, _: &BarSet) -> Freshness {
        let past = self.end.is_some_and(|e| e < exchange_today());
        Freshness::Every(Duration::from_secs(
            match (past, self.timeframe, trading_hours()) {
                (true, _, _) => 12 * 3600,
                (false, Timeframe::Day1, true) => 30 * 60,
                (false, Timeframe::Day1, false) => 6 * 3600,
                (false, _, true) => 60,
                (false, _, false) => 30 * 60,
            },
        ))
    }

    async fn fetch(&self, ctx: FetchCtx, _prev: Option<Arc<BarSet>>) -> Result<BarSet, FetchError> {
        let mut out = BarSet {
            bars: self
                .symbols
                .iter()
                .map(|s| (s.clone(), Vec::new()))
                .collect(),
            timeframe: self.timeframe,
            source: self.source,
            truncated: false,
        };
        let (start, end) = self.window(mt_core::time::now_utc());
        if self.symbols.is_empty() || end <= start {
            return Ok(out);
        }
        let mut token: Option<String> = None;
        for page in 0.. {
            if page == MAX_PAGES {
                out.truncated = true;
                break;
            }
            let url = self.url(start, end, token.as_deref());
            let body = ctx.get(request(&ctx, url)?).await?;
            let (bars, next) = parse::parse_bars(&body)?;
            for (sym, mut list) in bars {
                if let Some(have) = out.bars.get_mut(&sym) {
                    have.append(&mut list);
                }
            }
            // A token that repeats (a replay serves the same page) ends it too.
            match next {
                Some(n) if token.as_deref() != Some(n.as_str()) => token = Some(n),
                _ => break,
            }
        }
        for list in out.bars.values_mut() {
            // Only the window asked for (a recording may hold more).
            list.retain(|b| b.time >= start && b.time < end);
            list.sort_by_key(|b| b.time);
            list.dedup_by_key(|b| b.time);
        }
        Ok(out)
    }
}

// ------------------------------------------------------------------- assets

#[derive(Clone, Debug)]
pub struct AssetsQuery {
    alpaca: Alpaca,
}

impl AssetsQuery {
    pub(crate) fn new(alpaca: Alpaca) -> Self {
        Self { alpaca }
    }
}

impl Query for AssetsQuery {
    type Output = AssetList;

    fn key(&self) -> String {
        "alpaca/assets".into()
    }

    fn label(&self) -> String {
        "Alpaca asset list".into()
    }

    fn freshness(&self, _: &AssetList) -> Freshness {
        Freshness::Every(Duration::from_secs(24 * 3600))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<AssetList>>,
    ) -> Result<AssetList, FetchError> {
        let now = mt_core::time::now_utc();
        // At launch, a list saved in the last day is good enough.
        if prev.is_none()
            && let Some(saved) = ctx
                .local_get(ASSETS_KEY)
                .await
                .and_then(|b| assets_from_bytes(&b))
            && saved.fetched.is_some_and(|t| now - t < ASSETS_REUSE)
            && !saved.assets.is_empty()
        {
            return Ok(saved);
        }
        let url = format!(
            "{}/v2/assets?status=active&asset_class=us_equity",
            self.alpaca.endpoints.trading
        );
        let body = ctx.get(request(&ctx, url)?).await?;
        let list = AssetList::new(parse::parse_assets(&body)?, Some(now));
        ctx.local_put(ASSETS_KEY, assets_to_bytes(&list)).await;
        Ok(list)
    }
}

// -------------------------------------------------------- clock and calendar

#[derive(Clone, Debug)]
pub struct ClockQuery {
    alpaca: Alpaca,
}

impl ClockQuery {
    pub(crate) fn new(alpaca: Alpaca) -> Self {
        Self { alpaca }
    }
}

impl Query for ClockQuery {
    type Output = MarketClock;

    fn key(&self) -> String {
        "alpaca/clock".into()
    }

    fn label(&self) -> String {
        "Alpaca market clock".into()
    }

    fn freshness(&self, _: &MarketClock) -> Freshness {
        Freshness::Every(Duration::from_secs(5 * 60))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<MarketClock>>,
    ) -> Result<MarketClock, FetchError> {
        let url = format!("{}/v2/clock", self.alpaca.endpoints.trading);
        parse::parse_clock(&ctx.get(request(&ctx, url)?).await?)
    }
}

#[derive(Clone, Debug)]
pub struct CalendarQuery {
    alpaca: Alpaca,
    from: NaiveDate,
    to: NaiveDate,
}

impl CalendarQuery {
    pub(crate) fn new(alpaca: Alpaca, from: NaiveDate, to: NaiveDate) -> Self {
        Self { alpaca, from, to }
    }
}

impl Query for CalendarQuery {
    type Output = Vec<TradingDay>;

    fn key(&self) -> String {
        format!("alpaca/calendar/{}/{}", self.from, self.to)
    }

    fn label(&self) -> String {
        "Alpaca market calendar".into()
    }

    fn freshness(&self, _: &Vec<TradingDay>) -> Freshness {
        Freshness::Every(Duration::from_secs(12 * 3600))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Vec<TradingDay>>>,
    ) -> Result<Vec<TradingDay>, FetchError> {
        let url = format!(
            "{}/v2/calendar?start={}&end={}",
            self.alpaca.endpoints.trading, self.from, self.to
        );
        parse::parse_calendar(&ctx.get(request(&ctx, url)?).await?)
    }
}

// --------------------------------------------------------------------- news

/// Company news, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewsList {
    pub items: Vec<Headline>,
}

#[derive(Clone, Debug)]
pub struct NewsQuery {
    alpaca: Alpaca,
    symbols: Vec<String>,
}

impl NewsQuery {
    pub(crate) fn new<S: AsRef<str>>(alpaca: Alpaca, symbols: impl IntoIterator<Item = S>) -> Self {
        Self {
            alpaca,
            symbols: symbol_set(symbols),
        }
    }

    pub fn symbols(&self) -> &[String] {
        &self.symbols
    }
}

impl Query for NewsQuery {
    type Output = NewsList;

    fn key(&self) -> String {
        format!("alpaca/news/{}", self.symbols.join(","))
    }

    fn label(&self) -> String {
        format!("Alpaca news · {}", short_list(&self.symbols))
    }

    fn freshness(&self, _: &NewsList) -> Freshness {
        Freshness::Every(Duration::from_secs(5 * 60))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<NewsList>>,
    ) -> Result<NewsList, FetchError> {
        let mut url = format!(
            "{}/v1beta1/news?limit=50&sort=desc&include_content=false",
            self.alpaca.endpoints.data
        );
        if !self.symbols.is_empty() {
            url.push_str("&symbols=");
            url.push_str(&self.symbols.join(","));
        }
        let now = mt_core::time::now_utc();
        let (fresh, _) = parse::parse_news(&ctx.get(request(&ctx, url)?).await?, now)?;
        let wanted: HashSet<&str> = self.symbols.iter().map(String::as_str).collect();
        let fresh = fresh.into_iter().filter(|h| {
            wanted.is_empty() || h.sections.iter().any(|s| wanted.contains(s.as_str()))
        });
        Ok(NewsList {
            items: merge_news(prev.as_deref().map_or(&[], |p| &p.items), fresh, NEWS_KEEP),
        })
    }
}

/// New stories merged into old by id (keeping when each was first seen),
/// newest first, at most `keep`.
pub(crate) fn merge_news(
    old: &[Headline],
    fresh: impl IntoIterator<Item = Headline>,
    keep: usize,
) -> Vec<Headline> {
    let mut out: Vec<Headline> = Vec::with_capacity(old.len() + 50);
    let mut seen = HashSet::new();
    for mut h in fresh {
        if !seen.insert(h.id.clone()) {
            continue;
        }
        if let Some(o) = old.iter().find(|o| o.id == h.id) {
            h.seen = o.seen;
        }
        out.push(h);
    }
    out.extend(old.iter().filter(|o| seen.insert(o.id.clone())).cloned());
    out.sort_by_key(|h| std::cmp::Reverse(h.time()));
    out.truncate(keep);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_data::{BoxFuture, EventLog, FetchCtxOptions, Request, Response, Transport};
    use parking_lot::Mutex;

    /// Answers from a script and records the URLs asked.
    struct Scripted {
        answers: Mutex<Vec<&'static str>>,
        asked: Mutex<Vec<String>>,
    }

    impl Transport for Scripted {
        fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
            self.asked.lock().push(req.url.clone());
            let next = self.answers.lock().pop();
            Box::pin(async move {
                next.map(Response::ok)
                    .ok_or_else(|| FetchError::Network("script ended".into()))
            })
        }
        fn describe(&self) -> String {
            "script".into()
        }
        fn is_live(&self) -> bool {
            false
        }
    }

    fn scripted(answers: &[&'static str]) -> (FetchCtx, Arc<Scripted>) {
        let mut answers = answers.to_vec();
        answers.reverse();
        let t = Arc::new(Scripted {
            answers: Mutex::new(answers),
            asked: Mutex::default(),
        });
        let ctx = FetchCtx::new(
            t.clone(),
            None,
            FetchCtxOptions {
                polite_interval: Duration::ZERO,
                ..FetchCtxOptions::default()
            },
            EventLog::default(),
        );
        (ctx, t)
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[tokio::test]
    async fn bars_follow_pages_and_stop_on_a_repeat() {
        let (ctx, t) = scripted(&[
            r#"{"bars":{"XLU":[{"t":"2026-10-01T04:00:00Z","c":80}]},"next_page_token":"a+b/c="}"#,
            r#"{"bars":{"XLU":[{"t":"2026-10-02T04:00:00Z","c":81}],"XEL":[{"t":"2026-10-02T04:00:00Z","c":70}]},"next_page_token":null}"#,
        ]);
        let q = Alpaca::default().bars(
            ["xel", "XLU", "XLU"],
            Timeframe::Day1,
            day("2026-09-01"),
            Some(day("2026-10-02")),
        );
        let set = q.fetch(ctx, None).await.unwrap();
        assert_eq!(set.get("XLU").len(), 2);
        assert_eq!(set.get("XEL").len(), 1);
        assert!(!set.truncated);
        let asked = t.asked.lock().clone();
        assert_eq!(asked.len(), 2);
        assert!(
            asked[0].contains("timeframe=1Day&symbols=XEL,XLU&"),
            "{}",
            asked[0]
        );
        assert!(
            asked[0].contains("feed=sip"),
            "daily bars from every exchange"
        );
        assert!(
            asked[1].ends_with("&page_token=a%2Bb%2Fc%3D"),
            "{}",
            asked[1]
        );

        // A replay answers every page with the same token: stop after two.
        let page =
            r#"{"bars":{"XLU":[{"t":"2026-10-01T04:00:00Z","c":80}]},"next_page_token":"same"}"#;
        let (ctx, t) = scripted(&[page, page, page]);
        let set = q.fetch(ctx, None).await.unwrap();
        assert_eq!(set.get("XLU").len(), 1, "de-duplicated by time");
        assert_eq!(t.asked.lock().len(), 2);
    }

    #[test]
    fn total_return_bars_ask_for_every_adjustment() {
        let a = Alpaca::default();
        let split = a.bars(["XLU"], Timeframe::Day1, day("2025-10-01"), None);
        let all = split.clone().adjusted(Adjustment::All);
        let now = parse::parse_time("2026-10-02T18:00:00Z").unwrap();
        let (start, end) = all.window(now);
        assert!(split.url(start, end, None).contains("&adjustment=split&"));
        assert!(all.url(start, end, None).contains("&adjustment=all&"));
        assert_ne!(split.key(), all.key(), "cached apart");
        assert_eq!(all.adjusted(Adjustment::Split).key(), split.key());
    }

    #[test]
    fn delayed_bars_end_sixteen_minutes_ago() {
        let a = Alpaca::new(
            &crate::MarketsConfig {
                feed: Feed::DelayedSip,
                ..crate::MarketsConfig::default()
            },
            true,
        );
        let q = a.bars(["XLU"], Timeframe::Min1, day("2026-10-02"), None);
        let now = parse::parse_time("2026-10-02T18:00:00Z").unwrap();
        let (start, end) = q.window(now);
        assert_eq!(
            rfc3339(start),
            "2026-10-02T04:00:00Z",
            "midnight in New York"
        );
        assert_eq!(rfc3339(end), "2026-10-02T17:44:00Z");
        let iex = Alpaca::default().bars(
            ["XLU"],
            Timeframe::Min1,
            day("2026-10-02"),
            Some(day("2026-10-02")),
        );
        assert_eq!(
            rfc3339(iex.window(now).1),
            "2026-10-02T18:00:00Z",
            "never past now"
        );
        assert_ne!(q.key(), iex.key());
    }

    #[tokio::test]
    async fn snapshots_keep_only_what_was_asked_and_news_merges() {
        let (ctx, _) = scripted(&[
            r#"{"XLU":{"latestTrade":{"t":"2026-10-02T21:00:00Z","p":82}},"SPY":{"latestTrade":{"t":"2026-10-02T21:00:00Z","p":600}}}"#,
        ]);
        let s = Alpaca::default()
            .snapshots(["XLU"])
            .fetch(ctx, None)
            .await
            .unwrap();
        assert_eq!(s.by_symbol.keys().collect::<Vec<_>>(), ["XLU"]);

        let news = r#"{"news":[{"id":2,"headline":"XLU story","created_at":"2026-10-02T12:00:00Z","url":"https://b/2","symbols":["XLU"],"source":"benzinga"},
                                {"id":3,"headline":"Other","created_at":"2026-10-02T13:00:00Z","url":"https://b/3","symbols":["SPY"],"source":"benzinga"}],"next_page_token":null}"#;
        let (ctx, t) = scripted(&[news]);
        let q = Alpaca::default().news(["XLU"]);
        let old = NewsList {
            items: parse::parse_news(
                br#"{"news":[{"id":1,"headline":"Old","created_at":"2026-10-01T12:00:00Z","url":"https://b/1","symbols":["XLU"],"source":"benzinga"}]}"#,
                Utc::now(),
            )
            .unwrap()
            .0,
        };
        let got = q.fetch(ctx, Some(Arc::new(old))).await.unwrap();
        let ids: Vec<&str> = got.items.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            ["alpaca:2", "alpaca:1"],
            "filtered to XLU, merged, newest first"
        );
        assert!(t.asked.lock()[0].contains("symbols=XLU"));
        assert!(
            t.asked.lock()[0].contains("include_content=false"),
            "never the article text"
        );
    }
}
