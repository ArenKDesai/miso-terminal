//! Headless smoke tests: every function, every built-in theme, with no data and
//! with the recorded fixtures. New functions are covered automatically because
//! the tests iterate the registry.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::App as _;
use mt_data::{DataHub, EntryState, EventLog, FetchCtx, FetchCtxOptions, FixtureTransport};
use mt_miso::Miso;
use mt_theme::ThemeRegistry;

use crate::config::{AppConfig, AppPaths};
use crate::context::{AppCommand, PanelCx};
use crate::fonts::FontLibrary;
use crate::function::{Panel, Registry, Route};
use crate::skin::Skin;
use crate::{Deps, TerminalApp};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn hub(rt: &tokio::runtime::Runtime) -> DataHub {
    let ctx = FetchCtx::new(
        Arc::new(FixtureTransport::new(fixtures())),
        None,
        FetchCtxOptions {
            max_concurrent: 8,
            polite_interval: Duration::ZERO,
        },
        EventLog::default(),
    );
    DataHub::new(rt.handle().clone(), ctx)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn temp_paths(tag: &str) -> AppPaths {
    AppPaths::under(&std::env::temp_dir().join(format!("mt-smoke-{tag}-{}", std::process::id())))
}

/// Tessellate like a real frame would (this is where NaN geometry panics) and
/// release the texture updates a renderer would have consumed.
fn finish_frame(ctx: &egui::Context, mut out: egui::FullOutput) {
    out.textures_delta.clear();
    let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
}

/// Every route worth exercising: each function bare, plus argument variants.
fn routes(registry: &Registry) -> Vec<Route> {
    let mut out: Vec<Route> = registry
        .specs()
        .iter()
        .map(|s| Route::code(s.code))
        .collect();
    out.push(Route::new("GP", ["MINN.HUB"]));
    out.push(Route::new("GP", ["MINN.HUB", "3"]));
    out.push(Route::new("GP", ["NOT.A.NODE"]));
    out.push(Route::new("LMP", ["ALL"]));
    out.push(Route::new("MAP", ["MCC"]));
    out.push(Route::new("GP", ["ALTE.ALTE", "7", "HEAT"]));
    out.push(Route::new("GP", ["MINN.HUB", "7", "DUR"]));
    out.push(Route::new("SPRD", ["MINN.HUB", "ILLINOIS.HUB", "7", "DUR"]));
    out.push(Route::new(
        "SPRD",
        ["MINN.HUB", "ILLINOIS.HUB", "7", "HEAT"],
    ));
    out.push(Route::new("MAP", ["DART"]));
    out.push(Route::new("SPRD", ["MINN.HUB", "ILLINOIS.HUB"]));
    out.push(Route::new("SPRD", ["MINN.HUB", "ILLINOIS.HUB", "3"]));
    out.push(Route::new("WL", ["ALTE.ALTE"]));
    out.push(Route::new("THEME", ["everforge-light"]));
    out
}

struct Harness {
    ctx: egui::Context,
    hub: DataHub,
    miso: Miso,
    nws: mt_nws::Nws,
    config: AppConfig,
    paths: AppPaths,
    registry: Registry,
    themes: ThemeRegistry,
    alerts: crate::alerts::AlertEngine,
}

impl Harness {
    fn new(hub: DataHub) -> Self {
        Self {
            ctx: egui::Context::default(),
            hub,
            miso: Miso::default(),
            nws: mt_nws::Nws::default(),
            config: AppConfig::default(),
            paths: temp_paths("panels"),
            registry: Registry::builtin(),
            themes: ThemeRegistry::load(None),
            alerts: crate::alerts::AlertEngine::default(),
        }
    }

    fn apply(&self, skin: &Skin) {
        let (defs, warnings) = FontLibrary::new(None).definitions(&skin.theme.fonts);
        assert!(warnings.is_empty(), "{}: {warnings:?}", skin.theme.meta.id);
        self.ctx.set_fonts(defs);
        self.ctx.all_styles_mut(|s| skin.apply_style(s));
    }

    /// Draw one panel for a frame (twice: fonts load on the first pass).
    fn draw(&self, skin: &Skin, panel: &mut dyn Panel) -> Vec<AppCommand> {
        self.draw_with(skin, |ui, cx| {
            panel.ui(ui, cx);
            let _ = panel.title();
            let _ = panel.route();
        })
    }

    /// Run two frames of arbitrary UI with a full `PanelCx`.
    fn draw_with(
        &self,
        skin: &Skin,
        mut f: impl FnMut(&mut egui::Ui, &mut PanelCx<'_>),
    ) -> Vec<AppCommand> {
        let mut commands = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            };
            let out = self.ctx.run_ui(input, |ui| {
                let mut cx = PanelCx {
                    hub: &self.hub,
                    miso: &self.miso,
                    nws: &self.nws,
                    skin,
                    config: &self.config,
                    paths: &self.paths,
                    registry: &self.registry,
                    themes: &self.themes,
                    notices: &[],
                    alerts: &self.alerts,
                    commands: &mut commands,
                };
                f(ui, &mut cx);
            });
            finish_frame(&self.ctx, out);
        }
        commands
    }
}

/// Watch everything once, then wait for the fixture fetches to settle.
fn load_everything(h: &Harness, skin: &Skin, panels: &mut [Box<dyn Panel>]) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for p in panels.iter_mut() {
            h.draw(skin, p.as_mut());
        }
        std::thread::sleep(Duration::from_millis(50));
        if h.hub.in_flight() == 0 && !h.hub.status().iter().any(|s| s.state == EntryState::Empty) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixtures did not load: {:#?}",
            h.hub.status()
        );
    }
}

#[test]
fn every_function_renders_in_every_theme_with_and_without_data() {
    let rt = runtime();
    let registry = Registry::builtin();
    let themes = mt_theme::builtin();

    // No data: a paused hub never fetches, so panels must render placeholders.
    let empty = Harness::new(hub(&rt));
    empty.hub.set_paused(true);
    for theme in &themes {
        let skin = Skin::new(theme.clone());
        empty.apply(&skin);
        for route in routes(&registry) {
            let mut panel = registry
                .open(&route)
                .unwrap_or_else(|e| panic!("{route}: {e}"));
            empty.draw(&skin, panel.as_mut());
        }
    }

    // Fixture data: every feed every panel asks for must load and parse.
    let full = Harness::new(hub(&rt));
    // An unpublished RT day puts a gap in GP's history chart.
    let yesterday = mt_core::time::market_today() - chrono::Duration::days(1);
    full.hub.seed(&full.miso.rt_best_day(yesterday), None);
    let skin = Skin::new(themes[0].clone());
    full.apply(&skin);
    let mut panels: Vec<Box<dyn Panel>> = routes(&registry)
        .iter()
        .map(|r| registry.open(r).unwrap())
        .collect();
    // States that no route expresses.
    panels.push(crate::functions::gp::with_yesterday("MINN.HUB"));
    load_everything(&full, &skin, &mut panels);
    let failed: Vec<_> = full
        .hub
        .status()
        .into_iter()
        .filter(|s| s.state == EntryState::Error)
        .collect();
    assert!(
        failed.is_empty(),
        "feeds failed against fixtures: {failed:#?}"
    );
    for theme in &themes {
        let skin = Skin::new(theme.clone());
        full.apply(&skin);
        for p in &mut panels {
            full.draw(&skin, p.as_mut());
        }
    }
}

#[test]
fn routes_round_trip_through_panels() {
    let registry = Registry::builtin();
    for route in routes(&registry) {
        let panel = registry.open(&route).unwrap();
        let back = panel.route();
        assert_eq!(
            registry.find(&back.code).map(|s| s.code),
            registry.find(&route.code).map(|s| s.code),
            "{route}"
        );
        assert!(
            registry.open(&back).is_ok(),
            "{route} -> {back} does not reopen"
        );
    }
}

#[test]
fn app_shell_runs_frames_and_executes_commands() {
    let rt = runtime();
    let ctx = egui::Context::default();
    // An always-true alert, to check rules are evaluated against live (fixture) data.
    let config = AppConfig {
        alerts: vec![crate::alerts::AlertRule::PriceAbove {
            node: "MINN.HUB".into(),
            value: -10_000.0,
        }],
        ..AppConfig::default()
    };
    let deps = Deps {
        hub: hub(&rt),
        config,
        config_error: Some("example config problem".into()),
        paths: temp_paths("shell"),
        reset_layout: true,
        startup_commands: vec!["FUEL".into(), "GP INDIANA.HUB".into(), "ALRT".into()],
    };
    let mut app = TerminalApp::headless(&ctx, deps);
    let mut frame = eframe::Frame::_new_kittest();
    let mut run = |app: &mut TerminalApp| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 960.0),
            )),
            ..Default::default()
        };
        finish_frame(&ctx, ctx.run_ui(input, |ui| app.ui(ui, &mut frame)));
    };
    for _ in 0..3 {
        run(&mut app);
    }
    let routes = app.workspace_mut().routes();
    for code in ["GP", "ALRT"] {
        assert!(
            routes.iter().any(|r| r.code == code),
            "startup command {code} did not open: {routes:?}"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.alerts.history.is_empty() {
        assert!(Instant::now() < deadline, "the alert never fired");
        std::thread::sleep(Duration::from_millis(50));
        run(&mut app);
    }
    assert!(app.alerts.history[0].rule.contains("MINN.HUB"));
    let before = app.workspace_mut().routes().len();
    app.apply_commands(
        &ctx,
        vec![
            AppCommand::Run("GP MICHIGAN.HUB".into()),
            AppCommand::Run("bogus words".into()),
        ],
    );
    app.apply_commands(&ctx, vec![AppCommand::Open(Route::code("LMP"))]); // already open: focus, not duplicate
    // WL absorbs `WL <node>` into the open watchlist (the default layout has one).
    app.apply_commands(&ctx, vec![AppCommand::Run("WL ALTE.ALTE".into())]);
    assert_eq!(app.workspace_mut().routes().len(), before + 1);
    app.apply_commands(&ctx, vec![AppCommand::Run("THEME amber-terminal".into())]);
    run(&mut app);
    app.apply_commands(&ctx, vec![AppCommand::ResetLayout]);
    run(&mut app);
    let _ = std::fs::remove_dir_all(app.paths.config_file.parent().unwrap());
}

#[test]
fn readme_lists_every_function() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
        .expect("README.md at the repository root");
    for spec in Registry::builtin().specs() {
        assert!(
            readme.contains(&format!("| `{}` |", spec.code)),
            "the README functions table is missing {}",
            spec.code
        );
    }
}

#[test]
fn a_panicking_panel_is_contained_to_its_tab() {
    use crate::workspace::{draw_tab, tests as ws};
    let rt = runtime();
    let h = Harness::new(hub(&rt));
    let skin = Skin::new(mt_theme::builtin().remove(0));
    h.apply(&skin);
    let mut tab = ws::bomb_tab();
    // Two frames: the first catches the panic, the second shows the error page.
    h.draw_with(&skin, |ui, cx| draw_tab(ui, cx, &mut tab));
    assert_eq!(ws::crashed(&tab), Some("boom in a panel"));
}
