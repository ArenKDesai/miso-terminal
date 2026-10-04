//! RSS 2.0, RSS 1.0 (RDF) and Atom into items. Lenient where feeds are
//! sloppy (HTML in fields, unknown entities, missing dates), and it never
//! keeps article bodies (`content:encoded`, Atom `content`): headlines and
//! summaries only.

use chrono::{DateTime, Utc};
use mt_data::FetchError;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::text;

/// A feed as published, before it becomes headlines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedFeed {
    /// The channel's title.
    pub title: String,
    /// How long the channel asks to be cached, in minutes (RSS `ttl`).
    pub ttl_minutes: Option<u32>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub title: String,
    pub summary: String,
    pub link: String,
    /// The feed's id for the story (RSS `guid`, Atom `id`).
    pub guid: Option<String>,
    /// Whether `guid` is the article's URL (RSS `isPermaLink`, true unless it says not).
    pub guid_is_link: bool,
    pub author: Option<String>,
    /// When it was published (Atom's `updated` when nothing else says).
    pub published: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Title,
    Summary,
    Link,
    Guid,
    Author,
    Published,
    Updated,
    ChannelTitle,
    Ttl,
}

/// Parse a feed. Anything that is not RSS or Atom (an error page, a login
/// wall) is an error rather than an empty feed.
pub fn parse_feed(body: &[u8]) -> Result<ParsedFeed, FetchError> {
    const WHAT: &str = "news feed";
    let source = String::from_utf8_lossy(body);
    let mut reader = Reader::from_str(source.trim_start_matches('\u{feff}'));
    reader.config_mut().check_end_names = false;

    let mut feed = ParsedFeed::default();
    let mut stack: Vec<String> = Vec::new();
    let mut item: Option<(Item, Option<DateTime<Utc>>)> = None;
    let mut item_depth = 0;
    let mut capture: Option<(Field, usize)> = None;
    let mut buf = String::new();
    let mut root_seen = false;
    loop {
        let event = reader.read_event().map_err(|e| {
            FetchError::parse(WHAT, format!("{e} at byte {}", reader.buffer_position()))
        })?;
        match event {
            Event::Start(e) => {
                let name = local_name(&e);
                if !root_seen && !matches!(name.as_str(), "rss" | "feed" | "rdf") {
                    return Err(FetchError::parse(
                        WHAT,
                        format!("not an RSS or Atom feed (starts with <{name}>)"),
                    ));
                }
                root_seen = true;
                let parent = stack.last().cloned().unwrap_or_default();
                stack.push(name.clone());
                let depth = stack.len();
                match capture {
                    // Atom's <author><name>: keep just the name.
                    Some((Field::Author, _)) if name == "name" => {
                        buf.clear();
                        capture = Some((Field::Author, depth));
                    }
                    // Markup inside a field (HTML in a summary): its text still counts.
                    Some(_) => {}
                    None if matches!(name.as_str(), "item" | "entry") => {
                        item = Some((
                            Item {
                                guid_is_link: name == "item",
                                ..Item::default()
                            },
                            None,
                        ));
                        item_depth = depth;
                    }
                    None => {
                        let field = match &mut item {
                            Some((it, _)) if depth == item_depth + 1 => {
                                if name == "link" {
                                    atom_link(it, &e);
                                }
                                if name == "guid" {
                                    it.guid_is_link = attr(&e, "isPermaLink")
                                        .is_none_or(|v| !v.trim().eq_ignore_ascii_case("false"));
                                }
                                item_field(&name)
                            }
                            Some(_) => None,
                            None if matches!(parent.as_str(), "channel" | "feed") => {
                                match name.as_str() {
                                    "title" => Some(Field::ChannelTitle),
                                    "ttl" => Some(Field::Ttl),
                                    _ => None,
                                }
                            }
                            None => None,
                        };
                        if let Some(field) = field {
                            buf.clear();
                            capture = Some((field, depth));
                        }
                    }
                }
            }
            Event::Empty(e) => {
                if let Some((it, _)) = &mut item
                    && capture.is_none()
                    && stack.len() == item_depth
                    && local_name(&e) == "link"
                {
                    atom_link(it, &e);
                }
            }
            Event::End(_) => {
                let depth = stack.len();
                if let Some((field, at)) = capture
                    && at == depth
                {
                    capture = None;
                    let value = std::mem::take(&mut buf);
                    apply(&mut feed, item.as_mut(), field, &value);
                }
                if depth == item_depth
                    && let Some((mut it, updated)) = item.take()
                {
                    it.published = it.published.or(updated);
                    if !(it.title.is_empty() && it.link.is_empty()) {
                        feed.items.push(it);
                    }
                }
                stack.pop();
            }
            Event::Text(t) if capture.is_some() => {
                buf.push_str(&t.decode().map_err(|e| FetchError::parse(WHAT, e))?);
            }
            Event::CData(c) if capture.is_some() => {
                buf.push_str(&c.decode().map_err(|e| FetchError::parse(WHAT, e))?);
            }
            Event::GeneralRef(r) if capture.is_some() => {
                let name = r.decode().map_err(|e| FetchError::parse(WHAT, e))?;
                match (r.resolve_char_ref(), name.as_ref()) {
                    (Ok(Some(c)), _) => buf.push(c),
                    // XML's own entities now, so escaped HTML (`&lt;p&gt;`) is
                    // markup by the time tags are stripped.
                    (_, "amp") => buf.push('&'),
                    (_, "lt") => buf.push('<'),
                    (_, "gt") => buf.push('>'),
                    (_, "quot") => buf.push('"'),
                    (_, "apos") => buf.push('\''),
                    // HTML's (`&nbsp;`) are decoded with the text.
                    _ => {
                        buf.push('&');
                        buf.push_str(&name);
                        buf.push(';');
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen {
        return Err(FetchError::parse(
            WHAT,
            "not an RSS or Atom feed (no elements)",
        ));
    }
    Ok(feed)
}

fn item_field(name: &str) -> Option<Field> {
    Some(match name {
        "title" => Field::Title,
        // Never `content` / `content:encoded`: those are the article.
        "description" | "summary" => Field::Summary,
        "link" => Field::Link,
        "guid" | "id" => Field::Guid,
        "pubdate" | "published" | "date" | "issued" => Field::Published,
        "updated" | "modified" => Field::Updated,
        "creator" | "author" => Field::Author,
        _ => return None,
    })
}

fn apply(
    feed: &mut ParsedFeed,
    item: Option<&mut (Item, Option<DateTime<Utc>>)>,
    field: Field,
    value: &str,
) {
    match (field, item) {
        (Field::ChannelTitle, _) if feed.title.is_empty() => feed.title = text::plain(value),
        (Field::Ttl, _) => feed.ttl_minutes = value.trim().parse().ok(),
        (Field::Title, Some((it, _))) => it.title = text::plain(value),
        (Field::Summary, Some((it, _))) => {
            let summary = text::plain(value);
            if !summary.is_empty() {
                it.summary = summary;
            }
        }
        (Field::Link, Some((it, _))) => {
            let link = text::decode_entities(value.trim());
            if !link.is_empty() {
                it.link = link;
            }
        }
        (Field::Guid, Some((it, _))) => {
            let guid = text::decode_entities(value.trim());
            it.guid = (!guid.is_empty()).then_some(guid);
        }
        (Field::Author, Some((it, _))) => it.author = author(value),
        (Field::Published, Some((it, _))) => {
            it.published = it.published.or_else(|| parse_date(value));
        }
        (Field::Updated, Some((_, updated))) => *updated = parse_date(value),
        _ => {}
    }
}

/// Atom links: the `alternate` one (or one without `rel`) is the article.
fn atom_link(it: &mut Item, e: &BytesStart<'_>) {
    let Some(href) = attr(e, "href") else { return };
    match attr(e, "rel").as_deref().map(str::trim) {
        Some("alternate") => it.link = href.trim().to_owned(),
        None if it.link.is_empty() => it.link = href.trim().to_owned(),
        _ => {}
    }
}

/// `jdoe@example.com (Jane Doe)` and `By Jane Doe` become `Jane Doe`.
fn author(value: &str) -> Option<String> {
    let s = text::plain(value);
    let s = match (s.find('('), s.rfind(')')) {
        (Some(a), Some(b)) if a < b && s[..a].contains('@') => s[a + 1..b].trim().to_owned(),
        _ => s,
    };
    let s = s.strip_prefix("By ").unwrap_or(&s).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// RFC 2822 (`Sun, 04 Oct 2026 21:11:58 GMT`, RSS) or RFC 3339 (Atom).
pub fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    DateTime::parse_from_rfc2822(s)
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn local_name(e: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase()
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| {
            a.key
                .local_name()
                .as_ref()
                .eq_ignore_ascii_case(name.as_bytes())
        })
        .map(|a| text::decode_entities(&String::from_utf8_lossy(&a.value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_with_cdata_html_and_entities() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:media="http://search.yahoo.com/mrss/">
  <channel>
    <title><![CDATA[Markets]]></title>
    <ttl>15</ttl>
    <image><title>Logo</title></image>
    <item>
      <title><![CDATA[Gas &amp; power: MISO&#8217;s week ]]></title>
      <description>&lt;p&gt;Prices &lt;b&gt;rose&lt;/b&gt;&amp;nbsp;3%&lt;/p&gt;</description>
      <content:encoded><![CDATA[<p>The whole article, never kept.</p>]]></content:encoded>
      <media:content><media:description>A photo caption</media:description></media:content>
      <link>https://example.com/a?id=1&amp;syn-x=1</link>
      <guid isPermaLink="false">ABC123</guid>
      <dc:creator>Jane Doe</dc:creator>
      <pubDate>Sun, 04 Oct 2026 21:11:58 GMT</pubDate>
    </item>
    <item>
      <title>Second</title>
      <link>https://example.com/b</link>
      <guid>https://example.com/b</guid>
      <author>news@example.com (John Roe)</author>
      <pubDate>Sat, 3 Oct 2026 09:00:00 -0400</pubDate>
    </item>
    <item><description>No title and no link: skipped</description></item>
  </channel>
</rss>"#;
        let f = parse_feed(xml).unwrap();
        assert_eq!(f.title, "Markets");
        assert_eq!(f.ttl_minutes, Some(15));
        assert_eq!(f.items.len(), 2);
        let a = &f.items[0];
        assert_eq!(a.title, "Gas & power: MISO’s week");
        assert_eq!(a.summary, "Prices rose 3%");
        assert_eq!(a.link, "https://example.com/a?id=1&syn-x=1");
        assert_eq!((a.guid.as_deref(), a.guid_is_link), (Some("ABC123"), false));
        assert_eq!(a.author.as_deref(), Some("Jane Doe"));
        assert_eq!(
            a.published.unwrap().to_rfc3339(),
            "2026-10-04T21:11:58+00:00"
        );
        let b = &f.items[1];
        assert!(b.guid_is_link);
        assert_eq!(b.author.as_deref(), Some("John Roe"));
        assert_eq!(
            b.published.unwrap().to_rfc3339(),
            "2026-10-03T13:00:00+00:00"
        );
    }

    #[test]
    fn atom() {
        let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom">
  <title type="text">Energy desk</title>
  <link rel="self" href="https://example.com/feed.atom"/>
  <entry>
    <title type="html">Grid &lt;em&gt;strain&lt;/em&gt;</title>
    <link rel="enclosure" href="https://example.com/a.mp3"/>
    <link rel="alternate" type="text/html" href="https://example.com/grid"/>
    <id>tag:example.com,2026:grid</id>
    <updated>2026-10-04T12:00:00Z</updated>
    <summary>Reserve margins tighten.</summary>
    <content type="html">The article body.</content>
    <author><name>A. Writer</name><email>a@example.com</email></author>
  </entry>
</feed>"#;
        let f = parse_feed(xml).unwrap();
        assert_eq!(f.title, "Energy desk");
        let e = &f.items[0];
        assert_eq!(e.title, "Grid strain");
        assert_eq!(e.link, "https://example.com/grid");
        assert_eq!(e.summary, "Reserve margins tighten.");
        assert_eq!(e.author.as_deref(), Some("A. Writer"));
        assert!(!e.guid_is_link);
        assert_eq!(
            e.published.unwrap().to_rfc3339(),
            "2026-10-04T12:00:00+00:00"
        );
    }

    #[test]
    fn rss_1_items_are_siblings_of_the_channel() {
        let xml = br#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel><title>RDF news</title></channel>
  <item><title>One</title><link>https://example.com/1</link><dc:date>2026-10-04T08:00:00-05:00</dc:date></item>
</rdf:RDF>"#;
        let f = parse_feed(xml).unwrap();
        assert_eq!(f.title, "RDF news");
        assert_eq!(
            f.items[0].published.unwrap().to_rfc3339(),
            "2026-10-04T13:00:00+00:00"
        );
    }

    #[test]
    fn error_pages_are_not_feeds() {
        assert!(parse_feed(b"<!DOCTYPE html><html><body>Sign in</body></html>").is_err());
        assert!(parse_feed(b"").is_err());
        assert_eq!(
            parse_feed(b"<rss version=\"2.0\"><channel><title>Empty</title></channel></rss>")
                .unwrap()
                .items
                .len(),
            0
        );
    }

    #[test]
    fn dates() {
        for s in [
            "Sun, 04 Oct 2026 21:11:58 GMT",
            "Sun, 04 Oct 2026 21:11:58 +0000",
            "Sun, 04 Oct 2026 17:11:58 EDT",
            "2026-10-04T21:11:58Z",
        ] {
            assert_eq!(
                parse_date(s).map(|d| d.timestamp()),
                parse_date("2026-10-04T21:11:58+00:00").map(|d| d.timestamp()),
                "{s}"
            );
        }
        assert_eq!(parse_date("yesterday"), None);
    }
}
