//! News headlines: what the terminal keeps from a publisher's feed (title,
//! summary, link, time, source), never article text. Topics are keyword rules
//! over them (`NI ENERGY`), and so are headline alerts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One headline as published.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Headline {
    /// Identity for de-duplication: the publisher's id for the story, or the
    /// link without tracking parameters (see [`canonical_link`]).
    pub id: String,
    /// The publisher, e.g. `Financial Times`.
    pub source: String,
    pub title: String,
    /// The feed's standfirst or summary, as plain text (may be empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The article as published: opened in the reader's browser.
    pub link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// When the publisher says it was published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<DateTime<Utc>>,
    /// When the terminal first saw it.
    pub seen: DateTime<Utc>,
    /// The feed sections it appeared in (`Markets`, `Energy`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<String>,
}

impl Headline {
    /// When the story is from, for sorting and display: the publisher's
    /// time, or when it was first seen if the feed gives none.
    pub fn time(&self) -> DateTime<Utc> {
        self.published.unwrap_or(self.seen)
    }

    /// Whether every word of `query` appears in the title, summary, source
    /// or sections, ignoring case. An empty query matches everything.
    pub fn matches_query(&self, query: &str) -> bool {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return true;
        }
        let hay = format!(
            "{}\n{}\n{}\n{}",
            self.title,
            self.summary,
            self.source,
            self.sections.join("\n")
        )
        .to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    }

    /// The text keyword rules look at: title and summary.
    fn text(&self) -> String {
        format!("{}\n{}", self.title, self.summary)
    }
}

/// A link reduced to what identifies the article: `https`, a lower-case
/// host, no fragment and no tracking parameters (`utm_*`, FT's `syn-*`, ...).
pub fn canonical_link(url: &str) -> String {
    let url = url.trim();
    let url = url.split('#').next().unwrap_or(url);
    let (scheme_host, rest) = match url.split_once("://") {
        Some((_, after)) => match after.find(['/', '?']) {
            Some(i) => (
                format!("https://{}", after[..i].to_lowercase()),
                &after[i..],
            ),
            None => (format!("https://{}", after.to_lowercase()), ""),
        },
        None => return url.to_owned(),
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !is_tracking_param(p.split('=').next().unwrap_or(p)))
        .collect();
    let path = path.trim_end_matches('/');
    if kept.is_empty() {
        format!("{scheme_host}{path}")
    } else {
        format!("{scheme_host}{path}?{}", kept.join("&"))
    }
}

fn is_tracking_param(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "cmpid",
        "ocid",
        "fbclid",
        "gclid",
        "dclid",
        "msclkid",
        "mc_cid",
        "mc_eid",
        "ref",
        "src",
        "smid",
        "sref",
        "srnd",
        "leadsource",
        "ftag",
        "taid",
        "tpcc",
        "wpisrc",
        "wpmm",
        "cid",
        "sh",
        "share",
    ];
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_")
        || name.starts_with("syn-")
        || name.starts_with("itm_")
        || EXACT.contains(&name.as_str())
}

/// One keyword or phrase, matched at word boundaries. Written in capitals
/// (`MISO`, `PJM`, `LNG`) it matches capitals only, so `MISO` is not the
/// soup; otherwise case is ignored. A trailing `*` matches any ending
/// (`utilit*` finds utility and utilities).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyword {
    text: String,
    exact_case: bool,
    prefix: bool,
}

impl Keyword {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (s, prefix) = match s.strip_suffix('*') {
            Some(rest) => (rest.trim_end(), true),
            None => (s, false),
        };
        if s.is_empty() {
            return None;
        }
        let exact_case = s.chars().any(char::is_alphabetic) && !s.chars().any(char::is_lowercase);
        let text = if exact_case {
            s.to_owned()
        } else {
            s.to_lowercase()
        };
        Some(Self {
            text,
            exact_case,
            prefix,
        })
    }

    /// Whether the keyword occurs in `text` (`lower` is `text` lower-cased).
    fn found(&self, text: &str, lower: &str) -> bool {
        let hay = if self.exact_case { text } else { lower };
        let first_word = self.text.starts_with(|c: char| c.is_alphanumeric());
        let last_word = !self.prefix && self.text.ends_with(|c: char| c.is_alphanumeric());
        hay.match_indices(self.text.as_str()).any(|(i, m)| {
            let before = hay[..i].chars().next_back();
            let after = hay[i + m.len()..].chars().next();
            (!first_word || before.is_none_or(|c| !c.is_alphanumeric()))
                && (!last_word || after.is_none_or(|c| !c.is_alphanumeric()))
        })
    }
}

impl std::fmt::Display for Keyword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)?;
        if self.prefix {
            f.write_str("*")?;
        }
        Ok(())
    }
}

/// Any of a set of keywords: a topic, a headline alert.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matcher {
    keywords: Vec<Keyword>,
}

impl Matcher {
    pub fn new<'a>(keywords: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            keywords: keywords.into_iter().filter_map(Keyword::parse).collect(),
        }
    }

    /// From a comma-separated list, as typed: `MISO, PJM, power prices`.
    pub fn from_list(list: &str) -> Self {
        Self::new(list.split(','))
    }

    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
    }

    /// Whether any keyword occurs in `text`.
    pub fn matches_text(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        self.keywords.iter().any(|k| k.found(text, &lower))
    }

    /// Whether any keyword occurs in the headline's title or summary.
    pub fn matches(&self, h: &Headline) -> bool {
        self.matches_text(&h.text())
    }
}

/// A named set of keywords for `NI <topic>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    /// Upper case, typed after `NI`: `ENERGY`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Words or phrases (see [`Keyword`]); any one of them is a match.
    pub keywords: Vec<String>,
}

impl Topic {
    pub fn matcher(&self) -> Matcher {
        Matcher::new(self.keywords.iter().map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headline(title: &str, summary: &str) -> Headline {
        Headline {
            id: title.into(),
            source: "Test".into(),
            title: title.into(),
            summary: summary.into(),
            link: String::new(),
            author: None,
            published: None,
            seen: DateTime::UNIX_EPOCH,
            sections: vec!["Markets".into()],
        }
    }

    #[test]
    fn keywords_match_whole_words_and_capitals_only_match_capitals() {
        let m = Matcher::from_list("MISO, natural gas, utilit*, grid");
        assert!(m.matches_text("MISO's capacity auction clears high"));
        assert!(m.matches_text("MISO-wide prices"));
        assert!(!m.matches_text("A miso-glazed salmon for autumn"));
        assert!(m.matches_text("Natural Gas futures slide"));
        assert!(m.matches_text("Utilities rally"));
        assert!(m.matches_text("the utility said"));
        assert!(!m.matches_text("Gridlock in Congress"));
        assert!(m.matches_text("A strained grid."));
        assert!(!m.matches_text("Las Vegas casinos"));
        assert!(Matcher::from_list(" , ").is_empty());
        assert!(Matcher::from_list("S&P 500").matches_text("The S&P 500 closed higher"));
    }

    #[test]
    fn queries_need_every_word() {
        let h = headline("Henry Hub gas jumps", "Cold snap lifts demand");
        assert!(h.matches_query(""));
        assert!(h.matches_query("gas COLD"));
        assert!(h.matches_query("markets"), "sections count");
        assert!(!h.matches_query("gas oil"));
    }

    #[test]
    fn links_lose_tracking_parameters() {
        assert_eq!(
            canonical_link("https://www.FT.com/content/2084f349-0829?syn-25a6b1a6=1#comments"),
            "https://www.ft.com/content/2084f349-0829"
        );
        assert_eq!(
            canonical_link("http://example.com/a/?id=7&utm_source=rss&utm_medium=x"),
            "https://example.com/a?id=7"
        );
        assert_eq!(canonical_link("not a url"), "not a url");
    }

    #[test]
    fn headlines_round_trip_and_sort_by_time() {
        let mut h = headline("A", "");
        assert_eq!(h.time(), h.seen);
        h.published = Some(DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        assert_eq!(h.time(), h.published.unwrap());
        let json = serde_json::to_string(&h).unwrap();
        assert!(
            !json.contains("summary"),
            "empty fields are left out: {json}"
        );
        assert_eq!(serde_json::from_str::<Headline>(&json).unwrap(), h);
    }
}
