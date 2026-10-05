//! Every Alpaca dataset against its recording (`MT_FIXTURES` to use another
//! set, as the weekly drift job does with live ones). The checks are about
//! structure and sanity, not values, so a fresh recording keeps them green.

use std::path::{Path, PathBuf};

use chrono::{Timelike, Utc};
use mt_alpaca::account::{
    parse_account, parse_activities, parse_option_snapshots, parse_portfolio_history,
    parse_positions,
};
use mt_alpaca::parse::{
    parse_assets, parse_bars, parse_calendar, parse_clock, parse_news, parse_snapshots,
};
use mt_alpaca::{LiveMarket, LiveNews, LiveTrades, MarketStream, NewsStream};
use mt_core::account::{ActivityCategory, to_f64};
use mt_core::exchange;
use mt_data::{Applied, Frame, Stream};

fn root() -> PathBuf {
    std::env::var_os("MT_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

fn read(rel: &str) -> Vec<u8> {
    let path = root().join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn prices_ok(what: &str, values: &[f64]) {
    for v in values {
        assert!(v.is_finite() && *v > 0.0, "{what}: price {v}");
    }
}

#[test]
fn clock_and_calendar() {
    let clock = parse_clock(&read("paper-api.alpaca.markets/v2/clock.json")).unwrap();
    assert!(clock.next_open > clock.at - chrono::Duration::days(1));
    assert!(clock.next_close > clock.next_open || clock.is_open);
    let days = parse_calendar(&read("paper-api.alpaca.markets/v2/calendar.json")).unwrap();
    assert!(days.len() >= 20, "{} trading days", days.len());
    for d in &days {
        assert!(
            d.session_open <= d.open && d.open < d.close && d.close <= d.session_close,
            "{d:?}"
        );
        assert!(
            exchange::TradingDay::standard(d.date).is_some(),
            "{} is a weekend",
            d.date
        );
    }
}

#[test]
fn asset_list() {
    let assets = parse_assets(&read("paper-api.alpaca.markets/v2/assets.json")).unwrap();
    assert!(assets.len() >= 20, "{} assets", assets.len());
    for sym in ["XLU", "XEL", "SPY"] {
        let a = assets
            .iter()
            .find(|a| a.symbol == sym)
            .unwrap_or_else(|| panic!("{sym} is missing"));
        assert!(a.active && a.tradable && !a.name.is_empty(), "{a:?}");
        assert!(!a.exchange.is_empty());
    }
    assert!(
        assets
            .iter()
            .all(|a| mt_core::instrument::is_ticker(&a.symbol) || a.symbol.contains(' ')),
        "symbols are tickers"
    );
}

#[test]
fn snapshots() {
    for file in ["snapshots.json", "snapshots@delayed_sip.json"] {
        let snaps =
            parse_snapshots(&read(&format!("data.alpaca.markets/v2/stocks/{file}"))).unwrap();
        for sym in ["XLU", "XEL", "SPY"] {
            let s = snaps.get(sym).unwrap_or_else(|| panic!("{file}: no {sym}"));
            assert!(s.last().is_some(), "{file}: {sym} has no last price");
            assert!(
                s.prev_close().is_some(),
                "{file}: {sym} has no previous close"
            );
            let d = s.daily_bar.as_ref().expect("a daily bar");
            prices_ok(sym, &[d.open, d.high, d.low, d.close]);
            assert!(d.low <= d.high);
        }
        let quoted = snaps.values().filter(|s| s.latest_quote.is_some()).count();
        assert!(
            quoted * 2 >= snaps.len(),
            "{file}: most symbols have quotes"
        );
    }
}

#[test]
fn bars() {
    for (tf, secs, min_bars) in [("1Min", 60, 10), ("15Min", 900, 10), ("1Day", 86_400, 150)] {
        let (bars, token) = parse_bars(&read(&format!(
            "data.alpaca.markets/v2/stocks/bars@{tf}.json"
        )))
        .unwrap();
        assert!(token.is_none(), "{tf}: recordings are a single page");
        let xlu = bars.get("XLU").unwrap_or_else(|| panic!("{tf}: no XLU"));
        assert!(xlu.len() >= min_bars, "{tf}: {} XLU bars", xlu.len());
        for list in bars.values() {
            for w in list.windows(2) {
                assert!(w[0].time < w[1].time, "{tf}: out of order");
            }
            for b in list {
                prices_ok(tf, &[b.open, b.high, b.low, b.close]);
                assert!(
                    b.low <= b.open.min(b.close) + 1e-9 && b.high + 1e-9 >= b.open.max(b.close),
                    "{tf}: {b:?}"
                );
                assert!(
                    b.time.timestamp() % secs.min(3600) == 0 || secs == 86_400,
                    "{tf}: {b:?} is off the grid"
                );
            }
        }
        if tf == "1Day" {
            // Daily bars start at midnight in New York.
            for b in xlu {
                let local = exchange::to_exchange(b.time);
                assert_eq!((local.hour(), local.minute()), (0, 0), "{b:?}");
            }
        }
    }
}

#[test]
fn company_news() {
    let (news, _) = parse_news(&read("data.alpaca.markets/v1beta1/news.json"), Utc::now()).unwrap();
    assert!(!news.is_empty(), "no stories");
    for h in &news {
        assert!(!h.title.is_empty(), "{h:?}");
        assert!(
            h.link.starts_with("https://") || h.link.starts_with("http://"),
            "{h:?}"
        );
        assert!(h.published.is_some(), "{h:?}");
        assert!(
            !h.title.contains('<') || !h.title.contains('>'),
            "markup in {:?}",
            h.title
        );
    }
    assert!(
        news.windows(2).all(|w| w[0].time() >= w[1].time()),
        "newest first"
    );
}

/// Apply a recorded session's frames as the hub would.
fn replay<S: Stream>(s: &S, path: &Path) -> (S::State, bool) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut state = S::State::default();
    let mut ready = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match s.apply(&mut state, &Frame::Text(line.to_owned())) {
            Ok(Applied::Ready) => ready = true,
            Ok(_) => {}
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }
    (state, ready)
}

#[test]
fn streams() {
    let alpaca = mt_alpaca::Alpaca::default();
    let dir = root().join("stream.data.alpaca.markets");
    // The live feed: logged in, and every trade of the default list's symbols
    // accepted (the free plan allows 30 trade and quote subscriptions).
    let (live, ready): (LiveMarket, bool) =
        replay(&alpaca.market_stream(), &dir.join("v2/iex.jsonl"));
    assert!(ready, "never logged in");
    assert!(live.notice.is_none(), "{:?}", live.notice);
    let list = mt_alpaca::MarketsConfig::default()
        .list(mt_alpaca::config::DEFAULT_LIST)
        .unwrap()
        .securities();
    for sec in &list {
        assert!(
            live.ticking.contains(&sec.ticker),
            "{} does not stream its trades",
            sec.ticker
        );
    }
    // The test feed trades around the clock: the message formats.
    let test = MarketStream::test_feed(alpaca.endpoints());
    let (fake, ready): (LiveMarket, bool) = replay(&test, &dir.join("v2/test.jsonl"));
    assert!(ready);
    assert!(
        !fake.trades.is_empty() || !fake.quotes.is_empty(),
        "no trades or quotes from the test feed"
    );
    for t in fake.trades.values() {
        prices_ok("test trade", &[t.price]);
    }
    let news: NewsStream = alpaca.news_stream();
    let (_, ready): (LiveNews, bool) = replay(&news, &dir.join("v1beta1/news.jsonl"));
    assert!(ready);
}

/// Whether these are the repository's own fixtures (the sample account),
/// not a live recording.
fn sample() -> bool {
    std::env::var_os("MT_FIXTURES").is_none()
}

/// The account is only in live recordings made with `--verbatim` (and in
/// the repository's sample): skip it in a sample-copy recording.
fn has_account() -> bool {
    let there = root()
        .join("paper-api.alpaca.markets/v2/account.json")
        .exists();
    if !there {
        assert!(!sample(), "the repository's sample account is missing");
        eprintln!("no paper account in this recording; skipped");
    }
    there
}

#[test]
fn paper_account() {
    if !has_account() {
        return;
    }
    let a = parse_account(&read("paper-api.alpaca.markets/v2/account.json")).unwrap();
    assert!(!a.status.is_empty() && !a.number.is_empty(), "{a:?}");
    assert!(a.equity >= mt_core::money::Decimal::ZERO);
    // Equity is cash plus what the positions are worth.
    let sum = a.cash + a.long_market_value + a.short_market_value;
    assert!(
        (to_f64(a.equity) - to_f64(sum)).abs() < 1.0,
        "equity {} against cash and positions {sum}",
        a.equity
    );
    let positions = parse_positions(&read("paper-api.alpaca.markets/v2/positions.json")).unwrap();
    for p in &positions {
        assert!(!p.qty.is_zero(), "{p:?}");
        let (Some(mv), Some(price)) = (p.market_value, p.current_price) else {
            continue;
        };
        let expect = to_f64(p.qty * price * p.multiplier());
        assert!(
            (to_f64(mv) - expect).abs() <= expect.abs() * 0.005 + 0.05,
            "{}: market value {mv} against {expect}",
            p.symbol
        );
    }
    if sample() {
        // The sample reconciles exactly: the day's P&L is the positions'.
        let day: mt_core::money::Decimal = positions
            .iter()
            .filter_map(|p| p.unrealized_intraday_pl)
            .sum();
        assert_eq!(day, a.day_pl());
        assert!(positions.iter().any(|p| p.is_short()));
        assert!(positions.iter().any(|p| p.is_option()));
        assert!(
            a.number.contains("SAMPLE"),
            "never a real account number in the repository"
        );
    }
}

#[test]
fn equity_history() {
    if !has_account() {
        return;
    }
    for period in ["1D", "1W", "1M", "3M", "1A"] {
        let rel = format!("paper-api.alpaca.markets/v2/account/portfolio/history@{period}.json");
        let path = root().join(&rel);
        if !path.exists() && !sample() {
            continue;
        }
        let h = parse_portfolio_history(&read(&rel)).unwrap();
        assert!(!h.points.is_empty() || !sample(), "{period}: no points");
        assert!(
            h.points.windows(2).all(|w| w[0].time < w[1].time),
            "{period}: out of order"
        );
        for p in &h.points {
            assert!(p.equity.is_finite() && p.equity >= 0.0, "{period}: {p:?}");
        }
        assert!(!h.timeframe.is_empty(), "{period}: no timeframe");
        if sample() {
            let a = parse_account(&read("paper-api.alpaca.markets/v2/account.json")).unwrap();
            let last = h.last().unwrap().equity;
            assert!(
                (last - to_f64(a.equity)).abs() < 0.01,
                "{period}: ends at {last}, the account's equity is {}",
                a.equity
            );
        }
    }
}

#[test]
fn activities() {
    if !has_account() {
        return;
    }
    let list =
        parse_activities(&read("paper-api.alpaca.markets/v2/account/activities.json")).unwrap();
    assert!(
        list.windows(2).all(|w| w[0].when() >= w[1].when()),
        "newest first"
    );
    let mut ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), list.len(), "ids are unique");
    for a in &list {
        assert!(a.when().is_some(), "{a:?} has no time or date");
        if a.category() == ActivityCategory::Fill {
            assert!(
                a.symbol.is_some() && a.side.is_some() && a.qty.is_some() && a.price.is_some(),
                "{a:?}"
            );
        }
    }
    if sample() {
        for cat in [
            ActivityCategory::Fill,
            ActivityCategory::Dividend,
            ActivityCategory::Option,
        ] {
            assert!(
                list.iter().any(|a| a.category() == cat),
                "no {}",
                cat.label()
            );
        }
    }
    let options = root().join("data.alpaca.markets/v1beta1/options/snapshots.json");
    if options.exists() {
        let (snaps, _) = parse_option_snapshots(&std::fs::read(&options).unwrap()).unwrap();
        for (sym, s) in &snaps {
            assert!(
                mt_core::instrument::OptionContract::parse_occ(sym).is_some(),
                "{sym} is not an OCC symbol"
            );
            if let Some(d) = s.greeks.delta {
                assert!((-1.0..=1.0).contains(&d), "{sym}: delta {d}");
            }
        }
    }
}

#[test]
fn order_events() {
    if !has_account() {
        return;
    }
    let s = mt_alpaca::Alpaca::default().trade_stream();
    let (state, ready): (LiveTrades, bool) =
        replay(&s, &root().join("paper-api.alpaca.markets/stream.jsonl"));
    assert!(ready, "never logged in");
    assert!(state.listening, "not listening for order events");
    for e in &state.events {
        assert!(!e.event.is_empty() && !e.order_id.is_empty(), "{e:?}");
    }
    if sample() {
        assert!(state.events.iter().any(|e| e.is_fill()));
    }
}
