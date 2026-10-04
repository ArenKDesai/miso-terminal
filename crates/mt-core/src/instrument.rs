//! Instruments: what a command or panel is about.
//!
//! MISO pricing nodes and securities share the command line, and some node
//! names (`AECI`, `TVA`, `SOCO`) look like tickers. So securities always carry
//! a market code, Bloomberg style: `XLU US` is a security, `AECI` is a node,
//! and `XLU US GP 30` charts the security. Options use OCC symbols
//! (`XLU261218C00082500`): root, expiry `YYMMDD`, `C`/`P`, strike × 1000 in
//! eight digits.

use std::fmt;

use chrono::NaiveDate;
use rust_decimal::Decimal;

/// Where a security trades. Only US markets for now (through Alpaca).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Market {
    Us,
}

impl Market {
    pub fn code(self) -> &'static str {
        match self {
            Self::Us => "US",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        s.eq_ignore_ascii_case("US").then_some(Self::Us)
    }
}

/// A listed stock or ETF: `XLU US`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Security {
    /// Upper case, as the exchange lists it (`BRK.B`).
    pub ticker: String,
    pub market: Market,
}

impl Security {
    pub fn us(ticker: &str) -> Self {
        Self {
            ticker: ticker.to_ascii_uppercase(),
            market: Market::Us,
        }
    }

    /// `XLU US`, `xlu us` or `XLU US Equity`.
    pub fn parse(s: &str) -> Option<Self> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        match parse_tokens(&tokens) {
            Some((Instrument::Security(sec), n)) if n == tokens.len() => Some(sec),
            _ => None,
        }
    }
}

impl fmt::Display for Security {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.ticker, self.market.code())
    }
}

/// Whether `s` can be a ticker: a letter, then up to nine letters, digits or
/// class separators (`BRK.B`, `BF/B`, `RDS-A`).
pub fn is_ticker(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.len() <= 10
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OptionRight {
    Call,
    Put,
}

impl OptionRight {
    pub fn letter(self) -> char {
        match self {
            Self::Call => 'C',
            Self::Put => 'P',
        }
    }
}

/// A listed option, named by its OCC symbol.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OptionContract {
    pub underlying: String,
    pub expiry: NaiveDate,
    pub right: OptionRight,
    /// Dollars per share, exact.
    pub strike: Decimal,
}

impl OptionContract {
    /// Parse an OCC symbol, compact (`XLU261218C00082500`, Alpaca's form) or
    /// padded to six root characters (`XLU   261218C00082500`).
    pub fn parse_occ(s: &str) -> Option<Self> {
        let s = s.trim();
        // The fixed-width tail: YYMMDD + C/P + 8-digit strike.
        let tail_at = s.len().checked_sub(15)?;
        let (root, tail) = (s.get(..tail_at)?.trim_end(), s.get(tail_at..)?);
        if root.is_empty()
            || root.len() > 6
            || !root.chars().all(|c| c.is_ascii_alphanumeric())
            || !root.starts_with(|c: char| c.is_ascii_alphabetic())
        {
            return None;
        }
        let (date, rest) = tail.split_at(6);
        let (right, strike) = rest.split_at(1);
        if !date.bytes().all(|b| b.is_ascii_digit()) || !strike.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let expiry = NaiveDate::parse_from_str(&format!("20{date}"), "%Y%m%d").ok()?;
        let right = match right {
            "C" | "c" => OptionRight::Call,
            "P" | "p" => OptionRight::Put,
            _ => return None,
        };
        let strike = Decimal::new(strike.parse::<i64>().ok()?, 3).normalize();
        Some(Self {
            underlying: root.to_ascii_uppercase(),
            expiry,
            right,
            strike,
        })
    }

    /// The compact OCC symbol, as Alpaca writes it. `None` if the strike does
    /// not fit OCC's format (negative, more than three decimals, or ≥ $100,000).
    pub fn occ(&self) -> Option<String> {
        let thousandths = (self.strike * Decimal::ONE_THOUSAND).normalize();
        if thousandths.is_sign_negative() || thousandths.scale() > 0 {
            return None;
        }
        let n: u64 = thousandths.to_string().parse().ok()?;
        (n < 100_000_000).then(|| {
            format!(
                "{}{}{}{n:08}",
                self.underlying,
                self.expiry.format("%y%m%d"),
                self.right.letter()
            )
        })
    }
}

impl fmt::Display for OptionContract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.occ() {
            Some(occ) => f.write_str(&occ),
            None => write!(
                f,
                "{} {} {}{}",
                self.underlying,
                self.expiry.format("%y%m%d"),
                self.right.letter(),
                self.strike
            ),
        }
    }
}

/// Anything a function can be about.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Instrument {
    /// A MISO commercial pricing node, such as `MINN.HUB` or `AECI`.
    Node(String),
    Security(Security),
    Option(OptionContract),
}

impl Instrument {
    /// A security or option written out in full (`XLU US`, an OCC symbol).
    /// Nodes are not recognised here: which names are nodes is data, not syntax.
    pub fn parse_security(s: &str) -> Option<Self> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        parse_tokens(&tokens).and_then(|(i, n)| (n == tokens.len()).then_some(i))
    }

    pub fn is_node(&self) -> bool {
        matches!(self, Self::Node(_))
    }
}

impl fmt::Display for Instrument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(n) => f.write_str(n),
            Self::Security(s) => s.fmt(f),
            Self::Option(o) => o.fmt(f),
        }
    }
}

/// Read a security or option from the start of a command's tokens: a ticker
/// followed by a market code (and optionally Bloomberg's `Equity`), or one OCC
/// symbol. Returns it with the number of tokens used.
pub fn parse_tokens(tokens: &[&str]) -> Option<(Instrument, usize)> {
    let first = *tokens.first()?;
    if let Some(market) = tokens.get(1).and_then(|t| Market::parse(t))
        && is_ticker(first)
    {
        let equity = tokens
            .get(2)
            .is_some_and(|t| t.eq_ignore_ascii_case("EQUITY"));
        let sec = Security {
            ticker: first.to_ascii_uppercase(),
            market,
        };
        return Some((Instrument::Security(sec), if equity { 3 } else { 2 }));
    }
    OptionContract::parse_occ(first).map(|o| (Instrument::Option(o), 1))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn securities_need_a_market_code() {
        assert_eq!(Security::parse("xlu us"), Some(Security::us("XLU")));
        assert_eq!(Security::parse("XLU US Equity"), Some(Security::us("XLU")));
        assert_eq!(Security::parse("BRK.B US").unwrap().ticker, "BRK.B");
        assert_eq!(Security::us("xlu").to_string(), "XLU US");
        // Node names stay nodes.
        for node in ["AECI", "TVA", "MINN.HUB", "ALTE.ALTE", "XLU US GP"] {
            assert_eq!(Security::parse(node), None, "{node}");
        }
        assert_eq!(Security::parse("XLU UK"), None);
        assert_eq!(Security::parse("1ABC US"), None);
    }

    #[test]
    fn tokens_are_read_from_the_front() {
        let (i, n) = parse_tokens(&["XLU", "US", "GP", "30"]).unwrap();
        assert_eq!((i.to_string(), n), ("XLU US".into(), 2));
        let (_, n) = parse_tokens(&["xlu", "us", "equity", "GP"]).unwrap();
        assert_eq!(n, 3);
        assert_eq!(parse_tokens(&["GP", "XLU", "US"]), None);
        assert_eq!(parse_tokens(&["MINN.HUB"]), None);
        assert_eq!(parse_tokens(&[]), None);
        let (i, n) = parse_tokens(&["XLU261218C00082500", "GP"]).unwrap();
        assert!(matches!(i, Instrument::Option(_)) && n == 1);
    }

    #[test]
    fn occ_symbols_round_trip() {
        let o = OptionContract::parse_occ("XLU261218C00082500").unwrap();
        assert_eq!(o.underlying, "XLU");
        assert_eq!(o.expiry, NaiveDate::from_ymd_opt(2026, 12, 18).unwrap());
        assert_eq!(o.right, OptionRight::Call);
        assert_eq!(o.strike, Decimal::from_str("82.5").unwrap());
        assert_eq!(o.occ().as_deref(), Some("XLU261218C00082500"));
        let padded = OptionContract::parse_occ("SPY   270115P00450000").unwrap();
        assert_eq!(padded.strike, Decimal::from(450));
        assert_eq!(padded.occ().as_deref(), Some("SPY270115P00450000"));
        let fine = OptionContract::parse_occ("GE261120C00002125").unwrap();
        assert_eq!(fine.strike.to_string(), "2.125");
        assert_eq!(
            Instrument::parse_security("XLU261218C00082500").map(|i| i.to_string()),
            Some("XLU261218C00082500".into())
        );
        for bad in [
            "XLU261218X00082500",
            "XLU261318C00082500",
            "TOOLONGROOT261218C00082500",
            "261218C00082500",
            "MINN.HUB",
        ] {
            assert_eq!(OptionContract::parse_occ(bad), None, "{bad}");
        }
        let odd = OptionContract {
            strike: Decimal::from_str("1.2345").unwrap(),
            ..o
        };
        assert_eq!(odd.occ(), None, "OCC strikes have three decimals");
    }
}
