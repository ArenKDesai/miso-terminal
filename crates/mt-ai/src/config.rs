//! `[ai]` in `config.toml`: which model answers, how hard it thinks, and the
//! limits on rounds, tokens and money. Model ids and prices are
//! configuration, not constants, so a new model or a price change needs no
//! new release.

use serde::{Deserialize, Serialize};

/// How much thinking (and so how many tokens) an answer gets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    #[default]
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// A model the user can pick, with its prices in US dollars per million tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    /// Writing to the five-minute prompt cache.
    pub cache_write_per_mtok: f64,
    /// Ask the API to retry a declined request on the model Anthropic
    /// recommends (server-side fallback). Not every model has one.
    #[serde(default)]
    pub fallback: bool,
}

impl ModelInfo {
    fn new(id: &str, label: &str, prices: [f64; 4], fallback: bool) -> Self {
        let [input, output, cache_read, cache_write] = prices;
        Self {
            id: id.into(),
            label: label.into(),
            input_per_mtok: input,
            output_per_mtok: output,
            cache_read_per_mtok: cache_read,
            cache_write_per_mtok: cache_write,
            fallback,
        }
    }
}

/// The models offered out of the box (Anthropic's list prices, October 2026).
pub fn builtin_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo::new(
            "claude-opus-5-5",
            "Claude Opus 5.5",
            [4.0, 20.0, 0.20, 5.0],
            true,
        ),
        ModelInfo::new(
            "claude-sonnet-5-5",
            "Claude Sonnet 5.5",
            [2.0, 10.0, 0.20, 2.50],
            true,
        ),
        ModelInfo::new(
            "claude-haiku-5-5",
            "Claude Haiku 5.5",
            [0.10, 0.50, 0.01, 0.125],
            false,
        ),
    ]
}

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Web searches cost per thousand, on top of tokens.
pub const WEB_SEARCH_PER_THOUSAND: f64 = 10.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// A model id from `models` (or any id the API accepts; unknown ids are
    /// priced like the most expensive listed model, so the cap still holds).
    pub model: String,
    pub effort: Effort,
    /// Spending cap per calendar month, US dollars. `0` sends nothing.
    pub monthly_cap_usd: f64,
    /// Requests per question (each tool round is one).
    pub max_rounds: u32,
    /// Output tokens per request.
    pub max_tokens: u32,
    /// Input plus output tokens per question, across its rounds.
    pub max_question_tokens: u64,
    /// Let Claude search the web when the terminal has no source (cited).
    pub web_search: bool,
    /// Let the assistant see the paper account (positions and P&L). Also
    /// governs the MCP server's `paper_account` tool.
    pub share_paper_account: bool,
    /// Extra or replacement models, merged over the built-in list by id
    /// (`[[ai.models]]`). Left out of the file when empty, so a hand-written
    /// table never clashes with a `models = []` the terminal wrote.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelInfo>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            effort: Effort::Low,
            monthly_cap_usd: 0.0,
            max_rounds: 12,
            max_tokens: 16_000,
            max_question_tokens: 400_000,
            web_search: false,
            share_paper_account: false,
            models: Vec::new(),
        }
    }
}

impl AiConfig {
    /// The built-in models with the configured ones merged over them.
    pub fn model_list(&self) -> Vec<ModelInfo> {
        let mut list = builtin_models();
        for m in &self.models {
            match list.iter_mut().find(|b| b.id == m.id) {
                Some(b) => *b = m.clone(),
                None => list.push(m.clone()),
            }
        }
        list
    }

    /// The chosen model's details. An id nobody listed is priced like the
    /// dearest model, so a cap can only be reached early, never overrun.
    pub fn model_info(&self) -> ModelInfo {
        let list = self.model_list();
        if let Some(m) = list.iter().find(|m| m.id == self.model) {
            return m.clone();
        }
        let dearest = list
            .iter()
            .max_by(|a, b| a.output_per_mtok.total_cmp(&b.output_per_mtok))
            .cloned()
            .unwrap_or_else(|| builtin_models().remove(0));
        ModelInfo {
            id: self.model.clone(),
            label: self.model.clone(),
            fallback: false,
            ..dearest
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_send_nothing_and_merge_models() {
        let c = AiConfig::default();
        assert_eq!(c.monthly_cap_usd, 0.0);
        assert_eq!(c.effort, Effort::Low);
        assert!(!c.share_paper_account && !c.web_search);
        assert_eq!(c.model_info().id, "claude-opus-5-5");

        let toml_text = r#"
            model = "claude-next"
            effort = "high"
            [[models]]
            id = "claude-sonnet-5-5"
            label = "Sonnet"
            input_per_mtok = 1.0
            output_per_mtok = 5.0
            cache_read_per_mtok = 0.1
            cache_write_per_mtok = 1.25
        "#;
        let c: AiConfig = toml::from_str(toml_text).expect("config");
        assert_eq!(c.effort, Effort::High);
        assert_eq!(c.max_rounds, 12, "missing keys take defaults");
        let list = c.model_list();
        assert_eq!(list.len(), 3);
        assert_eq!(
            list.iter()
                .find(|m| m.id == "claude-sonnet-5-5")
                .map(|m| m.input_per_mtok),
            Some(1.0)
        );
        let unknown = c.model_info();
        assert_eq!(unknown.id, "claude-next");
        assert_eq!(unknown.output_per_mtok, 20.0, "priced like the dearest");
    }
}
