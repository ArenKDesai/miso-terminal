//! Record live MISO responses into `fixtures/` for parser tests and offline mode.
//!
//!     cargo run -p mt-miso --example capture_fixtures
//!
//! Large feeds (the rolling five-minute day, daily report CSVs) are trimmed to
//! a handful of nodes so the repository stays small. Re-run this whenever MISO
//! changes a format, then fix whichever parser test fails.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Duration;
use mt_core::time::market_today;
use mt_data::{EventLog, FetchCtx, FetchCtxOptions, FetchError, FixtureTransport, HttpTransport};
use mt_miso::MisoEndpoints;
use mt_miso::endpoints::{paths, reports};

/// Nodes kept in trimmed fixtures: the trading hubs, some interfaces and a few
/// load zones.
const FIXTURE_NODES: &[&str] = &[
    "ARKANSAS.HUB",
    "ILLINOIS.HUB",
    "INDIANA.HUB",
    "LOUISIANA.HUB",
    "MICHIGAN.HUB",
    "MINN.HUB",
    "MS.HUB",
    "TEXAS.HUB",
    "PJMC",
    "SWPP",
    "TVA",
    "ONT",
    "MHEB",
    "AECI",
    "LGEE",
    "SOCO",
    "ALTE.ALTE",
    "MGE.AZ",
    "WEC.AZ",
    "WPS.AZ",
    "CIN.PSI",
    "LEPA.LEPA",
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures").to_owned()),
    );
    let endpoints = MisoEndpoints::default();
    let ctx = FetchCtx::new(
        Arc::new(HttpTransport::new("MISO-Terminal fixture recorder")?),
        None,
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let fixtures = FixtureTransport::new(&out);
    let keep: HashSet<&str> = FIXTURE_NODES.iter().copied().collect();

    for path in paths::ALL {
        let url = endpoints.api(path);
        let body = ctx.get_text(&url).await?;
        let big = [paths::RT_FIVE_MIN_ROLLING, paths::RT_FIVE_MIN_PREVIOUS];
        let body = if big.contains(path) {
            trim_table(&body, &keep)?
        } else {
            body
        };
        write(&fixtures.path_for(&url), &body)?;
    }

    // Daily reports: DA for yesterday, today and (after ~1:30 pm EST) tomorrow;
    // the most recent RT prelim (yesterday's is not out until morning) and RT final.
    let today = market_today();
    let mut wanted = vec![
        (reports::DA_EXPOST, today - Duration::days(1)),
        (reports::DA_EXPOST, today),
        (reports::DA_EXPOST, today + Duration::days(1)),
    ];
    wanted.extend((1..4).map(|back| (reports::RT_PRELIM, today - Duration::days(back))));
    wanted.extend((2..15).map(|back| (reports::RT_FINAL, today - Duration::days(back))));
    let newest_only = [reports::RT_PRELIM, reports::RT_FINAL];
    let mut have = HashSet::new();
    for (suffix, day) in wanted {
        if have.contains(suffix) {
            continue;
        }
        let url = endpoints.report(day, suffix);
        match ctx.get_text(&url).await {
            Ok(body) => {
                write(&fixtures.path_for(&url), &trim_csv(&body, &keep))?;
                if newest_only.contains(&suffix) {
                    have.insert(suffix);
                }
            }
            Err(FetchError::NotFound(_)) => println!("not published: {url}"),
            Err(e) => return Err(e.into()),
        }
    }

    // Binding-constraint histories and the load forecast report (.xls, kept
    // whole): the newest of each, trying back a few days in case today's is
    // not out yet.
    for suffix in [reports::DA_BC, reports::RT_BC, reports::DF_AL] {
        for back in 0..4 {
            let url = endpoints.report_file(today - Duration::days(back), suffix, "xls");
            match ctx.get(&url).await {
                Ok(body) => {
                    let path = fixtures.path_for(&url);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&path, &body)?;
                    println!("{:>8} KB  {}", body.len() / 1024, path.display());
                    break;
                }
                Err(FetchError::NotFound(_)) => println!("not published: {url}"),
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

fn write(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)?;
    println!("{:>8} KB  {}", body.len() / 1024, path.display());
    Ok(())
}

/// Keep only `keep` nodes from a `{headers, data}` five-minute table.
fn trim_table(body: &str, keep: &HashSet<&str>) -> Result<String, serde_json::Error> {
    let mut v: serde_json::Value = serde_json::from_str(body)?;
    let node_col = v["headers"]
        .as_array()
        .and_then(|h| h.iter().position(|c| c == "CPNODE"))
        .unwrap_or(1);
    if let Some(rows) = v["data"].as_array_mut() {
        rows.retain(|r| r[node_col].as_str().is_some_and(|n| keep.contains(n)));
    }
    serde_json::to_string(&v)
}

/// Keep the banner, the header and rows for `keep` nodes from a report CSV.
fn trim_csv(body: &str, keep: &HashSet<&str>) -> String {
    let mut out = String::new();
    let mut in_table = false;
    for line in body.lines() {
        let node = line.split(',').next().unwrap_or_default();
        if !in_table || keep.contains(node) {
            out.push_str(line);
            out.push('\n');
        }
        in_table |= line.starts_with("Node,");
    }
    out
}
