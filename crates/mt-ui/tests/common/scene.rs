//! Build the real app offscreen against recorded fixtures and render it to an
//! image. Shared by `examples/render.rs` and `tests/snapshots.rs` (included
//! with `#[path]`, so it may use dev-dependencies).

#![allow(dead_code)] // each includer uses a different part

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mt_data::{DataHub, EventLog, FetchCtx, FetchCtxOptions, FixtureTransport};
use mt_ui::{AppConfig, AppPaths, Deps, TerminalApp};

/// What to show.
pub struct Scene<'a> {
    /// Commands to run first, as typed on the command line.
    pub run: &'a [&'a str],
    /// Zoom the focused panel (as Ctrl+M does).
    pub zoom: bool,
    /// Then click the widgets with these labels, in turn (to open a menu).
    pub click: &'a [&'a str],
    pub theme: Option<&'a str>,
    /// Theme files to install in the user themes folder first (e.g. a gallery theme).
    pub theme_files: &'a [PathBuf],
    /// Window size in points.
    pub size: (f32, f32),
    pub pixels_per_point: f32,
}

impl Default for Scene<'_> {
    fn default() -> Self {
        Self {
            run: &[],
            zoom: false,
            click: &[],
            theme: None,
            theme_files: &[],
            size: (1600.0, 960.0),
            pixels_per_point: 1.0,
        }
    }
}

/// The repository's recorded responses.
pub fn fixtures_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    // Without Windows' verbatim prefix, which the status bar would show.
    match std::fs::canonicalize(&dir) {
        Ok(p) => PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\")),
        Err(_) => dir,
    }
}

/// When the fixtures were recorded: just after the LMP table's latest
/// interval. Freezing the clock there shows them as they looked live.
pub fn fixtures_time(fixtures: &Path) -> Option<chrono::NaiveDateTime> {
    let body = std::fs::read_to_string(
        fixtures.join("public-api.misoenergy.org/api/MarketPricing/GetLmpConsolidatedTable.json"),
    )
    .ok()?;
    let board = mt_miso::parse::parse_lmp_board(&body).ok()?;
    Some(board.interval? + chrono::Duration::minutes(5))
}

/// A data hub replaying `fixtures`, with no politeness delay and no disk cache.
pub fn offline_hub(runtime: &tokio::runtime::Runtime, fixtures: &Path) -> DataHub {
    offline_hub_with_cache(runtime, fixtures, None)
}

/// The same, reading local data (archives) from a disk cache.
pub fn offline_hub_with_cache(
    runtime: &tokio::runtime::Runtime,
    fixtures: &Path,
    cache: Option<&Path>,
) -> DataHub {
    DataHub::new(
        runtime.handle().clone(),
        FetchCtx::new(
            Arc::new(FixtureTransport::new(fixtures).labelled("recorded MISO data")),
            cache.map(mt_data::DiskCache::new),
            FetchCtxOptions {
                max_concurrent: 8,
                polite_interval: Duration::ZERO,
                ..FetchCtxOptions::default()
            },
            EventLog::default(),
        ),
    )
}

/// Run the app until its data has loaded, apply the scene, and render it.
pub fn render(hub: &DataHub, scene: &Scene<'_>) -> Result<image::RgbaImage, String> {
    let home = std::env::temp_dir().join(format!(
        "mt-scene-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let mut config = AppConfig::default();
    // The machine's time zone would make renders differ between machines.
    config.ui.show_local_clock = false;
    if let Some(theme) = scene.theme {
        config.theme = theme.to_owned();
    }
    let paths = AppPaths::under(&home);
    for file in scene.theme_files {
        let name = file.file_name().ok_or("theme file has no name")?;
        std::fs::create_dir_all(&paths.themes_dir)
            .and_then(|()| std::fs::copy(file, paths.themes_dir.join(name)))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let deps = Deps {
        hub: hub.clone(),
        config,
        config_error: None,
        paths,
        reset_layout: true,
        startup_commands: scene.run.iter().map(|s| (*s).to_owned()).collect(),
        remote: None,
        notifier: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(scene.size.0, scene.size.1))
        .with_pixels_per_point(scene.pixels_per_point)
        .wgpu()
        .build_eframe(|cc| TerminalApp::new(cc, deps));

    // Step until nothing has been in flight for a while.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut quiet = 0;
    while quiet < 10 && Instant::now() < deadline {
        harness.step();
        std::thread::sleep(Duration::from_millis(30));
        quiet = if hub.in_flight() == 0 { quiet + 1 } else { 0 };
    }
    if scene.zoom {
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::M);
    }
    // A few more frames for layout (tables size their columns on the first pass).
    for _ in 0..5 {
        harness.step();
    }
    for label in scene.click {
        let Some(node) = harness.query_by_label(label) else {
            let _ = std::fs::remove_dir_all(&home);
            return Err(format!("nothing labelled {label:?} to click"));
        };
        node.click();
        for _ in 0..5 {
            harness.step();
        }
    }
    let image = harness.render();
    let _ = std::fs::remove_dir_all(&home);
    image
}
