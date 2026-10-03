//! Number and time formatting. One place decides how a price or a megawatt
//! figure looks everywhere.

use chrono::{DateTime, NaiveDateTime, Utc};

pub const DASH: &str = "—";

/// $/MWh with two decimals.
pub fn price(v: f64) -> String {
    format!("{v:.2}")
}

pub fn price_opt(v: Option<f64>) -> String {
    v.map_or_else(|| DASH.to_owned(), price)
}

/// Signed two-decimal change.
pub fn signed(v: f64) -> String {
    format!("{v:+.2}")
}

/// Whole megawatts with thousands separators.
pub fn mw(v: f64) -> String {
    group(v.round() as i64)
}

pub fn mw_opt(v: Option<f64>) -> String {
    v.map_or_else(|| DASH.to_owned(), mw)
}

/// Signed whole megawatts with thousands separators.
pub fn mw_signed(v: f64) -> String {
    let n = v.round() as i64;
    if n > 0 {
        format!("+{}", group(n))
    } else {
        group(n)
    }
}

pub fn pct(v: f64) -> String {
    format!("{v:.1}%")
}

fn group(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// `16:35`
pub fn hm(t: NaiveDateTime) -> String {
    t.format("%H:%M").to_string()
}

/// `Oct 02 16:35`
pub fn day_hm(t: NaiveDateTime) -> String {
    t.format("%b %d %H:%M").to_string()
}

/// `12s ago`, `3m ago`, `2h ago`.
pub fn ago(t: DateTime<Utc>) -> String {
    let s = (mt_core::time::now_utc() - t).num_seconds().max(0);
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(mw(84160.4), "84,160");
        assert_eq!(mw(-1131.0), "-1,131");
        assert_eq!(mw(999.0), "999");
        assert_eq!(mw(1_234_567.0), "1,234,567");
        assert_eq!(mw_signed(2157.0), "+2,157");
        assert_eq!(price(32.271), "32.27");
        assert_eq!(signed(-1.2), "-1.20");
        assert_eq!(price_opt(None), DASH);
    }
}
