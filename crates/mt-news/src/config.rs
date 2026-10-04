//! Which feeds to read and which topics to offer. Built-in lists live here,
//! so a new release can fix a feed that moved; `config.toml` (`[news]`) can
//! turn built-in feeds off, add feeds and topics, or replace them by id.

use mt_core::news::Topic;
use serde::{Deserialize, Serialize};

/// One RSS or Atom feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedSpec {
    /// Unique and stable, e.g. `ft-markets`: names the feed in config, the
    /// log and the on-disk archive.
    pub id: String,
    /// The publisher, shown with every headline: `Financial Times`.
    pub source: String,
    /// The section, shown beside each headline: `Markets`.
    #[serde(default)]
    pub section: String,
    pub url: String,
    /// Part of the top stories (TOP and HOME).
    #[serde(default)]
    pub top: bool,
}

impl FeedSpec {
    fn new(id: &str, source: &str, section: &str, url: &str, top: bool) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            section: section.into(),
            url: url.into(),
            top,
        }
    }
}

/// A publisher the terminal knows by a short code.
pub struct Source {
    pub name: &'static str,
    /// Shown in the source column: `FT`.
    pub short: &'static str,
    /// What `NEWS <source>` accepts, upper case.
    pub aliases: &'static [&'static str],
}

pub const FT: &str = "Financial Times";
pub const BLOOMBERG: &str = "Bloomberg";
pub const WAPO: &str = "The Washington Post";

pub const SOURCES: &[Source] = &[
    Source {
        name: FT,
        short: "FT",
        aliases: &["FT", "FINANCIAL", "FINANCIALTIMES"],
    },
    Source {
        name: BLOOMBERG,
        short: "BBG",
        aliases: &["BBG", "BLOOMBERG", "BB"],
    },
    Source {
        name: WAPO,
        short: "WP",
        aliases: &["WP", "WAPO", "POST", "WASHINGTONPOST"],
    },
];

/// The short code for a publisher: `FT`, `BBG`, `WP`, or the initials of
/// one the terminal does not know (`Energy News Network` -> `ENN`).
pub fn short_name(source: &str) -> String {
    if let Some(s) = SOURCES.iter().find(|s| s.name.eq_ignore_ascii_case(source)) {
        return s.short.to_owned();
    }
    let initials: String = source
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("the"))
        .filter_map(|w| w.chars().next())
        .take(4)
        .collect::<String>()
        .to_uppercase();
    if initials.is_empty() {
        "?".into()
    } else {
        initials
    }
}

/// The publisher a `NEWS` argument names, among `sources` (the configured
/// publishers): a known alias, a short code, or the name without spaces.
pub fn source_named<'a>(word: &str, sources: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let word: String = word
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    if word.is_empty() {
        return None;
    }
    sources.into_iter().find(|name| {
        let squashed: String = name
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_uppercase();
        let known = SOURCES
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .is_some_and(|s| s.aliases.contains(&word.as_str()));
        known
            || short_name(name) == word
            || squashed == word
            || squashed.strip_prefix("THE") == Some(word.as_str())
    })
}

/// The feeds the terminal reads unless told otherwise. Public RSS headline
/// feeds; checked 2026-10-04. Every one is allowed by its site's robots.txt.
pub fn builtin_feeds() -> Vec<FeedSpec> {
    let ft = |id, section, url, top| FeedSpec::new(id, FT, section, url, top);
    let bbg = |id, section: &str, top| {
        let path = section.to_lowercase();
        FeedSpec::new(
            id,
            BLOOMBERG,
            section,
            &format!("https://www.bloomberg.com/feeds/{path}/news.rss"),
            top,
        )
    };
    let wp = |id, section, path: &str, top| {
        FeedSpec::new(
            id,
            WAPO,
            section,
            &format!("https://feeds.washingtonpost.com/rss/{path}"),
            top,
        )
    };
    vec![
        ft(
            "ft-home",
            "Top stories",
            "https://www.ft.com/rss/home/international",
            true,
        ),
        ft(
            "ft-markets",
            "Markets",
            "https://www.ft.com/markets?format=rss",
            false,
        ),
        ft(
            "ft-energy",
            "Energy",
            "https://www.ft.com/energy?format=rss",
            false,
        ),
        ft(
            "ft-global-economy",
            "Global economy",
            "https://www.ft.com/global-economy?format=rss",
            false,
        ),
        bbg("bloomberg-markets", "Markets", true),
        bbg("bloomberg-economics", "Economics", false),
        bbg("bloomberg-industries", "Industries", false),
        bbg("bloomberg-politics", "Politics", false),
        bbg("bloomberg-technology", "Technology", false),
        wp("wapo-business", "Business", "business", true),
        wp("wapo-economy", "Economy", "business/economy", false),
        wp("wapo-politics", "Politics", "politics", false),
        wp("wapo-national", "National", "national", false),
    ]
}

/// Topics for `NI`. Keywords follow [`mt_core::news::Keyword`]: capitals
/// match capitals only, `*` matches any ending.
pub fn builtin_topics() -> Vec<Topic> {
    let topic = |name: &str, description: &str, keywords: &[&str]| Topic {
        name: name.into(),
        description: description.into(),
        keywords: keywords.iter().map(|k| (*k).to_owned()).collect(),
    };
    vec![
        topic(
            "ENERGY",
            "Power, gas, oil, coal, nuclear and renewables",
            &[
                "energy",
                "electricity",
                "power prices",
                "power plant*",
                "power grid",
                "utilit*",
                "natural gas",
                "LNG",
                "oil",
                "crude",
                "OPEC",
                "coal",
                "nuclear",
                "renewable*",
                "solar",
                "wind farm*",
                "wind power",
                "offshore wind",
                "battery storage",
                "pipeline*",
                "refiner*",
                "gasoline",
                "diesel",
            ],
        ),
        topic(
            "POWER",
            "Electricity: prices, plants, grids, outages and data-centre demand",
            &[
                "electricity",
                "power prices",
                "power price",
                "power demand",
                "power supply",
                "power plant*",
                "power grid",
                "power market*",
                "power generation",
                "power station*",
                "power outage*",
                "power line*",
                "blackout*",
                "grid",
                "data center*",
                "data centre*",
                "transmission line*",
                "interconnection",
                "capacity auction*",
                "capacity market*",
            ],
        ),
        topic(
            "GRID",
            "Grid operators, regulators and transmission",
            &[
                "MISO",
                "PJM",
                "ERCOT",
                "SPP",
                "CAISO",
                "NYISO",
                "ISO-NE",
                "FERC",
                "NERC",
                "grid operator*",
                "transmission line*",
                "power transmission",
                "interconnection queue*",
                "capacity auction*",
            ],
        ),
        topic(
            "GAS",
            "Natural gas and LNG",
            &[
                "natural gas",
                "LNG",
                "Henry Hub",
                "gas-fired",
                "gas pipeline*",
                "gas storage",
                "gas turbine*",
                "shale gas",
                "gas supplies",
                "gas exports",
            ],
        ),
        topic(
            "OIL",
            "Crude, OPEC, refining and fuels",
            &[
                "oil",
                "crude",
                "Brent",
                "WTI",
                "OPEC",
                "OPEC+",
                "refiner*",
                "refinery",
                "petroleum",
                "gasoline",
                "diesel",
                "jet fuel",
                "gas prices",
                "shale",
            ],
        ),
        topic(
            "UTILITIES",
            "Utilities and power producers, the MISO footprint first",
            &[
                "utilit*",
                "Xcel",
                "Ameren",
                "Entergy",
                "Alliant",
                "WEC Energy",
                "DTE",
                "Consumers Energy",
                "CMS Energy",
                "NiSource",
                "CenterPoint",
                "Otter Tail",
                "Duke Energy",
                "Southern Company",
                "Dominion Energy",
                "NextEra",
                "Exelon",
                "AEP",
                "American Electric Power",
                "PG&E",
                "Vistra",
                "Constellation Energy",
                "NRG",
                "Talen",
                "Calpine",
            ],
        ),
        topic(
            "POLICY",
            "Energy policy, regulators and rules",
            &[
                "FERC",
                "NERC",
                "EPA",
                "DOE",
                "Energy Department",
                "Department of Energy",
                "Energy Secretary",
                "Interior Department",
                "Inflation Reduction Act",
                "tax credit*",
                "permitting",
                "emissions rule*",
                "power plant rule*",
                "energy policy",
                "energy bill",
            ],
        ),
        topic(
            "CLIMATE",
            "Climate, emissions and the weather that moves load",
            &[
                "climate",
                "emissions",
                "carbon",
                "net zero",
                "net-zero",
                "decarboni*",
                "greenhouse",
                "heatwave*",
                "heat wave*",
                "hurricane*",
                "wildfire*",
                "drought",
                "extreme weather",
                "polar vortex",
                "winter storm*",
                "cold snap*",
            ],
        ),
        topic(
            "MACRO",
            "Rates, inflation and the economy",
            &[
                "Federal Reserve",
                "interest rate*",
                "rate cut*",
                "rate hike*",
                "inflation",
                "CPI",
                "jobs report",
                "payrolls",
                "unemployment",
                "GDP",
                "recession",
                "Treasury",
                "Treasuries",
                "bond yield*",
                "tariff*",
            ],
        ),
    ]
}

/// The `[news]` table in `config.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsConfig {
    /// Days of headlines kept on disk, so NEWS can search back across
    /// restarts. 0 keeps them all.
    pub keep_days: u32,
    /// Feeds to leave out, by id (`bloomberg-politics`).
    pub disabled_feeds: Vec<String>,
    /// More feeds, as `[[news.feeds]]`. One with a built-in's id replaces it.
    pub feeds: Vec<FeedSpec>,
    /// More topics for NI, as `[[news.topics]]`. One with a built-in's name
    /// replaces it.
    pub topics: Vec<Topic>,
}

impl Default for NewsConfig {
    fn default() -> Self {
        Self {
            keep_days: 21,
            disabled_feeds: Vec::new(),
            feeds: Vec::new(),
            topics: Vec::new(),
        }
    }
}

impl NewsConfig {
    /// Every feed, on or off: the built-ins (with any replaced by config),
    /// then the ones config adds.
    pub fn all_feeds(&self) -> Vec<FeedSpec> {
        let mut out: Vec<FeedSpec> = builtin_feeds()
            .into_iter()
            .map(|b| {
                self.feeds
                    .iter()
                    .find(|f| f.id.eq_ignore_ascii_case(&b.id))
                    .cloned()
                    .unwrap_or(b)
            })
            .collect();
        for f in &self.feeds {
            if !out.iter().any(|o| o.id.eq_ignore_ascii_case(&f.id)) && !f.url.trim().is_empty() {
                out.push(f.clone());
            }
        }
        out
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        !self
            .disabled_feeds
            .iter()
            .any(|d| d.eq_ignore_ascii_case(id))
    }

    /// The feeds to read.
    pub fn feeds(&self) -> Vec<FeedSpec> {
        self.all_feeds()
            .into_iter()
            .filter(|f| self.is_enabled(&f.id))
            .collect()
    }

    /// Every topic: the built-ins (with any replaced by config), then the
    /// ones config adds.
    pub fn topics(&self) -> Vec<Topic> {
        let mut out: Vec<Topic> = builtin_topics()
            .into_iter()
            .map(|b| {
                self.topics
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(&b.name))
                    .cloned()
                    .unwrap_or(b)
            })
            .collect();
        for t in &self.topics {
            if !out.iter().any(|o| o.name.eq_ignore_ascii_case(&t.name)) {
                out.push(t.clone());
            }
        }
        for t in &mut out {
            t.name = t.name.to_uppercase();
        }
        out
    }

    pub fn topic(&self, name: &str) -> Option<Topic> {
        self.topics()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_feeds_are_unique_https_and_named() {
        let feeds = builtin_feeds();
        for (i, f) in feeds.iter().enumerate() {
            assert!(f.url.starts_with("https://"), "{}", f.id);
            assert!(!f.section.is_empty() && !f.source.is_empty(), "{}", f.id);
            assert!(
                feeds[..i].iter().all(|g| g.id != f.id && g.url != f.url),
                "{} is listed twice",
                f.id
            );
        }
        for s in SOURCES {
            assert!(
                feeds.iter().any(|f| f.source == s.name && f.top),
                "{}",
                s.name
            );
        }
    }

    #[test]
    fn config_disables_replaces_and_adds() {
        let cfg: NewsConfig = toml::from_str(
            r#"
disabled_feeds = ["BLOOMBERG-POLITICS"]
[[feeds]]
id = "ft-energy"
source = "Financial Times"
section = "Energy (UK)"
url = "https://www.ft.com/energy?format=rss&edition=uk"
[[feeds]]
id = "enn"
source = "Energy News Network"
url = "https://energynews.us/feed/"
[[topics]]
name = "miso"
keywords = ["MISO", "Midcontinent"]
[[topics]]
name = "Gas"
keywords = ["LNG"]
"#,
        )
        .unwrap();
        assert_eq!(cfg.keep_days, 21, "defaults fill the rest");
        let all = cfg.all_feeds();
        assert_eq!(all.len(), builtin_feeds().len() + 1);
        let feeds = cfg.feeds();
        assert!(!feeds.iter().any(|f| f.id == "bloomberg-politics"));
        assert_eq!(
            feeds.iter().find(|f| f.id == "ft-energy").unwrap().section,
            "Energy (UK)"
        );
        assert_eq!(feeds.last().unwrap().id, "enn");
        assert_eq!(cfg.topic("gas").unwrap().keywords, vec!["LNG"]);
        assert_eq!(cfg.topic("MISO").unwrap().name, "MISO");
        assert_eq!(cfg.topics().len(), builtin_topics().len() + 1);
    }

    #[test]
    fn sources_by_alias_code_or_name() {
        let names = [FT, BLOOMBERG, WAPO, "Energy News Network"];
        assert_eq!(source_named("ft", names), Some(FT));
        assert_eq!(source_named("BBG", names), Some(BLOOMBERG));
        assert_eq!(source_named("wapo", names), Some(WAPO));
        assert_eq!(source_named("WashingtonPost", names), Some(WAPO));
        assert_eq!(source_named("ENN", names), Some("Energy News Network"));
        assert_eq!(source_named("gas", names), None);
        assert_eq!(short_name("Energy News Network"), "ENN");
        assert_eq!(short_name(WAPO), "WP");
    }

    #[test]
    fn topic_keywords_parse() {
        for t in builtin_topics() {
            assert_eq!(t.name, t.name.to_uppercase());
            assert!(!t.matcher().is_empty(), "{}", t.name);
            assert_eq!(
                t.matcher(),
                mt_core::news::Matcher::new(t.keywords.iter().map(String::as_str))
            );
        }
        let grid = builtin_topics()
            .into_iter()
            .find(|t| t.name == "GRID")
            .unwrap()
            .matcher();
        assert!(grid.matches_text("MISO warns of tight reserves"));
        assert!(!grid.matches_text("Miso soup recipes"));
    }
}
