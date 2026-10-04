//! Money: exact decimals wherever an order is involved, never `f64`.
//!
//! Prices, quantities (fractional shares are allowed), notionals, cash and
//! P&L that feed an order or a position are [`Decimal`]s. Brokers send them as
//! strings (`"82.51"`) to keep them exact, and `Decimal` deserialises from
//! those directly. Charts and quote displays may still use `f64`.

use std::str::FromStr;

pub use rust_decimal::{Decimal, RoundingStrategy};

/// Parse an amount: `82.51`, `$1,234.50`, `-.5`, ` 10 `. `None` for blanks,
/// placeholders and anything that is not exactly a number.
pub fn parse_decimal(s: &str) -> Option<Decimal> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("null") {
        return None;
    }
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | ' '))
        .collect();
    // Plain decimals only: no exponents (`1e3` in a quantity box is a typo).
    if !cleaned
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-'))
    {
        return None;
    }
    let cleaned = match cleaned.strip_prefix('-') {
        Some(rest) if rest.starts_with('.') => format!("-0{rest}"),
        _ if cleaned.starts_with('.') => format!("0{cleaned}"),
        _ => cleaned,
    };
    Decimal::from_str(&cleaned).ok()
}

/// Group thousands: `1234567.5` -> `1,234,567.5`.
fn group(digits: &str) -> String {
    let (int, frac) = digits
        .split_once('.')
        .map_or((digits, None), |(i, f)| (i, Some(f)));
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// Dollars to `dp` places, rounding half away from zero: `$1,234.50`, `-$3.00`.
pub fn fmt_usd(v: Decimal, dp: u32) -> String {
    let r = v.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero);
    let digits = format!("{:.*}", dp as usize, r.abs());
    let sign = if r.is_sign_negative() && !r.is_zero() {
        "-"
    } else {
        ""
    };
    format!("{sign}${}", group(&digits))
}

/// A quantity with as many decimals as it has: `10`, `0.5`, `1,000`.
pub fn fmt_qty(v: Decimal) -> String {
    let n = v.normalize();
    let sign = if n.is_sign_negative() && !n.is_zero() {
        "-"
    } else {
        ""
    };
    format!("{sign}{}", group(&n.abs().to_string()))
}

/// Round `price` to a multiple of `tick` (`0.01`, `0.05`) with `strategy`
/// (down for a buy limit, up for a sell, or to nearest).
pub fn round_to_tick(price: Decimal, tick: Decimal, strategy: RoundingStrategy) -> Decimal {
    if tick <= Decimal::ZERO {
        return price;
    }
    ((price / tick).round_dp_with_strategy(0, strategy) * tick).normalize()
}

pub fn is_on_tick(price: Decimal, tick: Decimal) -> bool {
    tick > Decimal::ZERO && (price % tick).is_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn parses_exactly() {
        assert_eq!(parse_decimal("82.51"), Some(d("82.51")));
        assert_eq!(parse_decimal("$1,234.50"), Some(d("1234.50")));
        assert_eq!(parse_decimal("-.5"), Some(d("-0.5")));
        assert_eq!(parse_decimal(".25"), Some(d("0.25")));
        assert_eq!(parse_decimal(" 10 "), Some(d("10")));
        assert_eq!(parse_decimal("None"), None);
        assert_eq!(parse_decimal("1e3"), None);
        assert_eq!(parse_decimal("NaN"), None);
        // The classic float trap stays exact.
        assert_eq!(d("0.1") + d("0.2"), d("0.3"));
    }

    #[test]
    fn formats_money_and_quantities() {
        assert_eq!(fmt_usd(d("1234.5"), 2), "$1,234.50");
        assert_eq!(fmt_usd(d("-3"), 2), "-$3.00");
        assert_eq!(fmt_usd(d("999999.995"), 2), "$1,000,000.00");
        assert_eq!(fmt_usd(d("-0.001"), 2), "$0.00");
        assert_eq!(fmt_usd(d("0.0042"), 4), "$0.0042");
        assert_eq!(fmt_qty(d("10.000")), "10");
        assert_eq!(fmt_qty(d("0.50")), "0.5");
        assert_eq!(fmt_qty(d("-1500")), "-1,500");
    }

    #[test]
    fn ticks() {
        let cent = d("0.01");
        assert_eq!(
            round_to_tick(d("82.517"), cent, RoundingStrategy::ToZero),
            d("82.51")
        );
        assert_eq!(
            round_to_tick(d("82.511"), cent, RoundingStrategy::AwayFromZero),
            d("82.52")
        );
        assert_eq!(
            round_to_tick(d("1.37"), d("0.05"), RoundingStrategy::MidpointNearestEven),
            d("1.35")
        );
        assert!(is_on_tick(d("82.50"), cent) && !is_on_tick(d("82.505"), cent));
        assert!(!is_on_tick(d("1"), Decimal::ZERO));
    }

    #[test]
    fn broker_json_deserialises_exactly() {
        #[derive(serde::Deserialize)]
        struct Fill {
            qty: Decimal,
            filled_avg_price: Decimal,
            notional: Option<Decimal>,
        }
        // Alpaca's trading API sends amounts as strings.
        let f: Fill =
            serde_json::from_str(r#"{"qty":"10","filled_avg_price":"82.51","notional":null}"#)
                .unwrap();
        assert_eq!(f.qty * f.filled_avg_price, d("825.10"));
        assert_eq!(f.notional, None);
        // Plain JSON numbers (market data) arrive exactly too.
        let f: Fill =
            serde_json::from_str(r#"{"qty":0.1,"filled_avg_price":82.51,"notional":825.1}"#)
                .unwrap();
        assert_eq!((f.qty, f.filled_avg_price), (d("0.1"), d("82.51")));
    }
}
