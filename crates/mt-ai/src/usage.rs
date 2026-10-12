//! Tokens, what they cost, and the monthly spending cap.

use serde::{Deserialize, Serialize};

use crate::config::{ModelInfo, WEB_SEARCH_PER_THOUSAND};

/// Tokens (and searches) used, as the API reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Input not read from or written to the cache.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_searches: u64,
}

impl Usage {
    /// From a `usage` object (`message_start`, `message_delta`). Fields the
    /// object leaves out keep their value: `message_delta` repeats only some.
    pub fn merge_json(&mut self, v: &serde_json::Value) {
        let get = |k: &str| v.get(k).and_then(serde_json::Value::as_u64);
        if let Some(n) = get("input_tokens") {
            self.input_tokens = n;
        }
        if let Some(n) = get("output_tokens") {
            self.output_tokens = n;
        }
        if let Some(n) = get("cache_read_input_tokens") {
            self.cache_read_input_tokens = n;
        }
        if let Some(n) = get("cache_creation_input_tokens") {
            self.cache_creation_input_tokens = n;
        }
        if let Some(n) = v
            .get("server_tool_use")
            .and_then(|s| s.get("web_search_requests"))
            .and_then(serde_json::Value::as_u64)
        {
            self.web_searches = n;
        }
    }

    pub fn add(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.web_searches += other.web_searches;
    }

    /// Every token sent or received, cached or not (for the per-question cap).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_input_tokens
            + self.cache_creation_input_tokens
    }

    /// US dollars at this model's prices.
    pub fn cost(&self, m: &ModelInfo) -> f64 {
        let per = |n: u64, price: f64| n as f64 * price / 1_000_000.0;
        per(self.input_tokens, m.input_per_mtok)
            + per(self.output_tokens, m.output_per_mtok)
            + per(self.cache_read_input_tokens, m.cache_read_per_mtok)
            + per(self.cache_creation_input_tokens, m.cache_write_per_mtok)
            + self.web_searches as f64 * WEB_SEARCH_PER_THOUSAND / 1000.0
    }
}

/// What has been spent this month. The app keeps one on disk; the
/// assistant checks it before every request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    /// `2026-10`: the month `spent_usd` belongs to (UTC, as Anthropic bills).
    pub month: String,
    pub spent_usd: f64,
    pub requests: u64,
}

impl Ledger {
    pub fn month_of(now: chrono::DateTime<chrono::Utc>) -> String {
        now.format("%Y-%m").to_string()
    }

    /// Spent so far in `month`; a ledger from an earlier month counts as nothing.
    pub fn spent_in(&self, month: &str) -> f64 {
        if self.month == month {
            self.spent_usd
        } else {
            0.0
        }
    }

    pub fn record(&mut self, month: &str, usd: f64) {
        if self.month != month {
            *self = Self {
                month: month.to_owned(),
                ..Self::default()
            };
        }
        self.spent_usd += usd;
        self.requests += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::builtin_models;

    #[test]
    fn costs_from_reported_usage() {
        let opus = &builtin_models()[0];
        let mut u = Usage::default();
        u.merge_json(&serde_json::json!({
            "input_tokens": 1000, "cache_read_input_tokens": 20000,
            "cache_creation_input_tokens": 0, "output_tokens": 1,
        }));
        u.merge_json(&serde_json::json!({"output_tokens": 500,
            "server_tool_use": {"web_search_requests": 2}}));
        assert_eq!(u.input_tokens, 1000, "kept when message_delta omits it");
        assert_eq!(u.output_tokens, 500);
        // 1000×4 + 500×20 + 20000×0.20 per million, plus 2 searches at $10/1000.
        let want = (4000.0 + 10000.0 + 4000.0) / 1e6 + 0.02;
        assert!((u.cost(opus) - want).abs() < 1e-12, "{}", u.cost(opus));
        assert_eq!(u.total_tokens(), 21_500);
    }

    #[test]
    fn the_ledger_starts_over_each_month() {
        let mut l = Ledger::default();
        l.record("2026-10", 0.25);
        l.record("2026-10", 0.5);
        assert_eq!(l.spent_in("2026-10"), 0.75);
        assert_eq!(l.spent_in("2026-11"), 0.0);
        l.record("2026-11", 0.1);
        assert_eq!((l.month.as_str(), l.requests), ("2026-11", 1));
        let t = chrono::DateTime::parse_from_rfc3339("2026-10-31T23:30:00-05:00")
            .expect("time")
            .with_timezone(&chrono::Utc);
        assert_eq!(Ledger::month_of(t), "2026-11");
    }
}
