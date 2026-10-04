//! Alpaca's WebSocket streams: live trades, quotes and minute bars, and news
//! as it is published.
//!
//! Every frame is a JSON array of messages tagged by `T`. After connecting
//! the server says `connected`; the login (`{"action":"auth",…}`) is answered
//! `authenticated` or an error; subscriptions are acknowledged with the full
//! list per channel. The free plan allows 30 trade and quote subscriptions in
//! all (a symbol's trades count one, its quotes another; more is refused with
//! a `405`) and minute bars for any number. So [`MarketStream`] subscribes
//! every symbol's trades first, then quotes while room remains, and holds the
//! rest back (they still get bars) until room frees up. Should the server
//! refuse anyway, it halves its limit and reconnects.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use mt_core::equity::{Bar, Feed, Quote, Trade};
use mt_core::news::Headline;
use mt_data::{Applied, FetchCtx, FetchError, Frame, Request, Stream};
use parking_lot::Mutex;
use serde_json::Value;

use crate::{KEY_ID, SECRET_KEY, parse};

/// Headlines a news stream keeps.
const LIVE_NEWS_KEEP: usize = 300;
/// The server's code for too many trade and quote subscriptions.
const SYMBOL_LIMIT: u64 = 405;

/// The topics for live prices of `symbols`: trades, quotes and minute bars.
pub fn market_topics<S: AsRef<str>>(symbols: impl IntoIterator<Item = S>) -> Vec<String> {
    symbols
        .into_iter()
        .filter_map(|s| crate::normalize_symbol(s.as_ref()))
        .flat_map(|s| {
            ["trades", "quotes", "bars"]
                .into_iter()
                .map(move |ch| format!("{ch}:{s}"))
        })
        .collect()
}

fn split_topic(topic: &str) -> Option<(&str, &str)> {
    topic
        .split_once(':')
        .filter(|(c, s)| !c.is_empty() && !s.is_empty())
}

/// The login message, or nothing for a replay.
fn login(ctx: &FetchCtx) -> Result<Vec<String>, FetchError> {
    if !ctx.is_live() {
        return Ok(Vec::new());
    }
    let (key, secret) = (ctx.secret(KEY_ID)?, ctx.secret(SECRET_KEY)?);
    Ok(vec![
        serde_json::json!({"action": "auth", "key": key.expose(), "secret": secret.expose()})
            .to_string(),
    ])
}

fn action(action: &str, channels: &BTreeMap<&str, Vec<String>>) -> Option<String> {
    let mut msg = serde_json::Map::new();
    msg.insert("action".into(), action.into());
    for (ch, syms) in channels.iter().filter(|(_, s)| !s.is_empty()) {
        msg.insert((*ch).into(), syms.clone().into());
    }
    (msg.len() > 1).then(|| Value::Object(msg).to_string())
}

/// What a control message means for the connection.
enum Control {
    Ready,
    Notice(String),
}

/// `success`/`error` messages. Refused logins and a plan without the feed
/// stop the stream; a connection another program holds retries later.
fn control(m: &Value, feed_name: &str) -> Result<Option<Control>, FetchError> {
    let msg = m.get("msg").and_then(Value::as_str).unwrap_or_default();
    match m.get("T").and_then(Value::as_str) {
        Some("success") if msg == "authenticated" => Ok(Some(Control::Ready)),
        Some("success") => Ok(None),
        Some("error") => {
            let code = m.get("code").and_then(Value::as_u64).unwrap_or(0);
            let what = format!("Alpaca {feed_name} stream: {msg} ({code})");
            match code {
                // not authenticated, auth failed, insufficient subscription
                401 | 402 | 409 => Err(FetchError::Auth(what)),
                406 => Err(FetchError::Network(format!(
                    "{what}; another program may be using this account's stream"
                ))),
                404 => Err(FetchError::Network(what)),
                _ => Ok(Some(Control::Notice(what))),
            }
        }
        _ => Ok(None),
    }
}

// ------------------------------------------------------------------- market

/// Live prices: each symbol's latest trade, quote and minute bar.
#[derive(Clone, Debug, Default)]
pub struct LiveMarket {
    pub trades: HashMap<String, Trade>,
    pub quotes: HashMap<String, Quote>,
    pub bars: HashMap<String, Bar>,
    /// Symbols whose every trade streams (the server's confirmation); the
    /// rest update with minute bars.
    pub ticking: BTreeSet<String>,
    /// The server's latest complaint (a subscription limit, say), if any.
    pub notice: Option<String>,
}

/// A trade or quote subscription: `("trades", "XLU")`.
type Pair = (String, String);

/// Trade and quote subscriptions sent, against the plan's limit.
#[derive(Debug)]
struct Wire {
    /// How many trade and quote subscriptions may be open (halved if the
    /// server says the limit is exceeded anyway).
    cap: usize,
    sent: BTreeSet<Pair>,
    /// Subscriptions held back by the limit.
    held: BTreeSet<Pair>,
}

impl Wire {
    fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            sent: BTreeSet::new(),
            held: BTreeSet::new(),
        }
    }

    /// Trades come before quotes: a symbol's last price matters more than
    /// its bid and ask, which the minute snapshots carry anyway.
    fn priority(p: &Pair) -> (bool, String) {
        (p.0 != "trades", p.1.clone())
    }

    /// Send what fits, trades first; a trade may displace a quote. Returns
    /// the pairs to subscribe and the quotes displaced (to unsubscribe).
    fn admit(&mut self, pairs: impl IntoIterator<Item = Pair>) -> (Vec<Pair>, Vec<Pair>) {
        let mut pairs: Vec<Pair> = pairs.into_iter().collect();
        pairs.sort_by_key(Self::priority);
        let (mut add, mut evict) = (Vec::new(), Vec::new());
        for p in pairs {
            if self.sent.contains(&p) {
                continue;
            }
            if self.sent.len() >= self.cap && p.0 == "trades" {
                let quote = self.sent.iter().rev().find(|(c, _)| c == "quotes").cloned();
                if let Some(q) = quote {
                    self.sent.remove(&q);
                    self.held.insert(q.clone());
                    evict.push(q);
                }
            }
            if self.sent.len() < self.cap {
                self.held.remove(&p);
                self.sent.insert(p.clone());
                add.push(p);
            } else {
                self.held.insert(p);
            }
        }
        (add, evict)
    }
}

fn messages(action_name: &str, pairs: &[Pair], bars: &[String]) -> Option<String> {
    let mut channels: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (ch, sym) in pairs {
        let ch = if ch == "trades" { "trades" } else { "quotes" };
        channels.entry(ch).or_default().push(sym.clone());
    }
    if !bars.is_empty() {
        channels.insert("bars", bars.to_vec());
    }
    action(action_name, &channels)
}

/// `wss://stream.data.alpaca.markets/v2/<feed>`.
#[derive(Clone, Debug)]
pub struct MarketStream {
    url: String,
    feed: Feed,
    wire: Arc<Mutex<Wire>>,
}

impl MarketStream {
    /// `limit`: trade and quote subscriptions allowed at once.
    pub(crate) fn new(url: String, feed: Feed, limit: usize) -> Self {
        Self {
            url,
            feed,
            wire: Arc::new(Mutex::new(Wire::new(limit))),
        }
    }

    /// Alpaca's test feed: made-up trades and quotes for `FAKEPACA`, around
    /// the clock (the recorder uses it to check the message formats).
    pub fn test_feed(endpoints: &crate::Endpoints) -> Self {
        Self::new(
            format!("{}/v2/test", endpoints.stream),
            Feed::Iex,
            crate::config::FREE_PLAN_STREAM_LIMIT,
        )
    }

    /// The feed's name in the URL: `iex`, `delayed_sip`, `sip` or `test`.
    fn endpoint(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or("iex")
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn feed(&self) -> Feed {
        self.feed
    }
}

impl Stream for MarketStream {
    type State = LiveMarket;

    fn key(&self) -> String {
        format!("alpaca/stream/{}", self.endpoint())
    }

    fn label(&self) -> String {
        match self.endpoint() {
            "test" => "Alpaca test feed".into(),
            _ => format!("Alpaca live prices ({})", self.feed.label()),
        }
    }

    fn request(&self, _ctx: &FetchCtx) -> Result<Request, FetchError> {
        Ok(Request::get(&self.url))
    }

    fn hello(&self, ctx: &FetchCtx) -> Result<Vec<String>, FetchError> {
        login(ctx)
    }

    fn waits_for_ready(&self) -> bool {
        true
    }

    fn subscribe(&self, topics: &[String]) -> Vec<String> {
        let mut bars = Vec::new();
        let mut pairs = Vec::new();
        for (ch, sym) in topics.iter().filter_map(|t| split_topic(t)) {
            match ch {
                "bars" => bars.push(sym.to_owned()),
                "trades" | "quotes" => pairs.push((ch.to_owned(), sym.to_owned())),
                _ => {}
            }
        }
        let (add, evict) = self.wire.lock().admit(pairs);
        messages("unsubscribe", &evict, &[])
            .into_iter()
            .chain(messages("subscribe", &add, &bars))
            .collect()
    }

    fn unsubscribe(&self, topics: &[String]) -> Vec<String> {
        let mut wire = self.wire.lock();
        let mut bars = Vec::new();
        let mut dropped = Vec::new();
        for (ch, sym) in topics.iter().filter_map(|t| split_topic(t)) {
            let pair = (ch.to_owned(), sym.to_owned());
            match ch {
                "bars" => bars.push(sym.to_owned()),
                "trades" | "quotes" if wire.sent.remove(&pair) => dropped.push(pair),
                _ => {
                    wire.held.remove(&pair);
                }
            }
        }
        // Room may have freed up for what was held back.
        let held: Vec<Pair> = wire.held.iter().cloned().collect();
        let (add, evict) = wire.admit(held);
        dropped.extend(evict);
        messages("unsubscribe", &dropped, &bars)
            .into_iter()
            .chain(messages("subscribe", &add, &[]))
            .collect()
    }

    fn apply(&self, state: &mut LiveMarket, frame: &Frame) -> Result<Applied, FetchError> {
        let Some(text) = frame.as_text() else {
            return Ok(Applied::Ignored);
        };
        let mut result = Applied::Ignored;
        let changed = |r: &mut Applied| {
            if *r == Applied::Ignored {
                *r = Applied::Changed;
            }
        };
        for m in parse::stream_messages(text)? {
            let sym = m
                .get("S")
                .and_then(Value::as_str)
                .map(str::to_ascii_uppercase);
            match (m.get("T").and_then(Value::as_str), sym) {
                (Some("t"), Some(sym)) => {
                    if let Some(t) = parse::trade(&m)
                        && state.trades.get(&sym).is_none_or(|old| old.time <= t.time)
                    {
                        state.trades.insert(sym, t);
                        changed(&mut result);
                    }
                }
                (Some("q"), Some(sym)) => {
                    if let Some(q) = parse::quote(&m)
                        && state.quotes.get(&sym).is_none_or(|old| old.time <= q.time)
                    {
                        state.quotes.insert(sym, q);
                        changed(&mut result);
                    }
                }
                (Some("b" | "u"), Some(sym)) => {
                    if let Some(b) = parse::bar(&m)
                        && state.bars.get(&sym).is_none_or(|old| old.time <= b.time)
                    {
                        state.bars.insert(sym, b);
                        changed(&mut result);
                    }
                }
                (Some("subscription"), _) => {
                    state.ticking = m
                        .get("trades")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                    changed(&mut result);
                }
                (Some("error"), _)
                    if m.get("code").and_then(Value::as_u64) == Some(SYMBOL_LIMIT) =>
                {
                    // The plan allows fewer than we thought: halve the
                    // limit and reconnect, which subscribes again.
                    let mut wire = self.wire.lock();
                    wire.cap = (wire.sent.len().min(wire.cap) / 2).max(1);
                    let what = format!(
                        "Alpaca {} stream: subscription limit exceeded; now streaming {} trades and quotes",
                        self.feed.label(),
                        wire.cap
                    );
                    state.notice = Some(what.clone());
                    return Err(FetchError::Network(what));
                }
                _ => match control(&m, self.feed.label())? {
                    Some(Control::Ready) => result = Applied::Ready,
                    Some(Control::Notice(n)) => {
                        state.notice = Some(n);
                        changed(&mut result);
                    }
                    None => {}
                },
            }
        }
        Ok(result)
    }

    fn on_connect(&self, state: &mut LiveMarket) {
        let mut wire = self.wire.lock();
        wire.sent.clear();
        wire.held.clear();
        state.ticking.clear();
    }
}

// --------------------------------------------------------------------- news

/// Stories published since the stream connected, newest first.
#[derive(Clone, Debug, Default)]
pub struct LiveNews {
    pub items: Vec<Headline>,
    pub notice: Option<String>,
}

/// `wss://stream.data.alpaca.markets/v1beta1/news`.
#[derive(Clone, Debug)]
pub struct NewsStream {
    url: String,
}

impl NewsStream {
    pub(crate) fn new(url: String) -> Self {
        Self { url }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Stream for NewsStream {
    type State = LiveNews;

    fn key(&self) -> String {
        "alpaca/stream/news".into()
    }

    fn label(&self) -> String {
        "Alpaca news (live)".into()
    }

    fn request(&self, _ctx: &FetchCtx) -> Result<Request, FetchError> {
        Ok(Request::get(&self.url))
    }

    fn hello(&self, ctx: &FetchCtx) -> Result<Vec<String>, FetchError> {
        login(ctx)
    }

    fn waits_for_ready(&self) -> bool {
        true
    }

    fn subscribe(&self, topics: &[String]) -> Vec<String> {
        let syms = news_symbols(topics);
        action("subscribe", &BTreeMap::from([("news", syms)]))
            .into_iter()
            .collect()
    }

    fn unsubscribe(&self, topics: &[String]) -> Vec<String> {
        let syms = news_symbols(topics);
        action("unsubscribe", &BTreeMap::from([("news", syms)]))
            .into_iter()
            .collect()
    }

    fn apply(&self, state: &mut LiveNews, frame: &Frame) -> Result<Applied, FetchError> {
        let Some(text) = frame.as_text() else {
            return Ok(Applied::Ignored);
        };
        let mut result = Applied::Ignored;
        let now = mt_core::time::now_utc();
        for m in parse::stream_messages(text)? {
            if m.get("T").and_then(Value::as_str) == Some("n") {
                if let Some(h) = parse::news_headline(&m, now)
                    && !state.items.iter().any(|o| o.id == h.id)
                {
                    state.items.insert(0, h);
                    state.items.truncate(LIVE_NEWS_KEEP);
                    if result == Applied::Ignored {
                        result = Applied::Changed;
                    }
                }
                continue;
            }
            match control(&m, "news")? {
                Some(Control::Ready) => result = Applied::Ready,
                Some(Control::Notice(n)) => state.notice = Some(n),
                None => {}
            }
        }
        Ok(result)
    }
}

fn news_symbols(topics: &[String]) -> Vec<String> {
    topics
        .iter()
        .filter_map(|t| split_topic(t))
        .filter(|(ch, _)| *ch == "news")
        .map(|(_, s)| s.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(s: &str) -> Frame {
        Frame::Text(s.to_owned())
    }

    fn topics(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn logins_subscriptions_and_prices() {
        let s = MarketStream::new("wss://x/v2/iex".into(), Feed::Iex, 30);
        let mut st = LiveMarket::default();
        assert_eq!(
            s.apply(&mut st, &frame(r#"[{"T":"success","msg":"connected"}]"#)),
            Ok(Applied::Ignored)
        );
        assert_eq!(
            s.apply(
                &mut st,
                &frame(r#"[{"T":"success","msg":"authenticated"}]"#)
            ),
            Ok(Applied::Ready)
        );
        let applied = s.apply(
            &mut st,
            &frame(
                r#"[{"T":"subscription","trades":["XLU"],"quotes":["XLU","XEL"],"bars":["XLU","XEL","AEE"]}]"#,
            ),
        );
        assert_eq!(applied, Ok(Applied::Changed));
        assert_eq!(
            st.ticking.iter().collect::<Vec<_>>(),
            ["XLU"],
            "every trade streams for the symbols confirmed for trades"
        );
        let data = r#"[{"T":"t","S":"XLU","i":1,"x":"V","p":82.51,"s":100,"t":"2026-10-02T19:59:58.1Z","c":["@"],"z":"B"},
            {"T":"q","S":"XLU","bx":"V","bp":82.5,"bs":2,"ax":"V","ap":82.52,"as":3,"t":"2026-10-02T19:59:59Z","c":["R"],"z":"B"},
            {"T":"b","S":"AEE","o":90,"h":90.2,"l":89.9,"c":90.1,"v":1200,"t":"2026-10-02T19:59:00Z","n":12,"vw":90.05},
            {"T":"t","S":"XLU","p":82.4,"s":1,"t":"2026-10-02T19:59:57Z"},
            {"T":"s","S":"XLU","sc":"T"}]"#;
        assert_eq!(s.apply(&mut st, &frame(data)), Ok(Applied::Changed));
        assert_eq!(
            st.trades["XLU"].price, 82.51,
            "an older trade does not replace a newer one"
        );
        assert_eq!(st.quotes["XLU"].ask, 82.52);
        assert_eq!(st.bars["AEE"].close, 90.1);
        // Over the limit after all: halve it and reconnect.
        s.subscribe(&market_topics(["XLU", "XEL", "AEE", "CMS"]));
        let limit = r#"[{"T":"error","code":405,"msg":"symbol limit exceeded"}]"#;
        assert!(matches!(
            s.apply(&mut st, &frame(limit)),
            Err(FetchError::Network(_))
        ));
        assert!(
            st.notice
                .as_deref()
                .is_some_and(|n| n.contains("now streaming 4"))
        );
        s.on_connect(&mut st);
        let msgs = s.subscribe(&market_topics(["XLU", "XEL", "AEE", "CMS", "WEC"]));
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["trades"].as_array().map(Vec::len), Some(4));
        assert!(v.get("quotes").is_none(), "{v}");
        // Other complaints are kept; refused logins stop the stream.
        let other = r#"[{"T":"error","code":407,"msg":"slow client"}]"#;
        assert_eq!(s.apply(&mut st, &frame(other)), Ok(Applied::Changed));
        assert!(
            st.notice
                .as_deref()
                .is_some_and(|n| n.contains("slow client"))
        );
        let refused = r#"[{"T":"error","code":402,"msg":"auth failed"}]"#;
        assert!(matches!(
            s.apply(&mut st, &frame(refused)),
            Err(FetchError::Auth(_))
        ));
        let busy = r#"[{"T":"error","code":406,"msg":"connection limit exceeded"}]"#;
        assert!(matches!(
            s.apply(&mut st, &frame(busy)),
            Err(FetchError::Network(_))
        ));
        assert!(matches!(
            s.apply(&mut st, &frame("not json")),
            Err(FetchError::Parse { .. })
        ));
        s.on_connect(&mut st);
        assert!(st.ticking.is_empty());
        assert!(st.trades.contains_key("XLU"), "prices survive a reconnect");
    }

    #[test]
    fn trades_first_then_quotes_within_the_limit() {
        // The free plan's 30 counts trade and quote subscriptions together.
        let s = MarketStream::new("wss://x/v2/iex".into(), Feed::Iex, 4);
        let msgs = s.subscribe(&market_topics(["XLU", "XEL", "AEE"]));
        assert_eq!(msgs.len(), 1);
        let v: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(v["action"], "subscribe");
        assert_eq!(v["trades"], serde_json::json!(["AEE", "XEL", "XLU"]));
        assert_eq!(
            v["quotes"],
            serde_json::json!(["AEE"]),
            "quotes with the room left"
        );
        assert_eq!(
            v["bars"],
            serde_json::json!(["XLU", "XEL", "AEE"]),
            "bars for every symbol"
        );
        // Dropping XEL frees a slot for a quote held back.
        let msgs = s.unsubscribe(&topics(&["trades:XEL", "quotes:XEL", "bars:XEL"]));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        let un: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(un["action"], "unsubscribe");
        assert_eq!(un["trades"], serde_json::json!(["XEL"]));
        assert_eq!(un["bars"], serde_json::json!(["XEL"]));
        assert!(un.get("quotes").is_none(), "quotes for XEL were never sent");
        let sub: Value = serde_json::from_str(&msgs[1]).unwrap();
        assert_eq!(sub["action"], "subscribe");
        assert_eq!(sub["quotes"], serde_json::json!(["XLU"]));
        // A new symbol's trades displace a quote.
        let msgs = s.subscribe(&topics(&["trades:CMS"]));
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        let un: Value = serde_json::from_str(&msgs[0]).unwrap();
        assert_eq!(un["action"], "unsubscribe");
        assert_eq!(un["quotes"].as_array().map(Vec::len), Some(1));
        let sub: Value = serde_json::from_str(&msgs[1]).unwrap();
        assert_eq!(sub["trades"], serde_json::json!(["CMS"]));
        // Dropping something held back sends nothing for it.
        let s = MarketStream::new("wss://x/v2/iex".into(), Feed::Iex, 1);
        s.subscribe(&topics(&["trades:XLU", "trades:XEL"]));
        // (symbols in order: XEL is sent, XLU held back)
        assert!(s.unsubscribe(&topics(&["trades:XLU"])).is_empty());
        assert!(s.subscribe(&[]).is_empty());
    }

    #[test]
    fn news_arrives_newest_first() {
        let s = NewsStream::new("wss://x/v1beta1/news".into());
        let sub: Value =
            serde_json::from_str(&s.subscribe(&topics(&["news:XLU", "news:XEL"]))[0]).unwrap();
        assert_eq!(sub["news"], serde_json::json!(["XLU", "XEL"]));
        let mut st = LiveNews::default();
        assert_eq!(
            s.apply(
                &mut st,
                &frame(r#"[{"T":"success","msg":"authenticated"}]"#)
            ),
            Ok(Applied::Ready)
        );
        let story = |id: u32| {
            frame(&format!(
                r#"[{{"T":"n","id":{id},"headline":"Story {id}","summary":"","author":"A","created_at":"2026-10-02T21:00:00Z",
                "updated_at":"2026-10-02T21:00:00Z","url":"https://www.benzinga.com/{id}","content":"<p>body</p>","symbols":["XLU"],"source":"benzinga"}}]"#
            ))
        };
        assert_eq!(s.apply(&mut st, &story(1)), Ok(Applied::Changed));
        assert_eq!(s.apply(&mut st, &story(2)), Ok(Applied::Changed));
        assert_eq!(
            s.apply(&mut st, &story(2)),
            Ok(Applied::Ignored),
            "once per story"
        );
        assert_eq!(st.items[0].title, "Story 2");
    }
}
