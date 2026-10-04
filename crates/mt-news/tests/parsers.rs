//! Every built-in news feed against its recording (`MT_FIXTURES` to use
//! another set, as the weekly drift job does with live ones).

use std::collections::HashSet;
use std::path::PathBuf;

use mt_data::FixtureTransport;
use mt_news::parse::parse_feed;
use mt_news::{SOURCES, builtin_feeds, headline};

fn root() -> PathBuf {
    std::env::var_os("MT_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

#[test]
fn every_builtin_feed_parses() {
    let fixtures = FixtureTransport::new(root());
    let now = chrono::Utc::now();
    let mut per_source: Vec<(&str, usize)> = SOURCES.iter().map(|s| (s.name, 0)).collect();
    for feed in builtin_feeds() {
        let path = fixtures.path_for(&feed.url);
        let body = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let parsed = parse_feed(&body).unwrap_or_else(|e| panic!("{}: {e}", feed.id));
        let mut ids = HashSet::new();
        for item in parsed.items {
            assert!(
                !item.title.is_empty(),
                "{}: an item without a title",
                feed.id
            );
            assert!(
                item.link.starts_with("https://") || item.link.starts_with("http://"),
                "{}: {:?} links to {:?}",
                feed.id,
                item.title,
                item.link
            );
            assert!(
                item.published.is_some(),
                "{}: {:?} has no date",
                feed.id,
                item.title
            );
            assert!(
                !item.title.contains('<') || !item.title.contains('>'),
                "{}: markup in {:?}",
                feed.id,
                item.title
            );
            let h = headline(&feed, item, now);
            assert_eq!(h.source, feed.source);
            assert!(ids.insert(h.id.clone()), "{}: {} twice", feed.id, h.id);
            if let Some(n) = per_source.iter_mut().find(|(s, _)| *s == feed.source) {
                n.1 += 1;
            }
        }
    }
    // A publisher whose every feed is empty has most likely moved them.
    for (source, n) in per_source {
        assert!(n > 0, "no headlines at all from {source}");
    }
}
