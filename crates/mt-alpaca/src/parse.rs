//! Alpaca's JSON into `mt-core` types. Pure functions, tested against the
//! recordings in `fixtures/`. Fields are read loosely (missing ones are
//! `None`), so a renamed field degrades one column rather than the dataset.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use mt_core::equity::{Asset, Bar, MarketClock, Quote, Snapshot, Trade};
use mt_core::exchange::TradingDay;
use mt_core::news::Headline;
use mt_data::FetchError;
use serde::Deserialize;
use serde_json::Value;

/// An RFC 3339 time with any offset and any precision.
pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn time_of(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    v.get(key).and_then(Value::as_str).and_then(parse_time)
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn json(what: &str, body: &[u8]) -> Result<Value, FetchError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| FetchError::parse(what, e))?;
    // An error answer where data was expected: say what Alpaca said.
    if let (Some(msg), true) = (
        v.get("message").and_then(Value::as_str),
        v.get("code").is_some(),
    ) {
        return Err(FetchError::parse(what, format!("Alpaca said: {msg}")));
    }
    Ok(v)
}

pub fn trade(v: &Value) -> Option<Trade> {
    Some(Trade {
        time: time_of(v, "t")?,
        price: num(v, "p")?,
        size: num(v, "s").unwrap_or(0.0),
        exchange: text(v, "x"),
    })
}

pub fn quote(v: &Value) -> Option<Quote> {
    Some(Quote {
        time: time_of(v, "t")?,
        bid: num(v, "bp").unwrap_or(0.0),
        bid_size: num(v, "bs").unwrap_or(0.0),
        ask: num(v, "ap").unwrap_or(0.0),
        ask_size: num(v, "as").unwrap_or(0.0),
        bid_exchange: text(v, "bx"),
        ask_exchange: text(v, "ax"),
    })
}

pub fn bar(v: &Value) -> Option<Bar> {
    let close = num(v, "c")?;
    Some(Bar {
        time: time_of(v, "t")?,
        open: num(v, "o").unwrap_or(close),
        high: num(v, "h").unwrap_or(close),
        low: num(v, "l").unwrap_or(close),
        close,
        volume: num(v, "v").unwrap_or(0.0),
        trades: v.get("n").and_then(Value::as_u64),
        vwap: num(v, "vw"),
    })
}

fn snapshot(v: &Value) -> Snapshot {
    let part = |k: &str| v.get(k).filter(|p| p.is_object());
    Snapshot {
        latest_trade: part("latestTrade").and_then(trade),
        latest_quote: part("latestQuote").and_then(quote),
        minute_bar: part("minuteBar").and_then(bar),
        daily_bar: part("dailyBar").and_then(bar),
        prev_daily_bar: part("prevDailyBar").and_then(bar),
    }
}

/// `GET /v2/stocks/snapshots`: one snapshot per symbol, keyed by symbol at the
/// top level. Symbols with no data are left out.
pub fn parse_snapshots(body: &[u8]) -> Result<BTreeMap<String, Snapshot>, FetchError> {
    let v = json("Alpaca snapshots", body)?;
    // Older answers nested them under "snapshots".
    let map = v.get("snapshots").unwrap_or(&v);
    let obj = map
        .as_object()
        .ok_or_else(|| FetchError::parse("Alpaca snapshots", "not an object"))?;
    Ok(obj
        .iter()
        .filter(|(_, s)| s.is_object())
        .map(|(sym, s)| (sym.to_ascii_uppercase(), snapshot(s)))
        .filter(|(_, s)| s != &Snapshot::default())
        .collect())
}

/// Bars per symbol, oldest first.
pub type BarsBySymbol = BTreeMap<String, Vec<Bar>>;

/// `GET /v2/stocks/bars`: bars per symbol, oldest first, and the token for
/// the next page. Also reads the single-symbol form (`/v2/stocks/{s}/bars`).
pub fn parse_bars(body: &[u8]) -> Result<(BarsBySymbol, Option<String>), FetchError> {
    let v = json("Alpaca bars", body)?;
    let token = text(&v, "next_page_token");
    let mut out = BarsBySymbol::new();
    match v.get("bars") {
        Some(Value::Object(per_symbol)) => {
            for (sym, list) in per_symbol {
                let bars: Vec<Bar> = list
                    .as_array()
                    .map(|a| a.iter().filter_map(bar).collect())
                    .unwrap_or_default();
                out.insert(sym.to_ascii_uppercase(), bars);
            }
        }
        Some(Value::Array(list)) => {
            let sym = text(&v, "symbol").unwrap_or_default().to_ascii_uppercase();
            out.insert(sym, list.iter().filter_map(bar).collect());
        }
        Some(Value::Null) | None => {}
        Some(_) => return Err(FetchError::parse("Alpaca bars", "bars is not a list")),
    }
    for bars in out.values_mut() {
        bars.sort_by_key(|b| b.time);
        bars.dedup_by_key(|b| b.time);
    }
    Ok((out, token))
}

#[derive(Deserialize)]
struct WireAsset {
    symbol: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    exchange: Option<String>,
    #[serde(default, rename = "class")]
    class: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tradable: bool,
    #[serde(default)]
    marginable: bool,
    #[serde(default)]
    shortable: bool,
    #[serde(default)]
    easy_to_borrow: bool,
    #[serde(default)]
    fractionable: bool,
    #[serde(default)]
    maintenance_margin_requirement: Option<f64>,
    #[serde(default)]
    attributes: Option<Vec<String>>,
}

/// `GET /v2/assets`: the asset list.
pub fn parse_assets(body: &[u8]) -> Result<Vec<Asset>, FetchError> {
    let v = json("Alpaca assets", body)?;
    let list = v
        .as_array()
        .ok_or_else(|| FetchError::parse("Alpaca assets", "not a list"))?;
    Ok(list
        .iter()
        .filter_map(|a| serde_json::from_value::<WireAsset>(a.clone()).ok())
        .filter(|a| !a.symbol.trim().is_empty())
        .map(|a| Asset {
            symbol: a.symbol.trim().to_ascii_uppercase(),
            name: a.name.unwrap_or_default().trim().to_owned(),
            exchange: a.exchange.unwrap_or_default(),
            class: a.class.unwrap_or_default(),
            active: a.status.as_deref() == Some("active"),
            tradable: a.tradable,
            marginable: a.marginable,
            shortable: a.shortable,
            easy_to_borrow: a.easy_to_borrow,
            fractionable: a.fractionable,
            maintenance_margin: a.maintenance_margin_requirement,
            attributes: a.attributes.unwrap_or_default(),
        })
        .collect())
}

/// `GET /v2/clock`.
pub fn parse_clock(body: &[u8]) -> Result<MarketClock, FetchError> {
    let v = json("Alpaca clock", body)?;
    let field = |k: &str| {
        time_of(&v, k).ok_or_else(|| FetchError::parse("Alpaca clock", format!("no {k}")))
    };
    Ok(MarketClock {
        at: field("timestamp")?,
        is_open: v.get("is_open").and_then(Value::as_bool).unwrap_or(false),
        next_open: field("next_open")?,
        next_close: field("next_close")?,
    })
}

/// `09:30` or `0930`.
fn clock_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    NaiveTime::parse_from_str(s, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H%M"))
        .ok()
}

/// `GET /v2/calendar`: the trading days in a range, with their hours (New York time).
pub fn parse_calendar(body: &[u8]) -> Result<Vec<TradingDay>, FetchError> {
    let v = json("Alpaca calendar", body)?;
    let list = v
        .as_array()
        .ok_or_else(|| FetchError::parse("Alpaca calendar", "not a list"))?;
    let mut days: Vec<TradingDay> = list
        .iter()
        .filter_map(|d| {
            let date = NaiveDate::parse_from_str(d.get("date")?.as_str()?, "%Y-%m-%d").ok()?;
            let std = TradingDay::standard(date)?;
            let t = |k: &str| d.get(k).and_then(Value::as_str).and_then(clock_time);
            Some(TradingDay {
                date,
                session_open: t("session_open").unwrap_or(std.session_open),
                open: t("open")?,
                close: t("close")?,
                session_close: t("session_close").unwrap_or(std.session_close),
            })
        })
        .collect();
    days.sort_by_key(|d| d.date);
    Ok(days)
}

/// News sources as Alpaca names them (`benzinga`) to how a headline credits them.
pub fn source_name(source: &str) -> String {
    match source.trim().to_ascii_lowercase().as_str() {
        "benzinga" | "" => "Benzinga".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

/// One Alpaca news article as a headline (summary only; never its content).
pub fn news_headline(v: &Value, now: DateTime<Utc>) -> Option<Headline> {
    let id = match v.get("id")? {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => return None,
    };
    let title = crate::text::plain(&text(v, "headline")?);
    let published = time_of(v, "created_at").or_else(|| time_of(v, "updated_at"));
    let symbols: Vec<String> = v
        .get("symbols")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_ascii_uppercase)
                .collect()
        })
        .unwrap_or_default();
    Some(Headline {
        id: format!("alpaca:{id}"),
        source: source_name(&text(v, "source").unwrap_or_default()),
        title,
        summary: text(v, "summary")
            .map(|s| crate::text::plain(&s))
            .unwrap_or_default(),
        link: text(v, "url").unwrap_or_default(),
        author: text(v, "author"),
        published,
        seen: now,
        sections: symbols,
    })
}

/// `GET /v1beta1/news`: articles newest first, and the next page's token.
pub fn parse_news(
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<(Vec<Headline>, Option<String>), FetchError> {
    let v = json("Alpaca news", body)?;
    let list = v
        .get("news")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::parse("Alpaca news", "no news list"))?;
    let mut items: Vec<Headline> = list.iter().filter_map(|n| news_headline(n, now)).collect();
    items.sort_by_key(|h| std::cmp::Reverse(h.time()));
    Ok((items, text(&v, "next_page_token")))
}

/// A stream frame: Alpaca sends arrays of messages, each tagged by `T`.
pub fn stream_messages(frame: &str) -> Result<Vec<Value>, FetchError> {
    match serde_json::from_str::<Value>(frame) {
        Ok(Value::Array(items)) => Ok(items),
        Ok(other @ Value::Object(_)) => Ok(vec![other]),
        Ok(_) => Err(FetchError::parse("Alpaca stream", "not a message list")),
        Err(e) => Err(FetchError::parse("Alpaca stream", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_bars_and_errors() {
        let body = br#"{"XLU":{"latestTrade":{"t":"2026-10-02T21:49:58.123456789Z","x":"V","p":82.5,"s":100,"c":["@"],"i":7,"z":"B"},
            "latestQuote":{"t":"2026-10-02T21:50:00Z","ax":"V","ap":82.6,"as":2,"bx":"V","bp":82.4,"bs":3,"c":["R"],"z":"B"},
            "minuteBar":null,"dailyBar":{"t":"2026-10-02T04:00:00Z","o":81,"h":83,"l":80.5,"c":82.4,"v":1000,"n":10,"vw":82},
            "prevDailyBar":{"t":"2026-10-01T04:00:00Z","o":80,"h":81,"l":79,"c":80,"v":900}},
            "NODATA":null,"EMPTY":{}}"#;
        let s = parse_snapshots(body).unwrap();
        assert_eq!(s.len(), 1);
        let xlu = &s["XLU"];
        assert_eq!(xlu.last(), Some(82.5));
        assert_eq!(xlu.prev_close(), Some(80.0));
        assert_eq!(xlu.latest_quote.as_ref().unwrap().ask, 82.6);
        assert_eq!(xlu.minute_bar, None);
        assert_eq!(xlu.daily_bar.as_ref().unwrap().trades, Some(10));

        let (bars, token) = parse_bars(
            br#"{"bars":{"xlu":[{"t":"2026-10-02T13:31:00Z","o":1,"h":2,"l":0.5,"c":1.5,"v":10},
                {"t":"2026-10-02T13:30:00Z","c":1}]},"next_page_token":"abc"}"#,
        )
        .unwrap();
        assert_eq!(token.as_deref(), Some("abc"));
        assert_eq!(bars["XLU"].len(), 2);
        assert!(bars["XLU"][0].time < bars["XLU"][1].time, "sorted");
        let (none, token) = parse_bars(br#"{"bars":null,"next_page_token":null}"#).unwrap();
        assert!(none.is_empty() && token.is_none());
        let (single, _) =
            parse_bars(br#"{"bars":[{"t":"2026-10-02T04:00:00Z","c":3}],"symbol":"XEL"}"#).unwrap();
        assert_eq!(single["XEL"].len(), 1);
        match parse_snapshots(br#"{"code":40010001,"message":"invalid symbol"}"#) {
            Err(FetchError::Parse { detail, .. }) => assert!(detail.contains("invalid symbol")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn calendar_clock_and_news() {
        let cal = parse_calendar(
            br#"[{"date":"2026-11-27","open":"09:30","close":"13:00","session_open":"0400","session_close":"1700","settlement_date":"2026-11-30"},
                 {"date":"2026-11-25","open":"09:30","close":"16:00"}]"#,
        )
        .unwrap();
        assert_eq!(cal.len(), 2);
        assert_eq!(cal[0].date.to_string(), "2026-11-25");
        assert_eq!(
            cal[0].session_close,
            NaiveTime::from_hms_opt(20, 0, 0).unwrap()
        );
        assert_eq!(cal[1].close, NaiveTime::from_hms_opt(13, 0, 0).unwrap());
        assert_eq!(
            cal[1].session_close,
            NaiveTime::from_hms_opt(17, 0, 0).unwrap()
        );

        let clock = parse_clock(
            br#"{"timestamp":"2026-10-02T17:55:01.123456-04:00","is_open":false,
                "next_open":"2026-10-05T09:30:00-04:00","next_close":"2026-10-05T16:00:00-04:00"}"#,
        )
        .unwrap();
        assert!(!clock.is_open);
        assert_eq!(clock.next_open.to_rfc3339(), "2026-10-05T13:30:00+00:00");

        let now = parse_time("2026-10-02T22:00:00Z").unwrap();
        let (news, token) = parse_news(
            br#"{"news":[{"id":1,"headline":"Older &amp; wiser","summary":"<p>Plain <b>text</b></p>","author":"A",
                    "created_at":"2026-10-01T12:00:00Z","url":"https://www.benzinga.com/x","symbols":["xlu"],"source":"benzinga","content":"never kept"},
                {"id":2,"headline":"Newer","summary":"","created_at":"2026-10-02T12:00:00Z","url":"https://www.benzinga.com/y","symbols":[],"source":"benzinga"}],
                "next_page_token":null}"#,
            now,
        )
        .unwrap();
        assert!(token.is_none());
        assert_eq!(news[0].title, "Newer", "newest first");
        let older = &news[1];
        assert_eq!(older.id, "alpaca:1");
        assert_eq!(older.title, "Older & wiser");
        assert_eq!(older.summary, "Plain text");
        assert_eq!(older.source, "Benzinga");
        assert_eq!(older.sections, ["XLU"]);
        assert!(!format!("{older:?}").contains("never kept"));
    }

    #[test]
    fn assets() {
        let list = parse_assets(
            br#"[{"id":"x","class":"us_equity","exchange":"ARCA","symbol":"XLU","name":"Utilities Select Sector SPDR Fund",
                  "status":"active","tradable":true,"marginable":true,"maintenance_margin_requirement":30,"shortable":true,
                  "easy_to_borrow":true,"fractionable":true,"attributes":["has_options"]},
                 {"symbol":"OLD","status":"inactive","attributes":null},
                 {"no_symbol":true}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].active && list[0].has_attribute("has_options"));
        assert_eq!(list[0].maintenance_margin, Some(30.0));
        assert_eq!(list[0].exchange_name(), "NYSE Arca");
        assert!(!list[1].active && list[1].name.is_empty());
    }
}
