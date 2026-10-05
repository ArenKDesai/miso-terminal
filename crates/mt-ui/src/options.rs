//! What the option functions share (OMON, and the tickets for contracts):
//! formats for premiums, implied volatility and greeks, expiry labels, the
//! expiry a chain opens on, and the note on where option prices come from.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use mt_core::exchange;
use mt_core::options::is_monthly;

use crate::market::fmt::DASH;

/// Where option prices come from on Alpaca's free plan, for footnotes.
pub const FEED_NOTE: &str = "Option prices are Alpaca's indicative feed: quotes derived from \
     OPRA's and sampled, trades 15 minutes late. Greeks and implied volatility are Alpaca's \
     (Black-Scholes).";

/// A premium per share: `1.60`, `0.05`.
pub fn premium(v: f64) -> String {
    format!("{v:.2}")
}

pub fn premium_opt(v: Option<f64>) -> String {
    v.filter(|v| v.is_finite())
        .map_or_else(|| DASH.to_owned(), premium)
}

/// A change in premium: `+0.12`, `-0.05`.
pub fn change_opt(v: Option<f64>) -> String {
    v.filter(|v| v.is_finite())
        .map_or_else(|| DASH.to_owned(), |v| format!("{v:+.2}"))
}

/// Implied volatility as a percentage: `20.3%`.
pub fn iv(v: Option<f64>) -> String {
    v.filter(|v| v.is_finite() && *v > 0.0)
        .map_or_else(|| DASH.to_owned(), |v| format!("{:.1}%", v * 100.0))
}

/// A greek to `dp` places, signed: `+0.51`, `-0.011`.
pub fn greek(v: Option<f64>, dp: usize) -> String {
    v.filter(|v| v.is_finite())
        .map_or_else(|| DASH.to_owned(), |v| format!("{v:+.dp$}"))
}

/// A count: `1,234`.
pub fn count(v: Option<f64>) -> String {
    v.filter(|v| v.is_finite())
        .map_or_else(|| DASH.to_owned(), crate::widgets::fmt::mw)
}

/// `Oct 16 · 14d`, `Mar 19 '27 · 168d`, `Oct 02 · today`.
pub fn expiry_label(expiry: NaiveDate, today: NaiveDate) -> String {
    let date = if expiry.year() == today.year() {
        expiry.format("%b %d").to_string()
    } else {
        expiry.format("%b %d '%y").to_string()
    };
    match (expiry - today).num_days() {
        ..=-1 => format!("{date} · expired"),
        0 => format!("{date} · today"),
        n => format!("{date} · {n}d"),
    }
}

/// Whether to mark an expiry as the month's standard one.
pub fn monthly(expiry: NaiveDate) -> bool {
    is_monthly(expiry)
}

/// The expiry a chain opens on: the first that is still trading (today's
/// until the close), else the last listed.
pub fn default_expiry(expiries: &[NaiveDate], now: DateTime<Utc>) -> Option<NaiveDate> {
    let local = exchange::to_exchange(now);
    let today = local.date_naive();
    let closed = local.time() >= chrono::NaiveTime::from_hms_opt(16, 0, 0).unwrap_or_default();
    expiries
        .iter()
        .copied()
        .find(|d| *d > today || (*d == today && !closed))
        .or_else(|| expiries.last().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn formats_and_expiries() {
        assert_eq!(premium_opt(Some(1.6)), "1.60");
        assert_eq!(premium_opt(None), DASH);
        assert_eq!(change_opt(Some(-0.05)), "-0.05");
        assert_eq!(iv(Some(0.2034)), "20.3%");
        assert_eq!(iv(Some(0.0)), DASH);
        assert_eq!(greek(Some(0.5053), 2), "+0.51");
        assert_eq!(greek(Some(-0.0111), 3), "-0.011");
        assert_eq!(count(Some(1234.0)), "1,234");
        let today = d("2026-10-02");
        assert_eq!(expiry_label(d("2026-10-16"), today), "Oct 16 · 14d");
        assert_eq!(expiry_label(d("2027-03-19"), today), "Mar 19 '27 · 168d");
        assert_eq!(expiry_label(today, today), "Oct 02 · today");
        let list = [d("2026-10-02"), d("2026-10-09")];
        // 15:00 and 17:00 New York time on the 2nd (EDT, UTC-4).
        let before: DateTime<Utc> = "2026-10-02T19:00:00Z".parse().unwrap();
        let after: DateTime<Utc> = "2026-10-02T21:00:00Z".parse().unwrap();
        assert_eq!(default_expiry(&list, before), Some(d("2026-10-02")));
        assert_eq!(default_expiry(&list, after), Some(d("2026-10-09")));
        assert_eq!(default_expiry(&list[..1], after), Some(d("2026-10-02")));
        assert_eq!(default_expiry(&[], after), None);
    }
}
