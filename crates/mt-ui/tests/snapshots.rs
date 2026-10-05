//! Visual regression: render the real app offscreen against the fixtures, with
//! the clock frozen at their recording time, and compare with the committed
//! images in `tests/snapshots/`. After an intended visual change, review the
//! renders and accept them:
//!
//!     UPDATE_SNAPSHOTS=force cargo test -p mt-ui --test snapshots
//!
//! A mismatch leaves `<name>.new.png` and `<name>.diff.png` next to the
//! reference for inspection (both are git-ignored).

#[path = "common/scene.rs"]
mod scene;

use egui_kittest::{SnapshotOptions, try_image_snapshot_options};
use scene::Scene;

#[test]
fn the_terminal_looks_as_it_did() {
    let fixtures = scene::fixtures_dir();
    let at = scene::fixtures_time(&fixtures).expect("the fixtures have an LMP table");
    // Process-wide, which is why this is the only test in this binary.
    mt_core::time::freeze_clock(Some(mt_core::time::market_to_utc(at)));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    // One hub for every scene: the fixtures load once.
    let hub = scene::offline_hub(&runtime, &fixtures);
    // Text is rasterised by egui on the CPU, so renders agree closely across
    // GPUs; allow a little antialiasing drift (e.g. CI's software adapter).
    let options = SnapshotOptions::new().threshold(1.0).max_failed_pixels(400);

    // The default layout in every built-in theme.
    let themes: Vec<String> = mt_theme::ThemeRegistry::load(None)
        .themes()
        .iter()
        .map(|t| t.meta.id.clone())
        .collect();
    let mut scenes: Vec<(String, Scene<'_>)> = themes
        .iter()
        .map(|id| {
            (
                format!("layout-{id}"),
                Scene {
                    theme: Some(id),
                    ..Scene::default()
                },
            )
        })
        .collect();
    for (name, run) in [
        ("map-mcc", &["MAP MCC"][..]),
        ("dam", &["DAM TODAY"][..]),
        ("seam", &["SEAM"][..]),
        ("gp-history", &["GP MINN.HUB 3"][..]),
        ("top", &["TOP"][..]),
        ("ni-energy", &["NI ENERGY"][..]),
        ("q", &["Q"][..]),
        ("gp-security", &["GP XLU US"][..]),
        ("des", &["DES XLU US"][..]),
        ("cmp-security", &["CMP XLU US MINN.HUB 7"][..]),
        ("omon", &["OMON XLU US 2026-12-18"][..]),
        ("port", &["PORT"][..]),
        ("acct", &["ACCT"][..]),
        ("pnl", &["PNL"][..]),
        ("pnl-year", &["PNL 1Y"][..]),
        ("act", &["ACT"][..]),
        ("ticket", &["BUY XLU US 10 LMT 44.50 DAY"][..]),
        ("ticket-blocked", &["SELL XLU US 200"][..]),
        ("ticket-option", &["BUY XLU261218C00046000 2 DAY"][..]),
        ("ord", &["ORD ALL"][..]),
    ] {
        scenes.push((
            format!("zoom-{name}"),
            Scene {
                run,
                zoom: true,
                size: (1280.0, 800.0),
                ..Scene::default()
            },
        ));
    }

    let mut failures = Vec::new();
    for (name, scene) in &scenes {
        let image = scene::render(&hub, scene).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Err(e) = try_image_snapshot_options(&image, name.as_str(), &options) {
            failures.push(format!("{name}: {e}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
