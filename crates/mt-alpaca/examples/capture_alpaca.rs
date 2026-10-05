//! Record Alpaca's responses (offline mode, parser tests, the weekly drift
//! job). About fifteen requests and three short stream sessions, and with
//! `--verbatim` the paper account too (about ten more requests and a session
//! of its order events).
//!
//!     cargo run -p mt-alpaca --example capture_alpaca [--verbatim] [DIR]
//!
//! Needs a paper account's keys: `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY`
//! in the environment, or the ones stored in SET (Windows Credential Manager).
//! The requests are the app's own (each dataset's query runs through a
//! recording transport), so the recording is what the app would see.
//!
//! Market data is licensed to the account holder and stories belong to their
//! publisher, so by default every price, size and volume is replaced by a
//! synthetic one (a smooth function of the symbol and the time, consistent
//! across files) and every story's words by sample text, keeping the
//! structure: fields, timestamps, venues, conditions, ids. The asset list is
//! trimmed to the symbols the fixtures use. That is what goes in the
//! repository's `fixtures/`. `--verbatim` keeps everything, for the drift
//! job, which parses a live recording and throws it away.
//!
//! The account (balances, positions, orders, equity curve, activities, order events)
//! is only recorded with `--verbatim`: the repository's account fixtures are a
//! made-up portfolio from `tools/sample_account.py`, never an account's own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use mt_alpaca::{Alpaca, HistoryPeriod, MarketsConfig, Timeframe};
use mt_core::equity::Feed;
use mt_core::exchange;
use mt_data::{
    Applied, BoxFuture, EventLog, FetchCtx, FetchCtxOptions, FetchError, FixtureTransport, Frame,
    HttpTransport, MemorySecrets, Query, Request, Response, Secret, SecretStore, Stream,
    StreamConn, Transport,
};
use parking_lot::Mutex;
use serde_json::Value;

/// Symbols with one-minute bars (GP's intraday view offline).
const MINUTE: &[&str] = &["XLU", "XEL", "VST", "CEG"];
/// Symbols with daily bars (GP history, DES).
const DAILY: &[&str] = &["XLU", "XEL", "VST", "CEG", "AEE", "UNG", "SPY"];
/// Extra symbols beside the built-in lists, for snapshots and the asset list.
const EXTRA: &[&str] = &["SPY", "QQQ"];
/// Well-known names kept in the trimmed asset list, for completion offline.
const ASSET_EXTRA: &[&str] = &[
    "AAPL", "AEP", "AMZN", "BRK.B", "CVX", "D", "DIA", "DUK", "ED", "EIX", "EXC", "GOOGL", "IWM",
    "JPM", "META", "MSFT", "NEE", "NVDA", "PCG", "PEG", "SO", "SRE", "TSLA", "VPU", "XOM",
];
/// How long each stream session records.
const STREAM_SECS: u64 = 15;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let verbatim = args.iter().any(|a| a == "--verbatim");
    args.retain(|a| a != "--verbatim");
    let out = PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures").to_owned()),
    );

    let recorder = Arc::new(Recorder::new(&out)?);
    let ctx = FetchCtx::new(
        recorder.clone(),
        None,
        FetchCtxOptions {
            polite_interval: Duration::ZERO,
            budgets: mt_alpaca::budgets(),
            secrets: keys()?,
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let alpaca = Alpaca::new(&MarketsConfig::default(), true);
    // The default list holds every built-in list's symbols.
    let lists: Vec<String> = MarketsConfig::default()
        .list(mt_alpaca::config::DEFAULT_LIST)
        .ok_or("no default list")?
        .securities()
        .into_iter()
        .map(|s| s.ticker)
        .collect();
    let mut quoted = lists.clone();
    quoted.extend(EXTRA.iter().map(|s| (*s).to_owned()));

    // The calendar says which sessions to ask for.
    let now = Utc::now();
    let today = exchange::to_exchange(now).date_naive();
    let calendar = run(&ctx, alpaca.calendar(today)).await?;
    // Sessions whose regular hours have begun (the weekly job runs on Monday
    // mornings, when that day's pre-market alone would make a thin recording).
    let sessions: Vec<NaiveDate> = calendar
        .iter()
        .filter(|d| d.regular_utc().is_some_and(|(open, _)| open <= now))
        .map(|d| d.date)
        .collect();
    let latest = *sessions.last().ok_or("no recent session in the calendar")?;
    let back = |n: usize| sessions[sessions.len().saturating_sub(n + 1)];

    run(&ctx, alpaca.clock()).await?;
    run(&ctx, alpaca.assets()).await?;
    run(&ctx, alpaca.snapshots(&quoted)).await?;
    let delayed = Alpaca::new(
        &MarketsConfig {
            feed: Feed::DelayedSip,
            ..MarketsConfig::default()
        },
        true,
    );
    run(&ctx, delayed.snapshots(&quoted)).await?;
    run(&ctx, alpaca.bars(MINUTE, Timeframe::Min1, latest, None)).await?;
    run(&ctx, alpaca.bars(&quoted, Timeframe::Min15, latest, None)).await?;
    run(&ctx, alpaca.bars(MINUTE, Timeframe::Min15, back(5), None)).await?;
    run(
        &ctx,
        alpaca.bars(
            DAILY,
            Timeframe::Day1,
            today - chrono::Duration::days(400),
            None,
        ),
    )
    .await?;
    let news = run(&ctx, alpaca.news(&lists)).await?;
    println!("{} stories", news.items.len());

    // Streams: the live feed with every list symbol (the free plan's limit is
    // 30), Alpaca's always-on test feed, and news.
    let topics = mt_alpaca::market_topics(&lists);
    let live = alpaca.market_stream();
    recorder.stream(&ctx, &live, &topics).await?;
    let test = mt_alpaca::MarketStream::test_feed(alpaca.endpoints());
    recorder
        .stream(&ctx, &test, &mt_alpaca::market_topics(["FAKEPACA"]))
        .await?;
    recorder
        .stream(&ctx, &alpaca.news_stream(), &["news:*".to_owned()])
        .await?;

    if verbatim {
        // The paper account, read-only, for the drift job's parsers.
        run(&ctx, alpaca.account()).await?;
        let positions = run(&ctx, alpaca.positions()).await?;
        println!("{} positions", positions.len());
        for period in HistoryPeriod::ALL {
            run(&ctx, alpaca.portfolio_history(period)).await?;
        }
        run(&ctx, alpaca.activities()).await?;
        let orders = run(&ctx, alpaca.orders()).await?;
        println!("{} orders", orders.len());
        let options: Vec<String> = positions
            .iter()
            .filter(|p| p.is_option())
            .map(|p| p.symbol.clone())
            .collect();
        if !options.is_empty() {
            run(&ctx, alpaca.option_snapshots(&options)).await?;
        }
        recorder
            .stream(
                &ctx,
                &alpaca.trade_stream(),
                &[mt_alpaca::TRADE_UPDATES.to_owned()],
            )
            .await?;
    }

    let symbols: Vec<String> = quoted
        .iter()
        .cloned()
        .chain(ASSET_EXTRA.iter().map(|s| (*s).to_owned()))
        .collect();
    recorder.write(verbatim, &symbols)?;
    Ok(())
}

/// Keys from the environment, else those stored in SET.
fn keys() -> Result<Arc<dyn SecretStore>, Box<dyn std::error::Error>> {
    match (
        std::env::var("APCA_API_KEY_ID"),
        std::env::var("APCA_API_SECRET_KEY"),
    ) {
        (Ok(id), Ok(secret)) if !id.trim().is_empty() && !secret.trim().is_empty() => {
            Ok(Arc::new(MemorySecrets::with([
                (mt_alpaca::KEY_ID, Secret::new(id.trim())),
                (mt_alpaca::SECRET_KEY, Secret::new(secret.trim())),
            ])))
        }
        _ => {
            let store = mt_data::os_store("miso-terminal");
            if !mt_alpaca::has_keys(store.as_ref()) {
                return Err("no Alpaca keys: set APCA_API_KEY_ID and APCA_API_SECRET_KEY, or store them in SET".into());
            }
            Ok(store)
        }
    }
}

async fn run<Q: Query>(ctx: &FetchCtx, q: Q) -> Result<Arc<Q::Output>, Box<dyn std::error::Error>> {
    let label = q.label();
    match q.fetch(ctx.clone(), None).await {
        Ok(v) => {
            println!("ok      {label}");
            Ok(Arc::new(v))
        }
        Err(e) => Err(format!("{label}: {e}").into()),
    }
}

/// The network, with every successful answer kept for writing out.
struct Recorder {
    inner: HttpTransport,
    fixtures: FixtureTransport,
    root: PathBuf,
    kept: Mutex<BTreeMap<PathBuf, Kept>>,
}

enum Kept {
    Json(Value),
    Frames(Vec<String>),
}

impl Recorder {
    fn new(root: &Path) -> Result<Self, FetchError> {
        Ok(Self {
            inner: HttpTransport::new(
                "MISO-Terminal fixture recorder (github.com/ArenKDesai/miso-terminal)",
            )?,
            fixtures: FixtureTransport::new(root),
            root: root.to_owned(),
            kept: Mutex::default(),
        })
    }

    /// Where a response is kept: bars by timeframe (`bars@1Day.json`),
    /// snapshots by feed when not IEX, everything else by its path.
    fn name_for(&self, url: &str) -> PathBuf {
        let path = self.fixtures.path_for(url);
        let param = |k: &str| {
            url.split_once('?').map(|(_, q)| q).and_then(|q| {
                q.split('&')
                    .find_map(|p| p.strip_prefix(&format!("{k}=")).map(str::to_owned))
            })
        };
        let variant = match path.file_name().and_then(|n| n.to_str()) {
            Some("bars.json") => param("timeframe"),
            Some("snapshots.json") if url.contains("/v2/stocks/") => {
                param("feed").filter(|f| f != "iex")
            }
            Some("history.json") => param("period"),
            _ => None,
        };
        match variant {
            Some(v) => path.with_file_name(format!(
                "{}@{v}.json",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("data")
            )),
            None => path,
        }
    }

    fn keep(&self, url: &str, body: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(body) else {
            eprintln!("not JSON: {url}");
            return;
        };
        let path = self.name_for(url);
        let mut kept = self.kept.lock();
        // Pages and repeat requests of bars merge per symbol.
        if let Some(Kept::Json(old)) = kept.get_mut(&path)
            && let (Some(Value::Object(have)), Some(Value::Object(add))) =
                (old.get_mut("bars"), value.get("bars"))
        {
            for (sym, list) in add {
                let entry = have
                    .entry(sym.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let (Value::Array(a), Value::Array(b)) = (entry, list) {
                    a.extend(b.iter().cloned());
                    a.sort_by(|x, y| x["t"].as_str().cmp(&y["t"].as_str()));
                    a.dedup_by(|x, y| x["t"] == y["t"]);
                }
            }
            return;
        }
        let mut value = value;
        if value.get("next_page_token").is_some() {
            value["next_page_token"] = Value::Null;
        }
        kept.insert(path, Kept::Json(value));
    }

    /// Connect, log in, subscribe and keep the server's frames for a while.
    async fn stream<S: Stream>(
        &self,
        ctx: &FetchCtx,
        s: &S,
        topics: &[String],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let req = s.request(ctx)?;
        let mut conn = self.inner.connect(&req).await?;
        for m in s.hello(ctx)? {
            conn.send(m).await?;
        }
        let (mut state, mut ready, mut frames) = (S::State::default(), false, Vec::new());
        let deadline = Instant::now() + Duration::from_secs(STREAM_SECS);
        while let Ok(Some(frame)) =
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), conn.recv()).await
        {
            let frame = frame?;
            match &frame {
                Frame::Text(t) => frames.push(t.clone()),
                // The account's order events come in binary frames.
                Frame::Binary(b) => frames.push(String::from_utf8_lossy(b).into_owned()),
                Frame::Pong => {}
            }
            match s.apply(&mut state, &frame) {
                Ok(Applied::Ready) if !ready => {
                    ready = true;
                    for m in s.subscribe(topics) {
                        conn.send(m).await?;
                    }
                }
                Err(e @ FetchError::Parse { .. }) => eprintln!("{}: {e}", s.label()),
                Err(e) => return Err(format!("{}: {e}", s.label()).into()),
                _ => {}
            }
        }
        conn.close().await;
        if !ready {
            return Err(format!("{}: never logged in", s.label()).into());
        }
        println!("ok      {} ({} frames)", s.label(), frames.len());
        let path = self.fixtures.stream_path_for(&req.url);
        self.kept.lock().insert(path, Kept::Frames(frames));
        Ok(())
    }

    fn write(&self, verbatim: bool, symbols: &[String]) -> std::io::Result<()> {
        for (path, kept) in self.kept.lock().iter() {
            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let body = match kept {
                Kept::Json(v) => {
                    let mut v = v.clone();
                    if !verbatim {
                        sample::json(&rel, &mut v, symbols);
                    }
                    serde_json::to_string(&v).unwrap_or_default()
                }
                Kept::Frames(frames) => frames
                    .iter()
                    .map(|f| {
                        if verbatim || rel.ends_with("v2/test.jsonl") {
                            return f.clone();
                        }
                        match serde_json::from_str::<Value>(f) {
                            Ok(mut v) => {
                                sample::frame(&mut v);
                                v.to_string()
                            }
                            Err(_) => f.clone(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, body.as_bytes())?;
            println!("{:>6} KB  {}", body.len() / 1024, path.display());
        }
        Ok(())
    }
}

impl Transport for Recorder {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        Box::pin(async move {
            let resp = self.inner.send(req).await?;
            if resp.is_success() {
                self.keep(&req.url, &resp.body);
            } else {
                eprintln!("HTTP {} {}: {}", resp.status, req.url, resp.excerpt(300));
            }
            Ok(resp)
        })
    }

    fn connect<'a>(
        &'a self,
        req: &'a Request,
    ) -> BoxFuture<'a, Result<Box<dyn StreamConn>, FetchError>> {
        self.inner.connect(req)
    }

    fn describe(&self) -> String {
        "recording Alpaca".into()
    }
}

/// Synthetic prices and sample words in a recording's structure.
mod sample {
    use super::*;

    fn hash(s: &str) -> u64 {
        s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    fn unit(s: &str) -> f64 {
        (hash(s) % 10_000) as f64 / 10_000.0
    }

    /// A smooth, made-up price for `sym` at `t`: a level from the symbol's
    /// name, a slow drift, a daily swing and some intraday wiggle.
    pub fn price(sym: &str, t: DateTime<Utc>) -> f64 {
        let x = t.timestamp() as f64;
        let days = x / 86_400.0;
        let phase = unit(sym) * std::f64::consts::TAU;
        let base = 20.0 + unit(&format!("{sym}/base")) * 160.0;
        let shape = 1.0
            + 0.10 * (days / 41.0 + phase).sin()
            + 0.04 * (days / 9.0 + 2.0 * phase).sin()
            + 0.012 * (days * 1.9 + phase).sin()
            + 0.004 * (x / 1_900.0 + phase).sin()
            + 0.002 * (x / 470.0 + 3.0 * phase).sin()
            + 0.0015 * (unit(&format!("{sym}/{}", t.timestamp() / 60)) - 0.5);
        round(base * shape)
    }

    fn round(p: f64) -> f64 {
        if p < 1.0 {
            (p * 10_000.0).round() / 10_000.0
        } else {
            (p * 100.0).round() / 100.0
        }
    }

    /// A whole number of shares (or round lots, for quotes).
    fn size(sym: &str, t: &str, scale: f64) -> u64 {
        (1.0 + unit(&format!("{sym}/{t}/size")) * scale).round() as u64
    }

    fn time(v: &Value) -> Option<DateTime<Utc>> {
        v.get("t")
            .and_then(Value::as_str)
            .and_then(mt_alpaca::parse::parse_time)
    }

    pub fn trade(sym: &str, v: &mut Value) {
        let Some(t) = time(v) else { return };
        let ts = t.to_rfc3339();
        v["p"] = price(sym, t).into();
        if v.get("s").is_some() {
            v["s"] = size(sym, &ts, 400.0).into();
        }
    }

    pub fn quote(sym: &str, v: &mut Value) {
        let Some(t) = time(v) else { return };
        let ts = t.to_rfc3339();
        let mid = price(sym, t);
        let half = round((mid * 0.0004).max(0.01));
        for (k, val) in [("bp", mid - half), ("ap", mid + half)] {
            if v.get(k).and_then(Value::as_f64).is_some_and(|p| p > 0.0) {
                v[k] = round(val).into();
            }
        }
        for k in ["bs", "as"] {
            if v.get(k).is_some() {
                v[k] = size(sym, &format!("{ts}{k}"), 8.0).into();
            }
        }
    }

    /// A bar of `secs` from `t`; daily bars cover the regular session.
    pub fn bar(sym: &str, v: &mut Value, secs: i64) {
        let Some(t) = time(v) else { return };
        let (from, to) = if secs >= 86_400 {
            let day = exchange::to_exchange(t).date_naive();
            let at = |h, m| {
                exchange::exchange_to_utc(day.and_hms_opt(h, m, 0).unwrap_or_default()).unwrap_or(t)
            };
            (at(9, 30), at(16, 0))
        } else {
            (t, t + chrono::Duration::seconds(secs - 1))
        };
        let steps = 8;
        let samples: Vec<f64> = (0..=steps)
            .map(|i| price(sym, from + (to - from) * i / steps))
            .collect();
        let (open, close) = (samples[0], samples[steps as usize]);
        let hi = samples.iter().copied().fold(f64::MIN, f64::max);
        let lo = samples.iter().copied().fold(f64::MAX, f64::min);
        let volume = (unit(&format!("{sym}/vol")) * 4_000_000.0 + 200_000.0)
            * (secs as f64 / 23_400.0).min(1.0)
            * (0.5 + unit(&format!("{sym}/{}", t.timestamp())));
        v["o"] = open.into();
        v["c"] = close.into();
        v["h"] = round(hi).into();
        v["l"] = round(lo).into();
        if v.get("vw").is_some() {
            v["vw"] = (((hi + lo + close) / 3.0 * 10_000.0).round() / 10_000.0).into();
        }
        if v.get("v").is_some() {
            v["v"] = (volume.round().max(1.0) as u64).into();
        }
        if v.get("n").is_some() {
            v["n"] = ((volume / 150.0).round().max(1.0) as u64).into();
        }
    }

    const SUBJECTS: &[&str] = &[
        "Utility shares",
        "Power producers",
        "Gas producers",
        "A Midwest utility",
        "Energy ETFs",
        "Analysts",
        "Data center demand",
        "A rate case",
    ];
    const PREDICATES: &[&str] = &[
        "edge higher in early trade",
        "slip after a downgrade",
        "draw an upgrade",
        "hold steady ahead of earnings",
        "rally on capacity prices",
        "weigh a new pipeline",
        "face a busy week",
        "set a dividend",
    ];

    pub fn story(v: &mut Value) {
        let id = v.get("id").map(Value::to_string).unwrap_or_default();
        let h = hash(&id);
        let subject = SUBJECTS[(h % SUBJECTS.len() as u64) as usize];
        let predicate = PREDICATES[(h / 97 % PREDICATES.len() as u64) as usize];
        v["headline"] = format!("Sample: {subject} {predicate}").into();
        if v.get("summary")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        {
            v["summary"] = format!(
                "Sample summary for a test fixture: {} {predicate}. The publisher's text is not kept here.",
                subject.to_lowercase()
            )
            .into();
        }
        if v.get("author").is_some() {
            v["author"] = "Sample Reporter".into();
        }
        if let Some(url) = v.get("url").and_then(Value::as_str) {
            v["url"] = sample_url(url).into();
        }
        if v.get("content").is_some() {
            v["content"] = "".into();
        }
        if v.get("images").is_some() {
            v["images"] = Value::Array(Vec::new());
        }
    }

    /// Wordy path segments (`/utilities-rally-on-capacity-prices`) become
    /// `sample-story-<n>`; ids and dates stay.
    fn sample_url(url: &str) -> String {
        url.split('/')
            .map(|seg| {
                if seg.contains('-') && seg.chars().any(|c| c.is_ascii_alphabetic()) {
                    format!("sample-story-{}", hash(seg) % 100_000)
                } else {
                    seg.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// One recorded file, by what it is.
    pub fn json(rel: &str, v: &mut Value, keep_assets: &[String]) {
        if rel.contains("/v2/stocks/snapshots") {
            let map = if v.get("snapshots").is_some() {
                &mut v["snapshots"]
            } else {
                v
            };
            if let Some(obj) = map.as_object_mut() {
                for (sym, s) in obj.iter_mut() {
                    for (k, secs) in [
                        ("minuteBar", 60),
                        ("dailyBar", 86_400),
                        ("prevDailyBar", 86_400),
                    ] {
                        if let Some(b) = s.get_mut(k).filter(|b| b.is_object()) {
                            bar(sym, b, secs);
                        }
                    }
                    if let Some(t) = s.get_mut("latestTrade").filter(|b| b.is_object()) {
                        trade(sym, t);
                    }
                    if let Some(q) = s.get_mut("latestQuote").filter(|b| b.is_object()) {
                        quote(sym, q);
                    }
                }
            }
        } else if rel.contains("/v2/stocks/bars") {
            let secs = match rel
                .rsplit_once('@')
                .map(|(_, v)| v.trim_end_matches(".json"))
            {
                Some("1Min") => 60,
                Some("5Min") => 300,
                Some("15Min") => 900,
                Some("1Hour") => 3600,
                _ => 86_400,
            };
            if let Some(per) = v.get_mut("bars").and_then(Value::as_object_mut) {
                for (sym, list) in per.iter_mut() {
                    for b in list.as_array_mut().into_iter().flatten() {
                        bar(sym, b, secs);
                    }
                }
            }
        } else if rel.contains("/v1beta1/news") {
            for n in v
                .get_mut("news")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                story(n);
            }
        } else if rel.ends_with("/v2/assets.json")
            && let Some(list) = v.as_array_mut()
        {
            list.retain(|a| {
                a.get("symbol")
                    .and_then(Value::as_str)
                    .is_some_and(|s| keep_assets.iter().any(|k| k == s))
            });
        }
    }

    /// One recorded stream frame.
    pub fn frame(v: &mut Value) {
        for m in v.as_array_mut().into_iter().flatten() {
            let sym = m
                .get("S")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            match m.get("T").and_then(Value::as_str) {
                Some("t") => trade(&sym, m),
                Some("q") => quote(&sym, m),
                Some("b" | "u") => bar(&sym, m, 60),
                Some("d") => bar(&sym, m, 86_400),
                Some("n") => story(m),
                _ => {}
            }
        }
    }
}
