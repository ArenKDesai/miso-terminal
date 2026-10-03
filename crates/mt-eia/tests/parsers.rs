//! EIA parsers against a recorded workbook (`MT_FIXTURES` to use another).

use std::path::PathBuf;

use mt_data::FixtureTransport;
use mt_eia::Eia;
use mt_eia::parse::parse_spot_history;

fn root() -> PathBuf {
    std::env::var_os("MT_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

#[test]
fn henry_hub_history() {
    let path = FixtureTransport::new(root()).path_for(&Eia::default().henry_hub_url());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let s = parse_spot_history("Henry Hub", "$/MMBtu", &bytes).expect("parses");
    assert!(
        s.points.len() > 5000,
        "daily since 1997: {}",
        s.points.len()
    );
    assert!(
        s.points.windows(2).all(|w| w[0].0 < w[1].0),
        "ascending dates"
    );
    assert!(s.points.iter().all(|p| (0.0..100.0).contains(&p.1)));
    let (last, _) = s.latest().expect("a latest price");
    assert!(last > chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
}
