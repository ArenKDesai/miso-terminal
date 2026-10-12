//! The operating feeds kept by day, against the recorded fixtures: each
//! source turns its feed into rows a day file can hold.

use std::path::PathBuf;
use std::sync::Arc;

use mt_data::{EventLog, FetchCtx, FetchCtxOptions, FixtureTransport};
use mt_miso::MisoEndpoints;

#[tokio::test]
async fn every_source_yields_rows() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let ctx = FetchCtx::new(
        Arc::new(FixtureTransport::new(fixtures)),
        None,
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let sources = mt_miso::ops::sources(&MisoEndpoints::default());
    assert_eq!(sources.len(), mt_miso::ops::KINDS.len());
    for s in sources {
        let batches = s
            .poll(&ctx)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", s.name()));
        assert_eq!(batches.len(), 1, "{}", s.name());
        let b = &batches[0];
        assert!(mt_miso::ops::KINDS.contains(&b.kind));
        assert!(!b.series.is_empty(), "{}", s.name());
        assert!(!b.rows.is_empty(), "{} has rows", s.name());
        for (t, values) in &b.rows {
            assert_eq!(values.len(), b.series.len(), "{} at {t}", s.name());
        }
        assert!(
            b.rows.windows(2).all(|w| w[0].0 <= w[1].0),
            "{} in time order",
            s.name()
        );
    }
}
