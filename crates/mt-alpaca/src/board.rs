//! One row per security for quote monitors: the minute-old snapshot brought
//! up to date with whatever the stream has delivered since.

use chrono::{DateTime, Utc};
use mt_core::equity::{Bar, Quote, Snapshot, Trade};
use mt_core::exchange::to_exchange;

use crate::LiveMarket;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub symbol: String,
    pub last: Option<f64>,
    pub last_time: Option<DateTime<Utc>>,
    /// The close changes are measured from.
    pub prev_close: Option<f64>,
    pub change: Option<f64>,
    pub change_pct: Option<f64>,
    pub quote: Option<Quote>,
    /// The last trade's session: open, high, low, volume and VWAP.
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub volume: Option<f64>,
    pub vwap: Option<f64>,
    /// Every trade and quote streams (rather than minute bars only).
    pub ticking: bool,
}

impl Row {
    /// When the newest of the last trade and the quote is from: how fresh
    /// the row's prices are, for the guardrails.
    pub fn priced_at(&self) -> Option<DateTime<Utc>> {
        self.last_time.max(self.quote.as_ref().map(|q| q.time))
    }
}

fn newer<'a, T>(
    a: Option<&'a T>,
    b: Option<&'a T>,
    time: impl Fn(&T) -> DateTime<Utc>,
) -> Option<&'a T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if time(y) > time(x) { y } else { x }),
        (x, y) => x.or(y),
    }
}

/// `symbol`'s figures from its snapshot and the live stream.
pub fn row(symbol: &str, snap: Option<&Snapshot>, live: Option<&LiveMarket>) -> Row {
    let snap_trade = snap.and_then(|s| s.latest_trade.as_ref());
    let trade: Option<&Trade> = newer(snap_trade, live.and_then(|l| l.trades.get(symbol)), |t| {
        t.time
    });
    let quote = newer(
        snap.and_then(|s| s.latest_quote.as_ref()),
        live.and_then(|l| l.quotes.get(symbol)),
        |q| q.time,
    );
    let snap_bar = snap.and_then(|s| s.minute_bar.as_ref());
    let bar: Option<&Bar> = newer(snap_bar, live.and_then(|l| l.bars.get(symbol)), |b| b.time);
    let daily = snap.and_then(|s| s.daily_bar.as_ref());

    // The last price: a trade, unless a newer minute bar has come since.
    let (last, last_time) = match (trade, bar) {
        (Some(t), Some(b)) if b.time > t.time => (Some(b.close), Some(b.time)),
        (Some(t), _) => (Some(t.price), Some(t.time)),
        (None, Some(b)) => (Some(b.close), Some(b.time)),
        (None, None) => (daily.map(|d| d.close), None),
    };
    let last = last.filter(|p| p.is_finite() && *p > 0.0);
    let prev_close = snap.and_then(|s| s.reference_close(last_time.or_else(|| s.last_time())));
    let change = last.zip(prev_close).map(|(l, p)| l - p);

    // The daily bar describes the last trade's session only if it is from it.
    let session = |t: DateTime<Utc>| to_exchange(t).date_naive();
    let daily = daily.filter(|d| last_time.is_none_or(|t| session(t) == session(d.time)));
    let mut high = daily.map(|d| d.high);
    let mut low = daily.map(|d| d.low);
    // Anything the stream brought after the snapshot widens the range.
    let since = snap_trade.map(|t| t.time).max(snap_bar.map(|b| b.time));
    let mut widen = |hi: f64, lo: f64| {
        high = Some(high.map_or(hi, |h| h.max(hi)));
        low = Some(low.map_or(lo, |l| l.min(lo)));
    };
    if let Some(t) = trade.filter(|t| since.is_none_or(|s| t.time > s)) {
        widen(t.price, t.price);
    }
    if let Some(b) = bar.filter(|b| since.is_none_or(|s| b.time > s)) {
        widen(b.high, b.low);
    }
    Row {
        symbol: symbol.to_owned(),
        last,
        last_time,
        prev_close,
        change,
        change_pct: change.zip(prev_close).map(|(c, p)| c / p * 100.0),
        quote: quote.cloned(),
        open: daily.map(|d| d.open),
        high,
        low,
        volume: daily.map(|d| d.volume),
        vwap: daily.and_then(|d| d.vwap),
        ticking: live.is_some_and(|l| l.ticking.contains(symbol)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn bar(at: &str, o: f64, h: f64, l: f64, c: f64) -> Bar {
        Bar {
            time: t(at),
            open: o,
            high: h,
            low: l,
            close: c,
            volume: 1000.0,
            trades: None,
            vwap: Some(c),
        }
    }

    fn trade(at: &str, p: f64) -> Trade {
        Trade {
            time: t(at),
            price: p,
            size: 10.0,
            exchange: Some("V".into()),
        }
    }

    #[test]
    fn the_stream_brings_the_snapshot_up_to_date() {
        let snap = Snapshot {
            latest_trade: Some(trade("2026-10-02T19:00:00Z", 82.0)),
            minute_bar: Some(bar("2026-10-02T18:59:00Z", 82.0, 82.0, 82.0, 82.0)),
            daily_bar: Some(bar("2026-10-02T04:00:00Z", 81.0, 82.5, 80.5, 82.0)),
            prev_daily_bar: Some(bar("2026-10-01T04:00:00Z", 79.0, 80.5, 79.0, 80.0)),
            ..Snapshot::default()
        };
        let r = row("XLU", Some(&snap), None);
        assert_eq!(
            (r.last, r.change, r.high),
            (Some(82.0), Some(2.0), Some(82.5))
        );
        assert_eq!(r.change_pct, Some(2.5));

        let mut live = LiveMarket::default();
        live.trades
            .insert("XLU".into(), trade("2026-10-02T19:30:00Z", 83.0));
        live.bars.insert(
            "XLU".into(),
            bar("2026-10-02T19:29:00Z", 82.0, 83.1, 79.9, 82.9),
        );
        live.ticking.insert("XLU".into());
        let r = row("XLU", Some(&snap), Some(&live));
        assert_eq!(r.last, Some(83.0));
        assert_eq!(r.change, Some(3.0));
        assert_eq!(
            (r.high, r.low),
            (Some(83.1), Some(79.9)),
            "widened by what came since"
        );
        assert!(r.ticking);
        assert_eq!(r.volume, Some(1000.0));

        // Monday's first trade: measured from Friday's close, and Friday's
        // range and volume are not Monday's.
        live.trades
            .insert("XLU".into(), trade("2026-10-05T12:00:00Z", 84.0));
        live.bars.clear();
        let r = row("XLU", Some(&snap), Some(&live));
        assert_eq!(r.prev_close, Some(82.0));
        assert_eq!(r.change, Some(2.0));
        assert_eq!((r.volume, r.high, r.low), (None, Some(84.0), Some(84.0)));
        assert_eq!(
            row("NONE", None, None),
            Row {
                symbol: "NONE".into(),
                ..Row::default()
            }
        );
    }
}
