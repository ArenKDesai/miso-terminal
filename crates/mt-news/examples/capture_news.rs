//! Record every built-in news feed (offline mode, parser tests, the weekly
//! drift job). One request per feed, FT's paced a second apart.
//!
//!     cargo run -p mt-news --example capture_news [--verbatim] [DIR]
//!
//! Headlines and summaries are the publishers' work, so by default each
//! story's words (title, summary, byline, the words in its URL) are replaced
//! with sample text, keeping the feed's structure: element order, CDATA,
//! namespaces, ids and dates. That is what goes in the repository's
//! `fixtures/`. `--verbatim` keeps the real text, for the drift job, which
//! parses a live recording and throws it away.

use std::path::PathBuf;
use std::sync::Arc;

use mt_data::{EventLog, FetchCtx, FetchCtxOptions, FixtureTransport, HttpTransport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let verbatim = args.iter().any(|a| a == "--verbatim");
    args.retain(|a| a != "--verbatim");
    let out = PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures").to_owned()),
    );
    let ctx = FetchCtx::new(
        Arc::new(HttpTransport::new(
            "MISO-Terminal fixture recorder (github.com/ArenKDesai/miso-terminal)",
        )?),
        None,
        FetchCtxOptions {
            budgets: mt_news::budgets(),
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let fixtures = FixtureTransport::new(&out);
    let mut failed = 0;
    for feed in mt_news::builtin_feeds() {
        match ctx.get(&feed.url).await {
            Ok(body) => {
                let body = if verbatim {
                    body.to_vec()
                } else {
                    sample_copy(&String::from_utf8_lossy(&body)).into_bytes()
                };
                let path = fixtures.path_for(&feed.url);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &body)?;
                println!("{:>6} KB  {}", body.len() / 1024, path.display());
            }
            Err(e) => {
                failed += 1;
                eprintln!("FAILED {}: {e}", feed.url);
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} feed(s) failed").into());
    }
    Ok(())
}

const SUBJECTS: &[&str] = &[
    "Power prices",
    "Natural gas futures",
    "MISO",
    "PJM",
    "Utilities",
    "Oil",
    "The Federal Reserve",
    "Data centers",
    "Wind and solar output",
    "Treasury yields",
    "Chipmakers",
    "Lawmakers",
    "FERC",
    "Refiners",
    "Stocks",
    "Shipping rates",
];

const PREDICATES: &[&str] = &[
    "rise as a cold snap nears",
    "slip on mild forecasts",
    "draw scrutiny from regulators",
    "hold steady ahead of an auction",
    "climb on record demand",
    "fall as supply returns",
    "weigh new transmission lines",
    "face a busy week",
    "signal a pause",
    "beat expectations",
];

/// A stable number for a piece of text, so a story in two feeds gets the
/// same sample words in both (and still de-duplicates).
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The feed with every story's words replaced by sample text.
fn sample_copy(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    loop {
        let start = ["<item>", "<item ", "<entry>", "<entry "]
            .iter()
            .filter_map(|t| rest.find(t))
            .min();
        let Some(start) = start else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let close = if rest[start..].starts_with("<entry") {
            "</entry>"
        } else {
            "</item>"
        };
        let end = rest[start..]
            .find(close)
            .map_or(rest.len(), |e| start + e + close.len());
        out.push_str(&sample_item(&rest[start..end]));
        rest = &rest[end..];
    }
    out
}

fn sample_item(item: &str) -> String {
    let mut item = item.to_owned();
    for tag in ["title", "description", "summary"] {
        item = replace_inner(&item, tag, |old| {
            let h = hash(old);
            let subject = SUBJECTS[(h % SUBJECTS.len() as u64) as usize];
            let predicate = PREDICATES[(h / 97 % PREDICATES.len() as u64) as usize];
            if tag == "title" {
                format!("Sample: {subject} {predicate}")
            } else {
                format!(
                    "Sample summary for a test fixture: {} {predicate}. The publisher's text is not kept here.",
                    subject.to_lowercase()
                )
            }
        });
    }
    for tag in ["dc:creator", "author"] {
        item = replace_inner(&item, tag, |_| "Sample Reporter".to_owned());
    }
    for tag in ["link", "guid", "id"] {
        item = replace_inner(&item, tag, sample_url);
    }
    item
}

/// URL path segments made of words (`/schneider-said-to-near-deal/`) become
/// `sample-story-<n>`; ids, dates and hex segments stay.
fn sample_url(url: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let path: Vec<String> = base
        .split('/')
        .map(|seg| {
            let wordy = seg.contains('-')
                && seg
                    .chars()
                    .any(|c| c.is_ascii_alphabetic() && !c.is_ascii_hexdigit());
            if wordy {
                format!("sample-story-{}", hash(seg) % 100_000)
            } else {
                seg.to_owned()
            }
        })
        .collect();
    let path = path.join("/");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// Replace the text inside each `<tag …>…</tag>` with `f(old text)`,
/// keeping a CDATA wrapper if there was one.
fn replace_inner(xml: &str, tag: &str, f: impl Fn(&str) -> String) -> String {
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    loop {
        let open = [format!("<{tag}>"), format!("<{tag} ")]
            .iter()
            .filter_map(|o| rest.find(o.as_str()))
            .min();
        let Some(open) = open else {
            out.push_str(rest);
            return out;
        };
        let Some(gt) = rest[open..].find('>').map(|g| open + g + 1) else {
            out.push_str(rest);
            return out;
        };
        // `<link href="…"/>` (Atom) has no text.
        if rest[..gt].ends_with("/>") {
            out.push_str(&rest[..gt]);
            rest = &rest[gt..];
            continue;
        }
        let Some(end) = rest[gt..].find(&close).map(|e| gt + e) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..gt]);
        let inner = &rest[gt..end];
        match inner
            .strip_prefix("<![CDATA[")
            .and_then(|s| s.strip_suffix("]]>"))
        {
            Some(text) => {
                out.push_str("<![CDATA[");
                out.push_str(&f(text));
                out.push_str("]]>");
            }
            None if inner.trim().is_empty() => out.push_str(inner),
            None => out.push_str(&f(inner)),
        }
        rest = &rest[end..];
    }
}
