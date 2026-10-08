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
            ..FetchCtxOptions::default()
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
    out.push(Route::new("HUBS", ["3"]));
    out.push(Route::new("DAM", ["YESTERDAY"]));
    out.push(Route::new("BCH", ["2026-09-30"]));
    out.push(Route::new(
        "CMP",
        ["MINN.HUB", "MICHIGAN.HUB", "ILLINOIS.HUB"],
    ));
    out.push(Route::new("CMP", ["MINN.HUB", "MICHIGAN.HUB", "3"]));
    out.push(Route::new("GP", ["ALTE.ALTE", "7", "HEAT"]));
    out.push(Route::new("GP", ["MINN.HUB", "7", "DUR"]));
    out.push(Route::new("GP", ["MINN.HUB", "3", "5MIN"]));
    out.push(Route::new(
        "SPRD",
        ["MINN.HUB", "ILLINOIS.HUB", "3", "5MIN"],
    ));
    out.push(Route::new("SPRD", ["MINN.HUB", "ILLINOIS.HUB", "7", "DUR"]));
    out.push(Route::new(
        "SPRD",
        ["MINN.HUB", "ILLINOIS.HUB", "7", "HEAT"],
    ));
    out.push(Route::new("MAP", ["DART"]));
    out.push(Route::new("SPRD", ["MINN.HUB", "ILLINOIS.HUB"]));
    out.push(Route::new("SPRD", ["MINN.HUB", "ILLINOIS.HUB", "3"]));
    out.push(Route::new("WL", ["ALTE.ALTE"]));
    out.push(Route::new("THEME", ["default-light"]));
    out.push(Route::new("NEWS", ["FT"]));
    out.push(Route::new("NEWS", ["natural", "gas"]));
    out.push(Route::new("NI", ["ENERGY"]));
    out.push(Route::new("NI", ["NOT-A-TOPIC"]));
    // Securities.
    out.push(Route::new("Q", ["UTILITIES"]));
    out.push(Route::new("Q", ["WL"]));
    out.push(Route::new("Q", ["NO-SUCH-LIST"]));
    out.push(Route::new("Q", ["XEL US", "AEE US"]));
    out.push(Route::new("GP", ["XLU US"]));
    out.push(Route::new("GP", ["XLU US", "5"]));
    out.push(Route::new("GP", ["XLU US", "365"]));
    out.push(Route::new("GP", ["NOTATICKER US"]));
    out.push(Route::new("DES", ["XLU US"]));
    out.push(Route::new("DES", ["NOTATICKER US"]));
    out.push(Route::new("CN", ["XLU US"]));
    out.push(Route::new("WL", ["XLU US"]));
    out.push(Route::new("CMP", ["XLU US", "MINN.HUB", "7"]));
    out.push(Route::new(
        "CMP",
        ["XLU US", "XEL US", "MINN.HUB", "ILLINOIS.HUB", "30"],
    ));
    out.push(Route::new("CMP", ["XEL US"]));
    // Options.
    out.push(Route::new("OMON", ["XLU US"]));
    out.push(Route::new("OMON", ["XLU US", "2026-12-18", "ALL"]));
    out.push(Route::new("OMON", ["XLU US", "2025-01-17"]));
    out.push(Route::new("OMON", ["VST US", "3"]));
    out.push(Route::new("OMON", ["XLU261218P00035000"]));
    out.push(Route::new("OMON", ["NOTATICKER US"]));
    // The paper account.
    for p in ["1W", "1M", "3M", "1Y"] {
        out.push(Route::new("PNL", [p]));
    }
    for k in ["FILLS", "DIV", "OPTIONS", "FEES"] {
        out.push(Route::new("ACT", [k]));
    }
    // Trading: tickets and the blotter.
    out.push(Route::new("BUY", ["XLU US", "10", "LMT", "44.50", "DAY"]));
    out.push(Route::new("BUY", ["XLU US", "10", "MKT"]));
    out.push(Route::new("SELL", ["XLU US", "200"]));
    out.push(Route::new(
        "SELL",
        ["UNG US", "10", "STPLMT", "150", "149.50", "GTC"],
    ));
    out.push(Route::new("BUY", ["XEL US", "1000", "LMT", "90", "DAY"]));
    out.push(Route::new("SELL", ["NOTATICKER US", "1"]));
    // Option tickets: buy to open, close the held call, a covered call short
    // of shares, a cash-secured put, a market order.
    out.push(Route::new(
        "BUY",
        ["XLU261218C00046000", "2", "LMT", "1.16"],
    ));
    out.push(Route::new("SELL", ["XLU261218C00045000", "2"]));
    out.push(Route::new("SELL", ["XLU261218C00047000", "2", "GTC"]));
    out.push(Route::new("SELL", ["VST261120P00034000", "1"]));
    out.push(Route::new("BUY", ["XLU261218P00044000", "1", "MKT"]));
    // Spreads: a debit, a credit, an iron condor, a calendar (two chains),
    // none and one leg.
    out.push(Route::new(
        "MLEG",
        ["+XLU261218C00045000", "-XLU261218C00047000", "2"],
    ));
    out.push(Route::new(
        "MLEG",
        ["-XLU261218C00045000", "+XLU261218C00047000", "LMT", "-0.70"],
    ));
    out.push(Route::new(
        "MLEG",
        [
            "+XLU261218P00040000",
            "-XLU261218P00042000",
            "-XLU261218C00047000",
            "+XLU261218C00049000",
            "LMT",
            "-0.60",
        ],
    ));
    out.push(Route::new(
        "MLEG",
        ["-XLU261120C00045000", "+XLU261218C00045000", "MKT"],
    ));
    out.push(Route::new("MLEG", ["+XLU261218C00045000"]));
    for v in ["FILLED", "CANCELED", "ALL", "KILL"] {
        out.push(Route::new("ORD", [v]));
    }
    out
}

struct Harness {
    ctx: egui::Context,
    hub: DataHub,
    miso: Miso,
    nws: mt_nws::Nws,
    eia: mt_eia::Eia,
    alpaca: mt_alpaca::Alpaca,
    desk: mt_alpaca::OrderDesk,
    backfill: mt_miso::Backfill,
    config: AppConfig,
    paths: AppPaths,
    registry: Registry,
    themes: ThemeRegistry,
    alerts: crate::alerts::AlertEngine,
    news_read: mt_news::ReadMarks,
}

impl Harness {
    fn new(hub: DataHub) -> Self {
        // Fixtures need no keys.
        let alpaca = mt_alpaca::Alpaca::new(&mt_alpaca::MarketsConfig::default(), true);
        Self {
            ctx: egui::Context::default(),
            desk: mt_alpaca::OrderDesk::new(
                &alpaca,
                hub.ctx().clone(),
                hub.runtime().clone(),
                mt_alpaca::AuditLog::in_memory(),
            ),
            // A replay: SET and LOG show it as unavailable.
            backfill: mt_miso::Backfill::new(hub.ctx().clone(), hub.runtime()),
            hub,
            miso: Miso::default(),
            nws: mt_nws::Nws::default(),
            eia: mt_eia::Eia::default(),
            alpaca,
            config: AppConfig {
                ui: crate::config::UiConfig {
                    favorite_securities: vec!["XLU US".into(), "VST US".into()],
                    ..crate::config::UiConfig::default()
                },
                ..AppConfig::default()
            },
            paths: temp_paths("panels"),
            registry: Registry::builtin(),
            themes: ThemeRegistry::load(None),
            alerts: crate::alerts::AlertEngine::default(),
            news_read: mt_news::ReadMarks::default(),
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
                    eia: &self.eia,
                    alpaca: &self.alpaca,
                    desk: &self.desk,
                    backfill: &self.backfill,
                    skin,
                    config: &self.config,
                    paths: &self.paths,
                    registry: &self.registry,
                    themes: &self.themes,
                    notices: &[],
                    alerts: &self.alerts,
                    news_read: &self.news_read,
                    // Exercise the notification controls too.
                    can_notify: true,
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
    // The built-ins plus the downloadable gallery, as if installed: a gallery
    // theme must not break a panel either (and Everforge Light is the only light one).
    let gallery = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/gallery");
    let installed = ThemeRegistry::load(Some(&gallery));
    assert!(installed.errors().is_empty(), "{:?}", installed.errors());
    let themes = installed.themes().to_vec();
    assert!(themes.len() > mt_theme::builtin().len());

    // No data: a paused hub never fetches, so panels must render placeholders.
    // Without Alpaca keys, securities panels say how to add them.
    let mut empty = Harness::new(hub(&rt));
    empty.hub.set_paused(true);
    for (i, theme) in themes.iter().enumerate() {
        let skin = Skin::new(theme.clone());
        empty.apply(&skin);
        empty.alpaca = mt_alpaca::Alpaca::new(&mt_alpaca::MarketsConfig::default(), i == 0);
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
    let (remote, inbox) = crate::remote::channel_pair();
    let deps = Deps {
        hub: hub(&rt),
        config,
        config_error: Some("example config problem".into()),
        paths: temp_paths("shell"),
        reset_layout: true,
        startup_commands: vec!["FUEL".into(), "GP INDIANA.HUB".into(), "ALRT".into()],
        remote: Some(inbox),
        notifier: None,
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
    // A later launch of the app forwards its commands to this window.
    assert!(remote.send(vec!["SPRD MINN.HUB TEXAS.HUB".into()]));
    run(&mut app);
    assert!(
        app.workspace_mut()
            .routes()
            .iter()
            .any(|r| r.code == "SPRD"),
        "a forwarded command did not open"
    );
    // Zoom the focused panel, draw it alone, and come back.
    app.apply_commands(&ctx, vec![AppCommand::ToggleZoom]);
    run(&mut app);
    assert!(app.workspace_mut().is_zoomed());
    app.apply_commands(&ctx, vec![AppCommand::Zoom(None)]);
    run(&mut app);
    assert!(!app.workspace_mut().is_zoomed());
    // Pop the focused panel out (headless egui embeds it as a floating window),
    // draw it, and dock it back.
    let tab = app
        .workspace_mut()
        .dock
        .find_active_focused()
        .map(|(_, t)| t.id)
        .expect("a focused tab");
    app.apply_commands(&ctx, vec![AppCommand::PopOut(tab)]);
    run(&mut app);
    run(&mut app);
    assert_eq!(app.workspace_mut().popped.len(), 1);
    app.apply_commands(&ctx, vec![AppCommand::DockBack(tab)]);
    run(&mut app);
    assert!(app.workspace_mut().popped.is_empty());
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
    // Securities: `XLU US GP 30` charts one, and so does a bare security.
    for (cmd, opens) in [("XLU US GP 30", "GP XLU US 30"), ("xlu us", "GP XLU US")] {
        app.apply_commands(&ctx, vec![AppCommand::Run(cmd.into())]);
        let (msg, error) = app.last_feedback().cloned().unwrap_or_default();
        assert!(!error && msg == opens, "{cmd}: {msg}");
        assert!(
            app.workspace_mut()
                .routes()
                .iter()
                .any(|r| r.to_string() == opens)
        );
    }
    // Functions that take no options refuse them, and those that take only
    // nodes refuse securities; a bare option opens its chain.
    for (cmd, says) in [
        ("GP XLU261218C00082500", "is an option"),
        ("HUBS XLU US", "is a security"),
    ] {
        app.apply_commands(&ctx, vec![AppCommand::Run(cmd.into())]);
        let (msg, error) = app.last_feedback().cloned().unwrap_or_default();
        assert!(error && msg.contains(says), "{cmd}: {msg}");
    }
    assert_eq!(app.workspace_mut().routes().len(), before + 3);
    app.apply_commands(&ctx, vec![AppCommand::Run("XLU261218C00045000".into())]);
    let (msg, error) = app.last_feedback().cloned().unwrap_or_default();
    assert!(!error && msg == "OMON XLU261218C00045000", "{msg}");
    assert_eq!(app.workspace_mut().routes().len(), before + 4);
    // A security on the watchlist goes to its own list.
    app.apply_commands(&ctx, vec![AppCommand::AddFavorite("xel us".into())]);
    assert!(
        app.config()
            .ui
            .favorite_securities
            .contains(&"XEL US".to_owned())
    );
    assert!(
        !app.config()
            .ui
            .favorite_nodes
            .iter()
            .any(|n| n.contains("XEL"))
    );
    app.apply_commands(&ctx, vec![AppCommand::RemoveFavorite("XEL US".into())]);
    assert!(app.config().ui.favorite_securities.is_empty());
    app.apply_commands(&ctx, vec![AppCommand::Run("THEME high-contrast".into())]);
    run(&mut app);
    assert_eq!(app.active_theme_id(), "high-contrast");
    // Install a gallery theme and switch to it; removing it goes back to the default.
    let src = include_str!("../../../themes/gallery/tokyo-night.toml");
    let theme = Box::new(mt_theme::GalleryTheme {
        theme: mt_theme::Theme::from_toml(src).unwrap(),
        source: src.into(),
    });
    let installed = app.paths.themes_dir.join("tokyo-night.toml");
    app.apply_commands(
        &ctx,
        vec![AppCommand::InstallTheme {
            theme,
            activate: true,
        }],
    );
    run(&mut app);
    assert_eq!(app.active_theme_id(), "tokyo-night");
    assert!(installed.exists());
    app.apply_commands(&ctx, vec![AppCommand::UninstallTheme("tokyo-night".into())]);
    run(&mut app);
    assert_eq!(app.active_theme_id(), mt_theme::DEFAULT_THEME_ID);
    assert!(!installed.exists());
    let open = app.workspace_mut().routes().len();
    app.apply_commands(
        &ctx,
        vec![
            AppCommand::CycleTab(true),
            AppCommand::CycleTab(false),
            AppCommand::CloseTab,
        ],
    );
    assert!(
        app.workspace_mut().routes().len() <= open,
        "close never adds tabs"
    );
    app.apply_commands(&ctx, vec![AppCommand::ResetLayout]);
    run(&mut app);

    // SET's *Download now* and *Pause* are saved at once (a replay has
    // nothing to download, and says so).
    app.apply_commands(&ctx, vec![AppCommand::SetBackfill(true)]);
    assert!(app.config().price_history.backfill);
    assert!(
        AppConfig::load(&app.paths.config_file)
            .0
            .price_history
            .backfill
    );
    run(&mut app);
    app.apply_commands(&ctx, vec![AppCommand::SetBackfill(false)]);
    assert!(!app.config().price_history.backfill);

    // SET's *Reset to defaults*: the old file is kept, the defaults are live
    // and saved, and the shell keeps running.
    app.apply_commands(&ctx, vec![AppCommand::SetNotifyAlerts(false)]);
    app.apply_commands(&ctx, vec![AppCommand::ResetConfig]);
    assert_eq!(app.config(), &AppConfig::default());
    let backup = AppConfig::backup_path(&app.paths.config_file);
    let (old, _) = AppConfig::load(&backup);
    assert!(
        !old.ui.notify_alerts && old.alerts.len() == 1,
        "backup holds the old settings"
    );
    assert_eq!(
        AppConfig::load(&app.paths.config_file).0,
        AppConfig::default()
    );
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

#[test]
fn intraday_prices_survive_a_restart() {
    use mt_core::time::{market_to_utc, market_today};
    use mt_core::{RtIntraday, RtRow};
    use mt_data::DiskCache;

    let rt = runtime();
    let paths = temp_paths("restart");
    let cache = DiskCache::new(paths.cache_dir.join("http"));
    let key = crate::app::intraday_key(market_today());
    let at = market_today().and_hms_opt(10, 0, 0).unwrap();
    let row = RtRow {
        interval: at,
        node: "MINN.HUB".into(),
        lmp: 12.5,
        mcc: 0.0,
        mlc: 0.0,
    };
    cache
        .put(&key, &RtIntraday::from_rows([row]).to_bytes())
        .unwrap();

    let ctx = FetchCtx::new(
        Arc::new(FixtureTransport::new(fixtures())),
        Some(cache.clone()),
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let hub = DataHub::new(rt.handle().clone(), ctx);
    hub.set_paused(true); // nothing may come from the network in this test
    let deps = Deps {
        hub: hub.clone(),
        config: AppConfig::default(),
        config_error: None,
        paths: paths.clone(),
        reset_layout: true,
        startup_commands: Vec::new(),
        remote: None,
        notifier: None,
    };
    let mut app = TerminalApp::headless(&egui::Context::default(), deps);

    let snap = hub.peek(&Miso::default().rt_intraday());
    let store = snap.data().expect("restored before any fetch");
    assert_eq!(store.latest("MINN.HUB").map(|(_, p)| p.lmp), Some(12.5));
    assert_eq!(
        snap.updated,
        Some(market_to_utc(at + chrono::Duration::minutes(5)))
    );

    // Exiting writes the store back (here unchanged, but freshly written).
    std::fs::remove_file(cache.path_for(&key)).unwrap();
    eframe::App::on_exit(&mut app);
    assert!(cache.get(&key).is_some(), "saved on exit");
    let _ = std::fs::remove_dir_all(paths.config_file.parent().unwrap());
}

#[test]
fn history_reads_stored_days_older_than_charts_download() {
    use crate::series::{self, Component, DOWNLOAD_DAYS};
    use mt_core::{DayLmpReport, DayNodeRow, DayReportKind};

    let rt = runtime();
    let root = std::env::temp_dir().join(format!("mt-smoke-stored-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache = mt_data::DiskCache::new(root.join("cache"));
    let today = mt_core::time::market_today();
    let old = today - chrono::Duration::days(200);
    for (kind, lmp) in [
        (DayReportKind::DaExPost, 42.0),
        (DayReportKind::RtFinal, 43.0),
    ] {
        let row = DayNodeRow {
            node: "MINN.HUB".into(),
            node_type: "Hub".into(),
            lmp: [lmp; 24],
            mcc: [0.0; 24],
            mlc: [0.0; 24],
        };
        let report = DayLmpReport::new(kind, old, vec![row]);
        cache
            .put(
                &mt_miso::day_store_key(kind.market(), old),
                &report.to_bytes(),
            )
            .unwrap();
    }
    // The fixtures stand in for MISO for the recent days a chart downloads.
    let live = FetchCtx::new(
        Arc::new(LiveLooking {
            fixtures: FixtureTransport::new(fixtures()),
            writes: parking_lot::Mutex::default(),
        }),
        Some(cache),
        FetchCtxOptions {
            max_concurrent: 8,
            polite_interval: Duration::ZERO,
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let h = Harness::new(DataHub::new(rt.handle().clone(), live));
    let skin = Skin::new(mt_theme::builtin().remove(0));
    h.apply(&skin);
    let deadline = Instant::now() + Duration::from_secs(30);
    let year = loop {
        let mut got = None;
        h.draw_with(&skin, |_, cx| {
            got = Some(series::node_history(cx, "MINN.HUB", Component::Lmp, 365));
        });
        let got = got.unwrap();
        if got.pending == 0 {
            break got;
        }
        assert!(Instant::now() < deadline, "still {} pending", got.pending);
        std::thread::sleep(Duration::from_millis(20));
    };
    let on = |pts: &series::Points| -> Vec<f64> {
        pts.iter()
            .filter(|(t, _)| t.date() == old)
            .map(|p| p.1)
            .collect()
    };
    assert_eq!(on(&year.da), vec![42.0; 24], "DA from the store");
    assert_eq!(on(&year.rt), vec![43.0; 24], "RT from the store");
    // Before the download window, the other days are counted, not fetched.
    assert_eq!(year.unstored_days, (365 - DOWNLOAD_DAYS - 1) as usize);
    let download_from = today - chrono::Duration::days(i64::from(DOWNLOAD_DAYS) - 1);
    for s in h.hub.status() {
        if let Some(day) = s.key.strip_prefix("miso/report/").and_then(|k| {
            k.rsplit('/')
                .next()
                .and_then(|d| d.parse::<chrono::NaiveDate>().ok())
        }) {
            assert!(day >= download_from, "{} was downloaded", s.key);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_config_that_failed_to_load_never_prunes_the_price_history() {
    use mt_core::{DayLmpReport, DayReportKind, Market};
    use mt_data::DiskCache;
    use mt_miso::history::Phase;

    let rt = runtime();
    let paths = temp_paths("history-guard");
    let cache = DiskCache::new(paths.cache_dir.join("http"));
    let old = mt_core::time::market_today() - chrono::Duration::days(200);
    let key = mt_miso::day_store_key(Market::DayAhead, old);
    let report = DayLmpReport::new(DayReportKind::DaExPost, old, Vec::new());
    cache.put(&key, &report.to_bytes()).unwrap();
    let live = FetchCtx::new(
        Arc::new(LiveLooking {
            fixtures: FixtureTransport::new(fixtures()),
            writes: parking_lot::Mutex::default(),
        }),
        Some(cache.clone()),
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let hub = DataHub::new(rt.handle().clone(), live);
    hub.set_paused(true);
    let deps = Deps {
        hub,
        // What a config.toml that does not parse becomes.
        config: AppConfig::default(),
        config_error: Some("config.toml: expected a value".into()),
        paths: paths.clone(),
        reset_layout: true,
        startup_commands: Vec::new(),
        remote: None,
        notifier: None,
    };
    let ctx = egui::Context::default();
    let mut app = TerminalApp::headless(&ctx, deps);
    app.apply_commands(&ctx, vec![AppCommand::SetPaused(true)]);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(app.backfill().status().phase, Phase::Starting);
    assert!(
        cache.get(&key).is_some(),
        "the default window removed nothing"
    );

    // Once the config is saved (here from SET), its window applies.
    app.apply_commands(&ctx, vec![AppCommand::ReplaceConfig(Box::default())]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.backfill().status().phase != Phase::Off {
        assert!(Instant::now() < deadline, "{:?}", app.backfill().status());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(cache.get(&key).is_none(), "now removed");
    let _ = std::fs::remove_dir_all(paths.config_file.parent().unwrap());
}

#[test]
fn headlines_and_read_marks_survive_a_restart() {
    use mt_core::news::Headline;
    use mt_data::{DiskCache, Query};

    let rt = runtime();
    let paths = temp_paths("news");
    let cache = DiskCache::new(paths.cache_dir.join("http"));
    let feed = mt_news::builtin_feeds().remove(0);
    let now = mt_core::time::now_utc();
    let headline = |id: &str, days_ago: i64| Headline {
        id: id.into(),
        source: feed.source.clone(),
        title: format!("Story {id}"),
        summary: String::new(),
        link: format!("https://www.ft.com/content/{id}"),
        author: None,
        published: Some(now - chrono::Duration::days(days_ago)),
        seen: now,
        sections: vec![feed.section.clone()],
    };
    let archived = mt_news::FeedHeadlines {
        items: vec![headline("new", 1), headline("expired", 60)],
        ttl_minutes: Some(15),
        fetched: Some(now - chrono::Duration::hours(2)),
        latest: 2,
    };
    cache
        .put(&mt_news::archive_key(&feed.id), &archived.to_bytes())
        .unwrap();
    let ctx = FetchCtx::new(
        Arc::new(FixtureTransport::new(fixtures())),
        Some(cache.clone()),
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let hub = DataHub::new(rt.handle().clone(), ctx);
    hub.set_paused(true); // nothing may come from the network in this test
    let deps = |hub: &DataHub| Deps {
        hub: hub.clone(),
        config: AppConfig::default(),
        config_error: None,
        paths: paths.clone(),
        reset_layout: true,
        startup_commands: Vec::new(),
        remote: None,
        notifier: None,
    };
    let ctx = egui::Context::default();
    let mut app = TerminalApp::headless(&ctx, deps(&hub));
    let query = mt_news::FeedQuery::new(feed.clone(), AppConfig::default().news.keep_days);
    let snap = hub.peek(&query);
    let ids: Vec<&str> = snap
        .data()
        .expect("restored before any fetch")
        .items
        .iter()
        .map(|h| h.id.as_str())
        .collect();
    assert_eq!(ids, ["new"], "expired headlines are not restored");
    assert!(query.key().starts_with("news/"));

    app.apply_commands(
        &ctx,
        vec![
            AppCommand::OpenHeadline {
                id: "new".into(),
                link: "https://www.ft.com/content/new".into(),
            },
            AppCommand::OpenHeadline {
                id: "evil".into(),
                link: "file:///C:/Windows/notepad.exe".into(),
            },
        ],
    );
    let (msg, error) = app.last_feedback().cloned().unwrap_or_default();
    assert!(error && msg.contains("not a web link"), "{msg}");
    assert!(app.news_read.is_read("new") && !app.news_read.is_read("evil"));
    // Marks are written by a background thread.
    let deadline = Instant::now() + Duration::from_secs(5);
    while cache.get(mt_news::READ_KEY).is_none() {
        assert!(Instant::now() < deadline, "read marks were not saved");
        std::thread::sleep(Duration::from_millis(20));
    }
    let again = TerminalApp::headless(&egui::Context::default(), deps(&hub));
    assert!(
        again.news_read.is_read("new"),
        "read marks survive a restart"
    );
    let _ = std::fs::remove_dir_all(paths.config_file.parent().unwrap());
}

#[test]
fn the_headline_browser_previews_filters_and_opens() {
    let rt = runtime();
    let h = Harness::new(hub(&rt));
    let skin = Skin::new(mt_theme::builtin().remove(0));
    h.apply(&skin);
    let mut combined = crate::news::Combined::default();
    let feeds = crate::news::queries(&h.config, false);
    let deadline = Instant::now() + Duration::from_secs(20);
    while combined.health().loaded < feeds.len() {
        assert!(Instant::now() < deadline, "{:?}", combined.health());
        combined.watch(&h.hub, &feeds);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(combined.health().failing.is_empty());
    let items = combined.items().clone();
    assert!(items.len() > 50, "{} headlines", items.len());
    let ids: std::collections::HashSet<&str> = items.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids.len(), items.len(), "combined without duplicates");
    let mut browser = crate::news::Browser::new("test");
    browser.select(&items[0].id);
    browser.source = Some(mt_news::config::BLOOMBERG.into());
    browser.query = "the".into();
    let commands = h.draw_with(&skin, |ui, cx| {
        browser.ui(ui, cx, &items, None, None);
    });
    assert!(commands.is_empty(), "drawing opens nothing: {commands:?}");
    // A topic narrows the list; an unknown word as a topic still draws.
    let energy = h.config.news.topic("energy").unwrap().matcher();
    let odd = mt_core::news::Matcher::from_list("zzzz");
    for (name, m) in [("ENERGY", &energy), ("ZZZZ", &odd)] {
        h.draw_with(&skin, |ui, cx| {
            browser.ui(ui, cx, &items, Some((name, m)), Some(5));
        });
    }
}

#[test]
fn double_clicking_a_tab_zooms_it() {
    let rt = runtime();
    let ctx = egui::Context::default();
    let deps = Deps {
        hub: hub(&rt),
        config: AppConfig::default(),
        config_error: None,
        paths: temp_paths("zoom"),
        reset_layout: true,
        startup_commands: Vec::new(),
        remote: None,
        notifier: None,
    };
    let mut app = TerminalApp::headless(&ctx, deps);
    let mut frame = eframe::Frame::_new_kittest();
    let mut time = 0.0;
    let mut run = |app: &mut TerminalApp, events: Vec<egui::Event>| {
        time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 960.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        finish_frame(&ctx, ctx.run_ui(input, |ui| app.ui(ui, &mut frame)));
    };
    for _ in 0..3 {
        run(&mut app, Vec::new());
    }
    // The first tab title (HOME) sits just under the top bar, at the left
    // (about x 4-83, y 64-88 with the built-in fonts at 1600 x 960).
    let tab = egui::pos2(40.0, 76.0);
    let button = |pressed| egui::Event::PointerButton {
        pos: tab,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    run(&mut app, vec![egui::Event::PointerMoved(tab)]);
    for _ in 0..2 {
        run(&mut app, vec![button(true)]);
        run(&mut app, vec![button(false)]);
    }
    run(&mut app, Vec::new());
    assert!(
        app.workspace_mut().is_zoomed(),
        "double-click zooms the tab"
    );
    run(
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    run(&mut app, Vec::new());
    assert!(
        !app.workspace_mut().is_zoomed(),
        "Esc goes back to the layout"
    );
}

/// A toy stream for LOG: `ok` accepts the login, `name=value` lines set values.
#[derive(Clone)]
struct ToyStream {
    key: &'static str,
    /// A secret the stream needs; missing ones are refused.
    needs: &'static str,
}

impl mt_data::Stream for ToyStream {
    type State = std::collections::BTreeMap<String, String>;
    fn key(&self) -> String {
        self.key.into()
    }
    fn label(&self) -> String {
        format!("Toy {}", self.key)
    }
    fn request(&self, ctx: &FetchCtx) -> Result<mt_data::Request, mt_data::FetchError> {
        Ok(mt_data::Request::get("wss://stream.example.com/v1/ticks")
            .secret_header("X-Key", ctx.secret(self.needs)?))
    }
    fn waits_for_ready(&self) -> bool {
        true
    }
    fn subscribe(&self, topics: &[String]) -> Vec<String> {
        vec![format!("sub {}", topics.join(","))]
    }
    fn unsubscribe(&self, topics: &[String]) -> Vec<String> {
        vec![format!("unsub {}", topics.join(","))]
    }
    fn apply(
        &self,
        state: &mut Self::State,
        frame: &mt_data::Frame,
    ) -> Result<mt_data::Applied, mt_data::FetchError> {
        Ok(match frame.as_text().and_then(|t| t.split_once('=')) {
            Some((k, v)) => {
                state.insert(k.into(), v.into());
                mt_data::Applied::Changed
            }
            None if frame.as_text() == Some("ok") => mt_data::Applied::Ready,
            None => mt_data::Applied::Ignored,
        })
    }
}

#[test]
fn log_and_set_show_streams_budgets_and_keys() {
    let rt = runtime();
    // A recorded stream session and a budgeted host.
    let root = std::env::temp_dir().join(format!("mt-smoke-streams-{}", std::process::id()));
    let dir = root.join("stream.example.com").join("v1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ticks.jsonl"), "ok\nA=1\nB=2\n").unwrap();
    let secrets =
        mt_data::MemorySecrets::with([("alpaca/paper/key-id", mt_data::Secret::new("PKTEST"))]);
    let ctx = FetchCtx::new(
        Arc::new(FixtureTransport::new(&root)),
        None,
        FetchCtxOptions {
            budgets: vec![mt_data::Budget::new(
                "Example",
                ["api.example.com"],
                2,
                Duration::from_secs(60),
            )],
            secrets: Arc::new(secrets),
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let h = Harness::new(DataHub::new(rt.handle().clone(), ctx));
    for _ in 0..2 {
        let _ = rt.block_on(h.hub.ctx().send("https://api.example.com/v2/clock"));
    }
    let live = ToyStream {
        key: "toy/stream/live",
        needs: "alpaca/paper/key-id",
    };
    let refused = ToyStream {
        key: "toy/stream/refused",
        needs: "alpaca/paper/secret-key",
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snap = h.hub.watch_stream(&live, &["A", "B"]);
        h.hub.watch_stream(&refused, &["A"]);
        let states: Vec<_> = h.hub.stream_status().iter().map(|s| s.phase).collect();
        if snap.data().is_some_and(|s| s.len() == 2)
            && states.contains(&mt_data::StreamPhase::Failed)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "streams did not settle: {states:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(h.hub.ctx().budget_status()[0].used, 2);
    let registry = Registry::builtin();
    let draw_all = |h: &Harness| {
        for theme in mt_theme::builtin() {
            let skin = Skin::new(theme.clone());
            h.apply(&skin);
            for code in ["LOG", "SET"] {
                let mut panel = registry.open(&Route::code(code)).unwrap();
                h.draw(&skin, panel.as_mut());
            }
        }
    };
    draw_all(&h);

    // The price history as a live terminal shows it: today's DA stored, then
    // a pass in which MISO has published nothing (every report is a 404 here).
    let mut h = h;
    let cache = mt_data::DiskCache::new(root.join("cache"));
    let today = mt_core::time::market_today();
    let report = mt_core::DayLmpReport::new(mt_core::DayReportKind::DaExPost, today, Vec::new());
    cache
        .put(
            &mt_miso::day_store_key(mt_core::Market::DayAhead, today),
            &report.to_bytes(),
        )
        .unwrap();
    let live = FetchCtx::new(
        Arc::new(LiveLooking {
            fixtures: FixtureTransport::new(&root),
            writes: parking_lot::Mutex::default(),
        }),
        Some(cache),
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    h.backfill = mt_miso::Backfill::with_timing(live, rt.handle(), Duration::ZERO, Duration::ZERO);
    let wait = |h: &Harness, phase: mt_miso::history::Phase| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while h.backfill.status().phase != phase {
            assert!(Instant::now() < deadline, "{:?}", h.backfill.status());
            std::thread::sleep(Duration::from_millis(10));
        }
        h.backfill.status()
    };
    let mut history = h.config.price_history.clone();
    let endpoints = mt_miso::MisoEndpoints::default();
    h.backfill.configure(&endpoints, &history, false);
    let off = wait(&h, mt_miso::history::Phase::Off);
    assert_eq!(off.coverage.da_stored, 1);
    assert_eq!(off.coverage.missing(), 92 * 2 - 2);
    draw_all(&h);
    history.backfill = true;
    h.backfill.configure(&endpoints, &history, false);
    let done = wait(&h, mt_miso::history::Phase::UpToDate);
    assert_eq!(done.not_published.len(), 92 * 2 - 2);
    draw_all(&h);
    let _ = std::fs::remove_dir_all(root);
}

/// The fixtures, posing as the live network (so the order desk would send if
/// anything asked it to), refusing and recording anything but a GET.
struct LiveLooking {
    fixtures: FixtureTransport,
    writes: parking_lot::Mutex<Vec<String>>,
}

impl mt_data::Transport for LiveLooking {
    fn send<'a>(
        &'a self,
        req: &'a mt_data::Request,
    ) -> mt_data::BoxFuture<'a, Result<mt_data::Response, mt_data::FetchError>> {
        if req.method == mt_data::Method::Get {
            return self.fixtures.send(req);
        }
        self.writes.lock().push(req.describe());
        Box::pin(async {
            Ok(mt_data::Response {
                status: 403,
                headers: Vec::new(),
                body: Vec::new().into(),
            })
        })
    }

    fn connect<'a>(
        &'a self,
        req: &'a mt_data::Request,
    ) -> mt_data::BoxFuture<'a, Result<Box<dyn mt_data::StreamConn>, mt_data::FetchError>> {
        self.fixtures.connect(req)
    }

    fn describe(&self) -> String {
        "fixtures posing as live".into()
    }
}

#[test]
fn commands_from_outside_the_window_only_open_tickets() {
    use mt_data::{MemorySecrets, Secret, SecretStore};

    let rt = runtime();
    let transport = Arc::new(LiveLooking {
        fixtures: FixtureTransport::new(fixtures()),
        writes: parking_lot::Mutex::default(),
    });
    let secrets = Arc::new(MemorySecrets::default());
    secrets
        .set(mt_alpaca::KEY_ID, &Secret::new("PKTEST"))
        .unwrap();
    secrets
        .set(mt_alpaca::SECRET_KEY, &Secret::new("SKTEST"))
        .unwrap();
    let ctx = FetchCtx::new(
        transport.clone(),
        None,
        FetchCtxOptions {
            secrets,
            polite_interval: Duration::ZERO,
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let mut config = AppConfig::default();
    config
        .ui
        .hotkeys
        .insert("F11".into(), "SELL XEL US 10 MKT DAY".into());
    let (remote, inbox) = crate::remote::channel_pair();
    let deps = Deps {
        hub: DataHub::new(rt.handle().clone(), ctx),
        config,
        config_error: None,
        paths: temp_paths("orders"),
        reset_layout: true,
        // As a desktop shortcut would (`--run`).
        startup_commands: vec![
            "BUY XLU US 10 LMT 44.50 DAY".into(),
            "ORD KILL".into(),
            "SELL XLU261218C00045000 2 LMT 1.60".into(),
            "MLEG +XLU261218C00045000 -XLU261218C00047000 1 LMT 0.85".into(),
        ],
        remote: Some(inbox),
        notifier: None,
    };
    let egui_ctx = egui::Context::default();
    let mut app = TerminalApp::headless(&egui_ctx, deps);
    assert!(
        app.desk().can_send().is_ok(),
        "the desk would send if asked"
    );
    let mut frame = eframe::Frame::_new_kittest();
    let mut run = |app: &mut TerminalApp, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 960.0),
            )),
            events,
            ..Default::default()
        };
        finish_frame(
            &egui_ctx,
            egui_ctx.run_ui(input, |ui| app.ui(ui, &mut frame)),
        );
    };
    for _ in 0..3 {
        run(&mut app, Vec::new());
    }
    // Another launch forwards its commands; a hotkey runs one.
    assert!(remote.send(vec![
        "SELL XLU US 5 MKT".into(),
        "BUY AEE US 1".into(),
        "BUY XLU261218C00046000 1 MKT".into(),
    ]));
    let f11 = egui::Event::Key {
        key: egui::Key::F11,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    run(&mut app, vec![f11]);
    // Let the account, prices and orders load, as they would for a user.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(app.account_loaded() && app.hub_settled()) {
        assert!(Instant::now() < deadline, "the account never loaded");
        std::thread::sleep(Duration::from_millis(50));
        run(&mut app, Vec::new());
    }
    for _ in 0..3 {
        run(&mut app, Vec::new());
    }
    let routes = app.workspace_mut().routes();
    for want in [
        "BUY XLU US",
        "SELL XLU US 5",
        "BUY AEE US 1",
        "SELL XEL US 10",
        "SELL XLU261218C00045000 2",
        "BUY XLU261218C00046000 1",
        "MLEG +XLU261218C00045000 -XLU261218C00047000 1",
        "ORD",
    ] {
        assert!(
            routes.iter().any(|r| r.to_string().starts_with(want)),
            "{want} did not open: {routes:?}"
        );
    }
    assert!(
        transport.writes.lock().is_empty(),
        "something was sent: {:?}",
        transport.writes.lock()
    );
    assert!(app.desk().audit().recent().is_empty());
    // And a restart drops the tickets rather than restoring them.
    let mut ws = crate::workspace::Workspace::default_layout();
    let registry = Registry::builtin();
    ws.open(Route::new("BUY", ["XLU US", "10"]), &registry);
    ws.close_codes(&["BUY", "SELL"]);
    assert!(!ws.routes().iter().any(|r| r.code == "BUY"));
}
