//! Record EIA's Henry Hub workbook into `fixtures/` (offline mode, parser tests).
//!
//!     cargo run -p mt-eia --example capture_fixtures [DIR]

use std::path::PathBuf;
use std::sync::Arc;

use mt_data::{EventLog, FetchCtx, FetchCtxOptions, FixtureTransport, HttpTransport};
use mt_eia::Eia;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures").to_owned()),
    );
    let ctx = FetchCtx::new(
        Arc::new(HttpTransport::new(
            "MISO-Terminal fixture recorder (github.com/ArenKDesai/miso-terminal)",
        )?),
        None,
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let url = Eia::default().henry_hub_url();
    let body = ctx.get(&url).await?;
    let path = FixtureTransport::new(&out).path_for(&url);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &body)?;
    println!("{:>8} KB  {}", body.len() / 1024, path.display());
    Ok(())
}
