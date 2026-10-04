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
    out
}

struct Harness {
    ctx: egui::Context,
    hub: DataHub,
    miso: Miso,
    nws: mt_nws::Nws,
    eia: mt_eia::Eia,
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
            eia: mt_eia::Eia::default(),
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
                    eia: &self.eia,
                    skin,
                    config: &self.config,
                    paths: &self.paths,
                    registry: &self.registry,
                    themes: &self.themes,
                    notices: &[],
                    alerts: &self.alerts,
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
    // Securities are recognised, and refused by functions that take nodes.
    for cmd in ["XLU US GP 30", "xlu us", "GP XLU261218C00082500"] {
        app.apply_commands(&ctx, vec![AppCommand::Run(cmd.into())]);
        let (msg, error) = app.last_feedback().cloned().unwrap_or_default();
        assert!(error && msg.contains("is a security"), "{cmd}: {msg}");
    }
    assert_eq!(app.workspace_mut().routes().len(), before + 1);
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
    for theme in mt_theme::builtin() {
        let skin = Skin::new(theme.clone());
        h.apply(&skin);
        for code in ["LOG", "SET"] {
            let mut panel = registry.open(&Route::code(code)).unwrap();
            h.draw(&skin, panel.as_mut());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
