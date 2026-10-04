//! News as a data source: headlines from publishers' public RSS and Atom
//! feeds (the Financial Times, Bloomberg and the Washington Post by default).
//! Headlines and summaries only, always attributed and linked: an article
//! opens in the reader's browser, where they are signed in, and its text is
//! never fetched or stored.
//!
//! Each feed is its own [`FeedQuery`], so LOG shows every feed's health. A
//! feed builds on what it already has: new headlines merge in by identity,
//! stay for [`NewsConfig::keep_days`], and are saved in the disk cache
//! (`local://news/<feed id>`), so NEWS searches back across restarts. Like
//! `mt-nws`, this crate depends only on `mt-core` and `mt-data`.

pub mod config;
pub mod parse;
mod text;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use mt_core::news::{Headline, canonical_link};
use mt_data::{Budget, DiskCache, FetchCtx, FetchError, Freshness, Query, host_of};
use serde::{Deserialize, Serialize};

pub use config::{
    FeedSpec, NewsConfig, SOURCES, Source, builtin_feeds, builtin_topics, short_name, source_named,
};

/// How often a feed is checked, unless it asks for longer (its RSS `ttl`).
pub const REFRESH: Duration = Duration::from_secs(5 * 60);
/// The longest a feed's `ttl` may stretch its refresh.
pub const MAX_TTL: Duration = Duration::from_secs(60 * 60);
/// Headlines kept per feed, whatever their age.
pub const MAX_PER_FEED: usize = 2_000;

/// One feed's headlines, newest first, including those kept from earlier fetches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FeedHeadlines {
    pub items: Vec<Headline>,
    /// How long the feed asks to be cached, in minutes (RSS `ttl`).
    #[serde(default)]
    pub ttl_minutes: Option<u32>,
    /// When the publisher last answered.
    #[serde(default)]
    pub fetched: Option<DateTime<Utc>>,
    /// Headlines in that answer (0: the feed answered, but empty).
    #[serde(default)]
    pub latest: usize,
}

impl FeedHeadlines {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

/// A feed's headlines, refreshed every [`REFRESH`] (or the feed's `ttl`).
#[derive(Clone, Debug)]
pub struct FeedQuery {
    feed: FeedSpec,
    keep_days: u32,
}

impl FeedQuery {
    pub fn new(feed: FeedSpec, keep_days: u32) -> Self {
        Self { feed, keep_days }
    }

    pub fn feed(&self) -> &FeedSpec {
        &self.feed
    }
}

impl Query for FeedQuery {
    type Output = FeedHeadlines;

    fn key(&self) -> String {
        format!("news/{}", self.feed.id.to_ascii_lowercase())
    }

    fn label(&self) -> String {
        if self.feed.section.is_empty() {
            format!("{} headlines", self.feed.source)
        } else {
            format!("{} · {} headlines", self.feed.source, self.feed.section)
        }
    }

    fn freshness(&self, current: &FeedHeadlines) -> Freshness {
        let ttl = Duration::from_secs(u64::from(current.ttl_minutes.unwrap_or(0)) * 60);
        Freshness::Every(ttl.clamp(REFRESH, MAX_TTL))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<FeedHeadlines>>,
    ) -> Result<FeedHeadlines, FetchError> {
        // The first fetch of a session builds on the archive.
        let prev = match prev {
            Some(p) => Some(p),
            None => ctx
                .local_get(&archive_key(&self.feed.id))
                .await
                .and_then(|b| FeedHeadlines::from_bytes(&b))
                .map(Arc::new),
        };
        let body = ctx.get(&self.feed.url).await?;
        let parsed = parse::parse_feed(&body).map_err(|e| match e {
            FetchError::Parse { detail, .. } => FetchError::parse(&self.feed.url, detail),
            e => e,
        })?;
        let now = mt_core::time::now_utc();
        let fresh: Vec<Headline> = parsed
            .items
            .into_iter()
            .map(|it| headline(&self.feed, it, now))
            .collect();
        let latest = fresh.len();
        let old = prev.as_deref().map_or(&[][..], |p| p.items.as_slice());
        let out = FeedHeadlines {
            items: merge(old, fresh, self.keep_days, now),
            ttl_minutes: parsed.ttl_minutes,
            fetched: Some(now),
            latest,
        };
        if prev.as_ref().is_none_or(|p| p.items != out.items) {
            ctx.local_put(&archive_key(&self.feed.id), out.to_bytes())
                .await;
        }
        Ok(out)
    }
}

/// A parsed item as a headline from `feed`, first seen `now`.
pub fn headline(feed: &FeedSpec, it: parse::Item, now: DateTime<Utc>) -> Headline {
    let id = identity(&it);
    let link = match (&it.guid, it.guid_is_link) {
        (Some(g), true) if it.link.is_empty() => g.clone(),
        _ => it.link,
    };
    Headline {
        id,
        source: feed.source.clone(),
        title: if it.title.is_empty() {
            link.clone()
        } else {
            it.title
        },
        summary: it.summary,
        link,
        author: it.author,
        published: it.published,
        seen: now,
        sections: if feed.section.is_empty() {
            Vec::new()
        } else {
            vec![feed.section.clone()]
        },
    }
}

/// A story's identity: the publisher's own id, scoped to its site (the same
/// FT story has one id in every FT feed), or else its link without tracking
/// parameters.
pub fn identity(it: &parse::Item) -> String {
    match (&it.guid, it.guid_is_link) {
        (Some(g), false) => {
            let host = host_of(&it.link);
            format!("{}:{g}", if host.is_empty() { "feed" } else { host })
        }
        (Some(g), true) if g.contains("://") => canonical_link(g),
        _ if !it.link.is_empty() => canonical_link(&it.link),
        _ => format!("title:{}", it.title),
    }
}

/// A feed's latest answer merged into what it had: one entry per story
/// (keeping when it was first seen and every section it appeared in),
/// newest first, without stories older than `keep_days` (0 keeps all).
pub fn merge(
    old: &[Headline],
    fresh: Vec<Headline>,
    keep_days: u32,
    now: DateTime<Utc>,
) -> Vec<Headline> {
    let old_by_id: HashMap<&str, &Headline> = old.iter().map(|h| (h.id.as_str(), h)).collect();
    let mut ids = HashSet::new();
    let mut out = Vec::with_capacity(old.len() + fresh.len());
    for mut h in fresh {
        if !ids.insert(h.id.clone()) {
            continue;
        }
        if let Some(o) = old_by_id.get(h.id.as_str()) {
            h.seen = o.seen.min(h.seen);
            h.published = h.published.or(o.published);
            for s in &o.sections {
                if !h.sections.contains(s) {
                    h.sections.push(s.clone());
                }
            }
        }
        out.push(h);
    }
    out.extend(old.iter().filter(|o| ids.insert(o.id.clone())).cloned());
    if keep_days > 0 {
        let cutoff = now - TimeDelta::days(i64::from(keep_days));
        out.retain(|h| h.time() >= cutoff);
    }
    out.sort_by_key(|h| std::cmp::Reverse(h.time()));
    out.truncate(MAX_PER_FEED);
    out
}

/// Several feeds' headlines as one list, newest first. A story in two feeds
/// (the same id, or the same link without tracking parameters) appears once,
/// with both sections.
pub fn combine<'a>(feeds: impl IntoIterator<Item = &'a [Headline]>) -> Vec<Headline> {
    let mut out: Vec<Headline> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for items in feeds {
        for h in items {
            let link = (!h.link.is_empty()).then(|| canonical_link(&h.link));
            let hit = index
                .get(&h.id)
                .or_else(|| link.as_ref().and_then(|l| index.get(l)))
                .copied();
            match hit {
                Some(i) => {
                    let o = &mut out[i];
                    for s in &h.sections {
                        if !o.sections.contains(s) {
                            o.sections.push(s.clone());
                        }
                    }
                    o.seen = o.seen.min(h.seen);
                    if o.summary.is_empty() {
                        o.summary.clone_from(&h.summary);
                    }
                }
                None => {
                    index.insert(h.id.clone(), out.len());
                    if let Some(l) = link {
                        index.insert(l, out.len());
                    }
                    out.push(h.clone());
                }
            }
        }
    }
    out.sort_by_key(|h| std::cmp::Reverse(h.time()));
    out
}

/// Pacing publishers ask for: the FT's robots.txt sets a one-second crawl delay.
pub fn budgets() -> Vec<Budget> {
    vec![Budget::new(
        "Financial Times (1 s crawl delay)",
        ["www.ft.com"],
        1,
        Duration::from_secs(1),
    )]
}

/// Where a feed's headlines are kept between sessions.
pub fn archive_key(feed_id: &str) -> String {
    format!("local://news/{}", feed_id.to_ascii_lowercase())
}

/// Where the ids of read headlines are kept.
pub const READ_KEY: &str = "local://news-read/ids";

/// The directories holding the archive and read marks inside `cache`
/// (exempt from the cache's size cap; they keep `keep_days` instead).
pub fn archive_dirs(cache: &DiskCache) -> Vec<PathBuf> {
    [archive_key("x"), READ_KEY.to_owned()]
        .iter()
        .filter_map(|k| cache.path_for(k).parent().map(Path::to_path_buf))
        .collect()
}

/// A feed's archived headlines, read synchronously (for showing them at
/// launch, before the first fetch).
pub fn load_archive(cache: &DiskCache, feed_id: &str) -> Option<FeedHeadlines> {
    FeedHeadlines::from_bytes(&cache.get(&archive_key(feed_id))?)
}

/// Delete archived feeds nobody has fetched for `keep_days` (feeds since
/// removed from the list). Returns how many were removed.
pub fn prune_archives(cache: &DiskCache, keep_days: u32) -> usize {
    if keep_days == 0 {
        return 0;
    }
    let Some(dir) = cache
        .path_for(&archive_key("x"))
        .parent()
        .map(Path::to_path_buf)
    else {
        return 0;
    };
    let max_age = Duration::from_secs(u64::from(keep_days) * 86_400);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > max_age)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Which headlines have been opened, by id, kept as long as headlines are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadMarks {
    ids: HashMap<String, DateTime<Utc>>,
}

impl ReadMarks {
    pub fn is_read(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Mark or unmark; returns whether anything changed.
    pub fn set(&mut self, id: &str, read: bool, now: DateTime<Utc>) -> bool {
        if read {
            self.ids.insert(id.to_owned(), now).is_none()
        } else {
            self.ids.remove(id).is_some()
        }
    }

    /// Forget marks older than `keep_days` (their headlines are gone too).
    pub fn prune(&mut self, keep_days: u32, now: DateTime<Utc>) {
        if keep_days > 0 {
            let cutoff = now - TimeDelta::days(i64::from(keep_days) + 1);
            self.ids.retain(|_, at| *at >= cutoff);
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_791_100_000 + h * 3600, 0).unwrap()
    }

    fn h(id: &str, hour: i64, section: &str) -> Headline {
        Headline {
            id: id.into(),
            source: "Test".into(),
            title: id.into(),
            summary: String::new(),
            link: format!("https://example.com/{id}?utm_source=rss"),
            author: None,
            published: Some(at(hour)),
            seen: at(hour),
            sections: vec![section.into()],
        }
    }

    #[test]
    fn merging_keeps_first_seen_sections_and_the_window() {
        let mut old_a = h("a", 0, "Markets");
        old_a.seen = at(1);
        let old = vec![
            old_a,
            h("ancient", -24 * 30, "Markets"),
            h("b", -5, "Markets"),
        ];
        let mut fresh_a = h("a", 0, "Energy");
        fresh_a.seen = at(10);
        let fresh = vec![h("c", 2, "Energy"), fresh_a, h("c", 2, "Energy")];
        let out = merge(&old, fresh, 21, at(12));
        let ids: Vec<&str> = out.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            ["c", "a", "b"],
            "newest first, no duplicates, no 30-day-old story"
        );
        assert_eq!(out[1].seen, at(1), "first seen survives");
        assert_eq!(out[1].sections, ["Energy", "Markets"]);
        assert_eq!(
            merge(&old, Vec::new(), 0, at(12)).len(),
            3,
            "0 keeps everything"
        );
    }

    #[test]
    fn combining_feeds_dedupes_by_id_and_by_link() {
        let markets = vec![h("x", 3, "Markets"), h("y", 1, "Markets")];
        let mut energy_x = h("x", 3, "Energy");
        energy_x.summary = "standfirst".into();
        let mut other_id_same_link = h("z", 2, "Energy");
        other_id_same_link.link = "https://EXAMPLE.com/y#top".into();
        let energy = vec![energy_x, other_id_same_link];
        let all = combine([markets.as_slice(), energy.as_slice()]);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].sections, ["Markets", "Energy"]);
        assert_eq!(all[0].summary, "standfirst");
        assert_eq!(all[1].sections, ["Markets", "Energy"]);
    }

    #[test]
    fn identities() {
        let mut it = parse::Item {
            title: "T".into(),
            link: "https://www.ft.com/content/abc?syn-25a6b1a6=1".into(),
            guid: Some("abc".into()),
            guid_is_link: false,
            ..parse::Item::default()
        };
        assert_eq!(identity(&it), "www.ft.com:abc");
        it.guid_is_link = true;
        it.guid = Some("https://www.ft.com/content/abc?utm_medium=rss".into());
        assert_eq!(identity(&it), "https://www.ft.com/content/abc");
        it.guid = None;
        assert_eq!(identity(&it), "https://www.ft.com/content/abc");
        it.link.clear();
        assert_eq!(identity(&it), "title:T");
    }

    #[test]
    fn read_marks_round_trip_and_expire() {
        let mut r = ReadMarks::default();
        assert!(r.set("a", true, at(0)));
        assert!(!r.set("a", true, at(1)), "already read");
        assert!(r.set("b", true, at(24 * 40)));
        let back = ReadMarks::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back, r);
        r.prune(21, at(24 * 40));
        assert!(!r.is_read("a") && r.is_read("b"));
        assert!(r.set("b", false, at(0)) && r.is_empty());
    }

    #[test]
    fn ttl_stretches_the_refresh_within_limits() {
        let q = FeedQuery::new(builtin_feeds().remove(0), 21);
        let with = |ttl| FeedHeadlines {
            ttl_minutes: ttl,
            ..FeedHeadlines::default()
        };
        assert_eq!(q.freshness(&with(None)), Freshness::Every(REFRESH));
        assert_eq!(q.freshness(&with(Some(1))), Freshness::Every(REFRESH));
        assert_eq!(
            q.freshness(&with(Some(15))),
            Freshness::Every(Duration::from_secs(900))
        );
        assert_eq!(q.freshness(&with(Some(1440))), Freshness::Every(MAX_TTL));
    }
}
