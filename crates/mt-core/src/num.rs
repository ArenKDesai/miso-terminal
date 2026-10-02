//! Lenient numeric parsing for MISO's stringly-typed payloads.
//!
//! MISO sends numbers as strings, sometimes with thousands separators (`"84,160"`),
//! currency symbols (`"$36.92"`), bare decimals (`".71"`), or placeholders
//! (`"None"`, `""`). Everything funnels through [`parse_num`] so one function
//! decides what counts as a number.

/// Parse a MISO number. Returns `None` for blanks and placeholders.
pub fn parse_num(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("null") {
        return None;
    }
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | ' '))
        .collect();
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// [`parse_num`] for optional JSON fields.
pub fn parse_opt(s: Option<&str>) -> Option<f64> {
    s.and_then(parse_num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_miso_spellings() {
        assert_eq!(parse_num("84,160"), Some(84160.0));
        assert_eq!(parse_num("$36.92"), Some(36.92));
        assert_eq!(parse_num(".71"), Some(0.71));
        assert_eq!(parse_num("-.3"), Some(-0.3));
        assert_eq!(parse_num(" -257.5 "), Some(-257.5));
        assert_eq!(parse_num("None"), None);
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("n/a"), None);
        assert_eq!(parse_num("NaN"), None);
    }
}
