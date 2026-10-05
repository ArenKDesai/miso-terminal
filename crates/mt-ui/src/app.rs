//! The application shell: command line and hub ticker on top, the dock in the
//! middle, a status bar at the bottom. Everything else is a function panel.

use std::time::{Duration, Instant};

use egui::{Align, Frame, Key, Layout, Margin, RichText, Stroke, Ui};
use egui_dock::DockArea;
use mt_core::instrument::Instrument;
use mt_core::time::{MARKET_TZ_LABEL, now_market};
use mt_core::{TRADING_HUBS, hub_short};
use mt_data::{DataHub, EntryState};
use mt_miso::Miso;
use mt_theme::{ThemeRegistry, dir_fingerprint};

use crate::alerts::{AlertData, AlertEngine, AlertRule, Feeds};
use crate::command::{self, Parsed, Suggestion};
use crate::config::{AppConfig, AppPaths};
use crate::context::{AppCommand, PanelCx};
use crate::fonts::FontLibrary;
use crate::function::{Registry, Route};
use crate::skin::{Skin, label_style};
use crate::widgets::{self, fmt};
use crate::workspace::{Viewer, Workspace};

const WORKSPACE_KEY: &str = "workspace";
/// Order tickets: closed rather than restored at launch.
const TICKET_CODES: &[&str] = &["BUY", "SELL"];
const THEME_POLL: Duration = Duration::from_secs(2);
const GC_EVERY: Duration = Duration::from_secs(60);
const GC_IDLE: Duration = Duration::from_secs(15 * 60);
const INTRADAY_SAVE_EVERY: Duration = Duration::from_secs(5 * 60);
/// Space kept free on the right of the top bar for Functions / Theme / Reset layout.
const MENU_BUTTONS_WIDTH: f32 = 290.0;

/// What the binary hands the UI.
pub struct Deps {
    pub hub: DataHub,
    pub config: AppConfig,
    /// A problem loading the config file, shown in LOG.
    pub config_error: Option<String>,
    pub paths: AppPaths,
    /// Ignore the saved layout (`--reset-layout`).
    pub reset_layout: bool,
    /// Command-line commands to run on the first frame (`--run`).
    pub startup_commands: Vec<String>,
    /// Commands forwarded by later launches of the app.
    pub remote: Option<crate::remote::RemoteInbox>,
    /// System notifications (Windows toasts), where the platform has them.
    pub notifier: Option<std::sync::Arc<dyn crate::notify::Notifier>>,
}

#[derive(Default)]
struct CommandLine {
    text: String,
    selected: usize,
    focus: bool,
    feedback: Option<(String, bool)>,
    history: Vec<String>,
    history_pos: Option<usize>,
    /// Suggestions for the text, and what they were computed from (the
    /// text, the number of known nodes, the asset list's generation).
    suggested: Option<((String, usize, u64), Vec<Suggestion>)>,
}

pub struct TerminalApp {
    hub: DataHub,
    miso: Miso,
    nws: mt_nws::Nws,
    eia: mt_eia::Eia,
    alpaca: mt_alpaca::Alpaca,
    config: AppConfig,
    pub(crate) paths: AppPaths,
    registry: Registry,
    themes: ThemeRegistry,
    skin: Skin,
    fonts: FontLibrary,
    notices: Vec<String>,
    pub(crate) alerts: AlertEngine,
    workspace: Workspace,
    cmd: CommandLine,
    themes_fingerprint: u64,
    last_theme_poll: Instant,
    last_gc: Instant,
    pending: Vec<AppCommand>,
    /// A capture to request at the start of the next frame, once the menu that
    /// asked for it has closed.
    capture_next: Option<crate::capture::Request>,
    last_intraday_save: Instant,
    /// Generation of the intraday store last written to disk.
    intraday_saved: u64,
    remote: Option<crate::remote::RemoteInbox>,
    notifier: Option<std::sync::Arc<dyn crate::notify::Notifier>>,
    /// Which headlines have been opened (saved in the cache).
    pub(crate) news_read: mt_news::ReadMarks,
    /// Every feed's headlines, for headline alerts.
    alert_news: crate::news::Combined,
    /// The order-event stream's last token, for the early account re-sync.
    trade_sync: Option<(u64, u64)>,
    /// Places, replaces and cancels orders (only from clicks in a ticket or ORD).
    desk: mt_alpaca::OrderDesk,
    /// The desk's generation last seen: when it moves, the account is re-read.
    desk_seen: u64,
}

impl TerminalApp {
    pub fn new(cc: &eframe::CreationContext<'_>, deps: Deps) -> Self {
        let ctx = cc.egui_ctx.clone();
        deps.hub.set_notify(move || ctx.request_repaint());
        let saved = if deps.reset_layout {
            None
        } else {
            cc.storage
                .and_then(|s| eframe::get_value::<Workspace>(s, WORKSPACE_KEY))
        };
        let app = Self::build(&cc.egui_ctx, deps, saved);
        cc.egui_ctx
            .set_zoom_factor(app.config.ui.zoom.clamp(0.5, 3.0));
        app
    }

    /// Build without a window or saved layout: tests and tools.
    pub fn headless(ctx: &egui::Context, deps: Deps) -> Self {
        Self::build(ctx, deps, None)
    }

    fn build(ctx: &egui::Context, deps: Deps, saved: Option<Workspace>) -> Self {
        let themes = ThemeRegistry::load(Some(&deps.paths.themes_dir));
        let alpaca = mt_alpaca::Alpaca::new(&deps.config.markets, alpaca_ready(&deps.hub));
        let desk = mt_alpaca::OrderDesk::new(
            &alpaca,
            deps.hub.ctx().clone(),
            deps.hub.runtime().clone(),
            mt_alpaca::AuditLog::to_dir(&deps.paths.audit_dir),
        );
        let repaint = ctx.clone();
        desk.set_notify(move || repaint.request_repaint());
        let mut app = Self {
            miso: Miso::new(deps.config.endpoints.clone()),
            nws: mt_nws::Nws::default(),
            eia: mt_eia::Eia::default(),
            alpaca,
            hub: deps.hub,
            registry: Registry::builtin(),
            skin: Skin::new(themes.resolve(&deps.config.theme).clone()),
            themes_fingerprint: dir_fingerprint(&deps.paths.themes_dir),
            themes,
            fonts: FontLibrary::new(Some(deps.paths.fonts_dir.clone())),
            notices: deps.config_error.into_iter().collect(),
            workspace: {
                let mut ws = Workspace::restore(saved);
                // A ticket from the last session must be typed again.
                ws.close_codes(TICKET_CODES);
                ws
            },
            cmd: CommandLine {
                focus: true,
                ..Default::default()
            },
            last_theme_poll: Instant::now(),
            last_gc: Instant::now(),
            pending: deps
                .startup_commands
                .into_iter()
                .map(AppCommand::Run)
                .collect(),
            alerts: AlertEngine::default(),
            capture_next: None,
            last_intraday_save: Instant::now(),
            intraday_saved: 0,
            remote: deps.remote,
            notifier: deps.notifier,
            news_read: mt_news::ReadMarks::default(),
            alert_news: crate::news::Combined::default(),
            trade_sync: None,
            desk,
            desk_seen: 0,
            config: deps.config,
            paths: deps.paths,
        };
        app.apply_theme(ctx);
        app.restore_intraday();
        app.restore_news();
        if let Some(inbox) = &app.remote {
            inbox.attach(ctx);
        }
        app
    }

    /// Show today's five-minute prices from the last session straight away
    /// (marked stale) instead of waiting for the rolling feed, which can take
    /// most of a minute late in the day.
    fn restore_intraday(&self) {
        let Some(cache) = self.hub.ctx().cache() else {
            return;
        };
        let today = mt_core::time::market_today();
        let Some(store) = cache
            .get(&intraday_key(today))
            .and_then(|b| mt_core::RtIntraday::from_bytes(&b))
        else {
            return;
        };
        let Some(latest) = store
            .latest_interval()
            .filter(|_| store.market_day == Some(today))
        else {
            return;
        };
        let as_of = mt_core::time::market_to_utc(latest + chrono::Duration::minutes(5));
        tracing::info!(
            "restored {} five-minute intervals from the last session",
            store.intervals().len()
        );
        self.hub.seed_stale(&self.miso.rt_intraday(), store, as_of);
    }

    /// Headlines and read marks from the last session, without those past
    /// their keep window. The headlines show at once (marked stale) and the
    /// feeds refresh as soon as something watches them.
    fn restore_news(&mut self) {
        let Some(cache) = self.hub.ctx().cache() else {
            return;
        };
        let now = mt_core::time::now_utc();
        let keep_days = self.config.news.keep_days;
        let mut restored = 0;
        for q in crate::news::queries(&self.config, false) {
            let Some(mut feed) = mt_news::load_archive(cache, &q.feed().id) else {
                continue;
            };
            feed.items = mt_news::merge(&feed.items, Vec::new(), keep_days, now);
            restored += feed.items.len();
            let as_of = feed.fetched.unwrap_or(chrono::DateTime::UNIX_EPOCH);
            self.hub.seed_stale(&q, feed, as_of);
        }
        if restored > 0 {
            tracing::info!("restored {restored} headlines from the last session");
        }
        if let Some(mut marks) = cache
            .get(mt_news::READ_KEY)
            .and_then(|b| mt_news::ReadMarks::from_bytes(&b))
        {
            marks.prune(keep_days, now);
            self.news_read = marks;
        }
    }

    fn mark_read(&mut self, ids: &[String], read: bool) {
        let now = mt_core::time::now_utc();
        let mut changed = false;
        for id in ids {
            changed |= self.news_read.set(id, read, now);
        }
        if let (true, Some(cache)) = (changed, self.hub.ctx().cache().cloned()) {
            let bytes = self.news_read.to_bytes();
            std::thread::spawn(move || {
                if let Err(e) = cache.put(mt_news::READ_KEY, &bytes) {
                    tracing::warn!("could not save read headlines: {e}");
                }
            });
        }
    }

    /// Write today's five-minute store to the cache if it changed since the
    /// last save. `background` hands the work to a thread.
    fn save_intraday(&mut self, background: bool) {
        let Some(cache) = self.hub.ctx().cache().cloned() else {
            return;
        };
        let snap = self.hub.peek(&self.miso.rt_intraday());
        let (Some(store), true) = (snap.data, snap.generation != self.intraday_saved) else {
            return;
        };
        let Some(day) = store.market_day else { return };
        self.intraday_saved = snap.generation;
        let write = move || {
            if let Err(e) = cache.put(&intraday_key(day), &store.to_bytes()) {
                tracing::warn!("could not save intraday prices: {e}");
            }
        };
        if background {
            std::thread::spawn(write);
        } else {
            write();
        }
    }

    pub fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    /// The theme id to show now: the configured one, or the light/dark pair
    /// when following the OS setting (and the OS has told us which it is).
    fn wanted_theme(&self, ctx: &egui::Context) -> String {
        let ui = &self.config.ui;
        match (ui.follow_system_theme, ctx.system_theme()) {
            (true, Some(egui::Theme::Dark)) => ui.dark_theme.clone(),
            (true, Some(egui::Theme::Light)) => ui.light_theme.clone(),
            _ => self.config.theme.clone(),
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let wanted = self.wanted_theme(ctx);
        let theme = self.themes.resolve(&wanted).clone();
        if !theme.meta.id.eq_ignore_ascii_case(&wanted) {
            // Most likely a theme that used to be built in (Everforge, Amber Terminal).
            self.notices.push(format!(
                "Theme {wanted:?} is not installed; using {}. Install more themes from \
                 the gallery in THEME.",
                theme.meta.name
            ));
        }
        self.skin = Skin::new(theme);
        let (defs, warnings) = self.fonts.definitions(&self.skin.theme.fonts);
        self.notices.extend(warnings);
        ctx.set_fonts(defs);
        ctx.set_theme(if self.skin.theme.meta.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        let skin = &self.skin;
        ctx.all_styles_mut(|style| skin.apply_style(style));
    }

    #[cfg(test)]
    pub(crate) fn active_theme_id(&self) -> &str {
        &self.skin.theme.meta.id
    }

    #[cfg(test)]
    pub(crate) fn config(&self) -> &AppConfig {
        &self.config
    }

    #[cfg(test)]
    pub(crate) fn desk(&self) -> &mt_alpaca::OrderDesk {
        &self.desk
    }

    /// Whether everything asked for has loaded (or failed).
    #[cfg(test)]
    pub(crate) fn hub_settled(&self) -> bool {
        self.hub.in_flight() == 0
            && !self
                .hub
                .status()
                .iter()
                .any(|s| s.state == EntryState::Empty)
    }

    #[cfg(test)]
    pub(crate) fn account_loaded(&self) -> bool {
        self.hub.peek(&self.alpaca.account()).data.is_some()
            && self.hub.peek(&self.alpaca.orders()).data.is_some()
    }

    /// Re-read the themes folder now (after an install), not at the next poll.
    fn reload_themes(&mut self, ctx: &egui::Context) {
        self.themes_fingerprint = dir_fingerprint(&self.paths.themes_dir);
        self.themes.reload();
        self.apply_theme(ctx);
    }

    fn set_theme(&mut self, ctx: &egui::Context, id: &str) {
        let Some(theme) = self.themes.get(id) else {
            self.feedback(format!("No theme {id:?}. Try THEME."), true);
            return;
        };
        self.config.theme = theme.meta.id.clone();
        // Picking a theme by hand means "this one", not "whatever the OS says".
        self.config.ui.follow_system_theme = false;
        self.apply_theme(ctx);
        self.save_config();
    }

    /// Swap in a whole configuration, rebuild what depends on it, and save.
    fn replace_config(&mut self, ctx: &egui::Context, config: AppConfig) {
        let endpoints_changed = config.endpoints != self.config.endpoints;
        let markets_changed = config.markets != self.config.markets;
        let theme_changed = config.theme != self.config.theme
            || config.ui.follow_system_theme != self.config.ui.follow_system_theme
            || config.ui.light_theme != self.config.ui.light_theme
            || config.ui.dark_theme != self.config.ui.dark_theme;
        self.config = config;
        if markets_changed {
            // A new feed means new queries and a new stream.
            self.alpaca = mt_alpaca::Alpaca::new(&self.config.markets, self.alpaca.is_ready());
        }
        ctx.set_zoom_factor(self.config.ui.zoom.clamp(0.5, 3.0));
        if endpoints_changed {
            // Cached values stay until their next refresh, which uses the new URLs.
            self.miso = Miso::new(self.config.endpoints.clone());
        }
        if theme_changed {
            self.apply_theme(ctx);
        }
        self.save_config();
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save(&self.paths.config_file) {
            self.notices.push(format!("Could not save config: {e}"));
        }
    }

    fn feedback(&mut self, msg: impl Into<String>, error: bool) {
        self.cmd.feedback = Some((msg.into(), error));
    }

    /// The command line's last message and whether it was an error.
    #[cfg(test)]
    pub(crate) fn last_feedback(&self) -> Option<&(String, bool)> {
        self.cmd.feedback.as_ref()
    }

    /// Every pricing node we know about, favourites first, for completion.
    fn known_nodes(&self) -> Vec<String> {
        let mut nodes: Vec<String> = self.config.ui.favorite_nodes.clone();
        if let Some(b) = self.hub.peek(&self.miso.lmp_board()).data() {
            nodes.extend(b.rows.iter().map(|r| r.node.clone()));
        }
        if let Some(d) = self.hub.peek(&self.miso.rt_intraday()).data() {
            nodes.extend(d.node_names().iter().cloned());
        }
        let mut seen = std::collections::HashSet::new();
        nodes.retain(|n| seen.insert(n.clone()));
        nodes
    }

    /// Completions for the command line, recomputed only when the text, the
    /// known nodes or the asset list change.
    fn suggestions(&mut self) -> Vec<Suggestion> {
        let nodes = self.known_nodes();
        let assets = if self.alpaca.is_ready() {
            self.hub.peek(&self.alpaca.assets())
        } else {
            mt_data::Snapshot::default()
        };
        let key = (self.cmd.text.clone(), nodes.len(), assets.generation);
        if let Some((k, s)) = &self.cmd.suggested
            && *k == key
        {
            return s.clone();
        }
        let list = assets.data.unwrap_or_default();
        let securities = |text: &str, limit: usize| crate::market::completions(&list, text, limit);
        let s = command::suggest(&self.cmd.text, &self.registry, &nodes, &securities, 10);
        self.cmd.suggested = Some((key, s.clone()));
        s
    }

    fn run_command(&mut self, ctx: &egui::Context, text: &str) {
        let nodes = self.known_nodes();
        match command::parse(text, &self.registry, |n| nodes.iter().any(|k| k == n)) {
            Parsed::Empty => {}
            Parsed::Route(route) => {
                if route.code == "THEME"
                    && let Some(id) = route.arg(0)
                {
                    self.set_theme(ctx, id);
                    return;
                }
                if let Some(Instrument::Option(o)) = command::security_arg(&route) {
                    self.feedback(
                        format!("{o} is an option. Options arrive with OMON (markets phase 5)."),
                        true,
                    );
                    return;
                }
                if let Some(spec) = self.registry.find(&route.code)
                    && !spec.takes_security
                    && let Some(sec) = command::security_arg(&route)
                {
                    self.feedback(
                        format!("{} takes MISO nodes; {sec} is a security.", spec.code),
                        true,
                    );
                    return;
                }
                self.feedback(route.to_string(), false);
                self.workspace.open(route, &self.registry);
            }
            // A bare security graphs it, as a bare node does.
            Parsed::Instrument(Instrument::Security(sec)) => {
                let route = Route::new("GP", [sec.to_string()]);
                self.feedback(route.to_string(), false);
                self.workspace.open(route, &self.registry);
            }
            Parsed::Instrument(inst) => self.feedback(
                format!("{inst} is an option. Options arrive with OMON (markets phase 5)."),
                true,
            ),
            Parsed::Unknown(s) => self.feedback(
                format!("Unknown command {s:?}. Type HELP for functions."),
                true,
            ),
        }
    }

    pub(crate) fn apply_commands(&mut self, ctx: &egui::Context, commands: Vec<AppCommand>) {
        for c in commands {
            match c {
                AppCommand::Open(route) => self.workspace.open(route, &self.registry),
                AppCommand::Run(text) => self.run_command(ctx, &text),
                AppCommand::SetTheme(id) => self.set_theme(ctx, &id),
                AppCommand::InstallTheme { theme, activate } => {
                    let (id, name) = (theme.theme.meta.id.clone(), theme.theme.meta.name.clone());
                    match mt_theme::gallery::install(&theme, &self.paths.themes_dir) {
                        Ok(_) => {
                            self.reload_themes(ctx);
                            if activate {
                                self.set_theme(ctx, &id);
                            } else {
                                self.feedback(
                                    format!("Installed {name}. THEME {id} switches to it."),
                                    false,
                                );
                            }
                        }
                        Err(e) => self.feedback(format!("Could not install {name}: {e}"), true),
                    }
                }
                AppCommand::UninstallTheme(id) => {
                    match mt_theme::gallery::uninstall(&id, &self.paths.themes_dir) {
                        Ok(path) => {
                            // Removing the theme in use goes back to the default, not to a notice.
                            if self.config.theme.eq_ignore_ascii_case(&id) {
                                self.config.theme = mt_theme::DEFAULT_THEME_ID.into();
                                self.save_config();
                            }
                            self.reload_themes(ctx);
                            self.feedback(format!("Removed {}.", path.display()), false);
                        }
                        Err(e) => self.feedback(
                            format!("Could not remove {id}.toml from the themes folder: {e}"),
                            true,
                        ),
                    }
                }
                AppCommand::AddAlert(rule) => {
                    if !self.config.alerts.contains(&rule) {
                        self.config.alerts.push(rule);
                        self.save_config();
                    }
                }
                AppCommand::RemoveAlert(i) => {
                    if i < self.config.alerts.len() {
                        self.config.alerts.remove(i);
                        self.save_config();
                    }
                }
                AppCommand::AlertsSeen => self.alerts.unseen = 0,
                AppCommand::SetNotifyAlerts(on) => {
                    self.config.ui.notify_alerts = on;
                    self.save_config();
                }
                AppCommand::TestNotification => {
                    if let Some(n) = &self.notifier {
                        n.notify(
                            "MISO Terminal",
                            "Alert notifications are working. This is a test.",
                        );
                    }
                }
                AppCommand::Capture(req) => {
                    self.capture_next = Some(req);
                    ctx.request_repaint();
                }
                AppCommand::SetThemeFollow {
                    follow,
                    light,
                    dark,
                } => {
                    let ui = &mut self.config.ui;
                    (ui.follow_system_theme, ui.light_theme, ui.dark_theme) = (follow, light, dark);
                    self.apply_theme(ctx);
                    self.save_config();
                }
                AppCommand::AddFavorite(item) => {
                    let (list, item) = match crate::market::security_of(&item) {
                        Some(sec) => (&mut self.config.ui.favorite_securities, sec.to_string()),
                        None => (
                            &mut self.config.ui.favorite_nodes,
                            item.trim().to_ascii_uppercase(),
                        ),
                    };
                    if !item.is_empty() && !list.contains(&item) {
                        list.push(item);
                        self.save_config();
                    }
                }
                AppCommand::RemoveFavorite(item) => {
                    match crate::market::security_of(&item) {
                        Some(sec) => {
                            let sec = sec.to_string();
                            self.config.ui.favorite_securities.retain(|s| s != &sec);
                        }
                        None => self.config.ui.favorite_nodes.retain(|n| n != &item),
                    }
                    self.save_config();
                }
                AppCommand::CredentialsChanged => {
                    self.alpaca =
                        mt_alpaca::Alpaca::new(&self.config.markets, alpaca_ready(&self.hub));
                }
                AppCommand::SetTradingEnabled(on) => {
                    if self.config.trading.enabled != on {
                        self.config.trading.enabled = on;
                        self.save_config();
                    }
                    self.feedback(
                        if on {
                            "Trading is on: tickets can send orders again"
                        } else {
                            "Trading is off: tickets cannot send orders (ORD turns it back on)"
                        },
                        !on,
                    );
                }
                AppCommand::ResetLayout => self.workspace = Workspace::default_layout(),
                AppCommand::CloseTab => self.workspace.close_focused(),
                AppCommand::CycleTab(forward) => self.workspace.cycle_focused(forward),
                AppCommand::ToggleZoom => self.workspace.toggle_zoom(),
                AppCommand::PopOut(tab) => self.workspace.pop_out(tab),
                AppCommand::DockBack(tab) => self.workspace.dock_back(tab),
                AppCommand::Zoom(tab) => self.workspace.zoom(tab),
                AppCommand::RefreshWatched => self.hub.refresh_watched(),
                AppCommand::SetPaused(p) => self.hub.set_paused(p),
                AppCommand::ClearCache => {
                    if let Some(cache) = self.hub.ctx().cache()
                        && let Err(e) = cache.clear()
                    {
                        self.notices.push(format!("Could not clear cache: {e}"));
                    }
                }
                AppCommand::RevealPath(path) => reveal(&path),
                AppCommand::OpenHeadline { id, link } => {
                    // Feed content is untrusted: only web links leave the app.
                    if link.starts_with("https://") || link.starts_with("http://") {
                        ctx.open_url(egui::OpenUrl::new_tab(&link));
                        self.mark_read(&[id], true);
                        self.feedback(
                            format!("Opened {} in your browser", mt_data::host_of(&link)),
                            false,
                        );
                    } else {
                        self.feedback(format!("Not opening {link:?}: not a web link"), true);
                    }
                }
                AppCommand::MarkRead { ids, read } => self.mark_read(&ids, read),
                AppCommand::ReplaceConfig(config) => {
                    self.replace_config(ctx, *config);
                    self.feedback("Settings saved", false);
                }
                AppCommand::ResetConfig => match AppConfig::back_up(&self.paths.config_file) {
                    Ok(backup) => {
                        self.replace_config(ctx, AppConfig::default());
                        let kept = backup.map_or_else(String::new, |b| {
                            format!("; the old file is {}", b.display())
                        });
                        self.feedback(format!("Settings reset to defaults{kept}"), false);
                    }
                    Err(e) => {
                        // Never discard someone's settings without a copy.
                        let msg =
                            format!("Settings not reset: could not back up config.toml ({e})");
                        self.notices.push(msg.clone());
                        self.feedback(msg, true);
                    }
                },
            }
        }
    }

    /// Evaluate alert rules against the latest data; flash the taskbar when one fires.
    fn check_alerts(&mut self, ctx: &egui::Context) {
        if self.config.alerts.is_empty() {
            return;
        }
        let board = self.hub.watch(&self.miso.lmp_board());
        let rules = &self.config.alerts;
        let feeds = rules
            .iter()
            .fold(Feeds::default(), |f, r| f.union(r.feeds()));
        // Only pull the all-node feed if a rule watches a node the board lacks.
        let needs_intraday = rules
            .iter()
            .flat_map(AlertRule::nodes)
            .any(|n| board.data().is_none_or(|b| b.row(n).is_none()));
        let intraday = needs_intraday.then(|| self.hub.watch(&self.miso.rt_intraday()));
        let cons = feeds
            .constraints
            .then(|| self.hub.watch(&self.miso.binding_constraints()));
        let transfer = feeds
            .transfer
            .then(|| self.hub.watch(&self.miso.regional_transfer()));
        let load = feeds.load.then(|| self.hub.watch(&self.miso.load()));
        let ace = feeds.ace.then(|| self.hub.watch(&self.miso.ace()));
        let headlines = feeds.news.then(|| {
            self.alert_news
                .watch(&self.hub, &crate::news::queries(&self.config, false));
            self.alert_news.items().clone()
        });
        let price = |node: &str| -> Option<(chrono::NaiveDateTime, f64)> {
            if let Some(b) = board.data()
                && let Some(p) = b.row(node).and_then(|r| r.rt_5min)
            {
                return Some((b.interval?, p.lmp));
            }
            let (t, p) = intraday.as_ref()?.data()?.latest(node)?;
            Some((t, p.lmp))
        };
        let data = AlertData {
            price: &price,
            constraints: cons.as_ref().and_then(|c| c.data()),
            transfer: transfer.as_ref().and_then(|c| c.data()),
            load: load.as_ref().and_then(|c| c.data()),
            ace: ace.as_ref().and_then(|c| c.data()),
            headlines: headlines.as_deref().map(Vec::as_slice),
        };
        let fired = self.alerts.evaluate(rules, &data);
        if let Some(last) = fired.last() {
            for e in &fired {
                tracing::info!("alert: {} ({})", e.rule, e.detail);
            }
            self.feedback(format!("⚠ {}: {}", last.rule, last.detail), false);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
            // In-app feedback is enough while the terminal has focus.
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if let (Some(n), true, false) = (&self.notifier, self.config.ui.notify_alerts, focused)
            {
                for (title, body) in crate::notify::for_alerts(&fired) {
                    n.notify(&title, &body);
                }
            }
        }
    }

    fn background_work(&mut self, ctx: &egui::Context) {
        // The OS light/dark setting can change at any time.
        if self.config.ui.follow_system_theme {
            let wanted = self.wanted_theme(ctx);
            if !wanted.eq_ignore_ascii_case(&self.skin.theme.meta.id)
                && self.themes.get(&wanted).is_some()
            {
                self.apply_theme(ctx);
            }
        }
        if self.last_theme_poll.elapsed() >= THEME_POLL {
            self.last_theme_poll = Instant::now();
            if dir_fingerprint(&self.paths.themes_dir) != self.themes_fingerprint {
                self.reload_themes(ctx);
                tracing::info!("themes folder changed; reloaded");
            }
        }
        // The asset list behind ticker completion (fetched daily, kept on disk).
        if self.alpaca.is_ready() {
            self.hub.watch(&self.alpaca.assets());
            // After an order event or a reconnect, the account at once.
            crate::portfolio::resync(&self.hub, &self.alpaca, &mut self.trade_sync);
            // And after the desk places, replaces or cancels something.
            let generation = self.desk.generation();
            if generation != self.desk_seen {
                self.desk_seen = generation;
                crate::portfolio::refresh_account(&self.hub, &self.alpaca);
            }
        }
        if self.last_intraday_save.elapsed() >= INTRADAY_SAVE_EVERY {
            self.last_intraday_save = Instant::now();
            self.save_intraday(true);
        }
        if self.last_gc.elapsed() >= GC_EVERY {
            self.last_gc = Instant::now();
            let dropped = self.hub.gc(GC_IDLE);
            if dropped > 0 {
                tracing::debug!("dropped {dropped} idle data feeds");
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, commands: &mut Vec<AppCommand>) {
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, Key::K) {
                self.cmd.focus = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                // Esc backs out of a zoomed panel, then to the command line.
                if self.workspace.is_zoomed() {
                    commands.push(AppCommand::Zoom(None));
                }
                self.cmd.focus = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::M) {
                commands.push(AppCommand::ToggleZoom);
            }
            if i.consume_key(egui::Modifiers::NONE, Key::F1) {
                commands.push(AppCommand::Open(Route::code("HELP")));
            }
            for (name, command) in &self.config.ui.hotkeys {
                if let Some(key) = Key::from_name(name)
                    && !matches!(key, Key::F1 | Key::F5)
                    && i.consume_key(egui::Modifiers::NONE, key)
                {
                    commands.push(AppCommand::Run(command.clone()));
                }
            }
            if i.consume_key(egui::Modifiers::NONE, Key::F5) {
                commands.push(AppCommand::RefreshWatched);
            }
            if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L) {
                commands.push(AppCommand::ResetLayout);
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::W) {
                commands.push(AppCommand::CloseTab);
            }
            if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Tab) {
                commands.push(AppCommand::CycleTab(false));
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::Tab) {
                commands.push(AppCommand::CycleTab(true));
            }
        });
    }

    fn top_bar(&mut self, ui: &mut Ui, commands: &mut Vec<AppCommand>) {
        let skin = self.skin.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("MISO").heading().strong().color(skin.accent));
            ui.label(RichText::new("TERMINAL").heading().color(skin.text_muted));
            ui.add_space(8.0);
            self.command_line(ui, &skin, commands);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("Functions", |ui| {
                    for cat in crate::function::Category::ALL {
                        ui.label(
                            RichText::new(cat.label().to_uppercase())
                                .text_style(label_style())
                                .color(skin.text_muted),
                        );
                        for spec in self.registry.specs().iter().filter(|s| s.category == cat) {
                            if ui
                                .button(format!("{:<6} {}", spec.code, spec.name))
                                .clicked()
                            {
                                commands.push(AppCommand::Open(Route::code(spec.code)));
                                ui.close();
                            }
                        }
                    }
                });
                ui.menu_button("Theme", |ui| {
                    for t in self.themes.themes() {
                        if ui
                            .radio(t.meta.id == skin.theme.meta.id, &t.meta.name)
                            .clicked()
                        {
                            commands.push(AppCommand::SetTheme(t.meta.id.clone()));
                            ui.close();
                        }
                    }
                });
                if ui
                    .button("Reset layout")
                    .on_hover_text("Ctrl+Shift+L")
                    .clicked()
                {
                    commands.push(AppCommand::ResetLayout);
                }
            });
        });
        self.ticker(ui, &skin, commands);
    }

    fn command_line(&mut self, ui: &mut Ui, skin: &Skin, commands: &mut Vec<AppCommand>) {
        let suggestions = self.suggestions();
        let (up, down, tab, enter) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Tab),
                i.key_pressed(Key::Enter),
            )
        });
        ui.label(RichText::new("›").monospace().strong().color(skin.live));
        let edit = egui::TextEdit::singleline(&mut self.cmd.text)
            .id_salt("mt-command")
            .font(egui::TextStyle::Monospace)
            .hint_text(
                "type a function, node or security, e.g. LMP, GP MINN.HUB, XLU US — Enter to go",
            )
            .desired_width((ui.available_width() - 330.0).clamp(240.0, 720.0))
            .lock_focus(true);
        let resp = ui.add(edit);
        if std::mem::take(&mut self.cmd.focus) {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if resp.changed() {
            self.cmd.selected = 0;
            self.cmd.history_pos = None;
        }
        if focused && !suggestions.is_empty() {
            if down || tab {
                self.cmd.selected = (self.cmd.selected + 1) % suggestions.len();
            }
            if up {
                self.cmd.selected = self
                    .cmd
                    .selected
                    .checked_sub(1)
                    .unwrap_or(suggestions.len() - 1);
            }
        } else if focused && self.cmd.text.is_empty() && up && !self.cmd.history.is_empty() {
            let pos = self
                .cmd
                .history_pos
                .map_or(self.cmd.history.len() - 1, |p| p.saturating_sub(1));
            self.cmd.history_pos = Some(pos);
            self.cmd.text = self.cmd.history[pos].clone();
        }

        if resp.lost_focus() && enter {
            let text = match suggestions.get(self.cmd.selected) {
                // Enter on an exact code runs what was typed (keeps arguments); otherwise the highlighted suggestion.
                Some(s)
                    if !self.cmd.text.contains(' ')
                        && self.registry.find(self.cmd.text.trim()).is_none() =>
                {
                    s.command.clone()
                }
                _ => self.cmd.text.clone(),
            };
            if !text.trim().is_empty() {
                self.cmd.history.retain(|h| h != &text);
                self.cmd.history.push(text.clone());
                commands.push(AppCommand::Run(text));
            }
            self.cmd.text.clear();
            self.cmd.focus = true;
        }

        if focused && !suggestions.is_empty() {
            let below = resp.rect.left_bottom() + egui::vec2(0.0, 4.0);
            egui::Area::new(egui::Id::new("mt-suggest"))
                .fixed_pos(below)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(resp.rect.width());
                        for (i, s) in suggestions.iter().enumerate() {
                            let selected = i == self.cmd.selected;
                            let text = RichText::new(format!("{:<24} {}", s.command, s.detail))
                                .monospace();
                            if ui.selectable_label(selected, text).clicked() {
                                commands.push(AppCommand::Run(s.command.clone()));
                                self.cmd.text.clear();
                            }
                        }
                    });
                });
        }
        // While typing a known code, show how to use it; otherwise the last result.
        // Bounded so long text truncates instead of running under the menu buttons.
        let typing = focused
            .then(|| command::hint(&self.cmd.text, &self.registry))
            .flatten();
        let room = (ui.available_width() - MENU_BUTTONS_WIDTH).max(0.0);
        let height = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(room, height),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_max_width(room);
                if let Some((usage, description)) = typing {
                    ui.label(RichText::new(usage).monospace().small().color(skin.warning));
                    ui.add(
                        egui::Label::new(RichText::new(description).small().color(skin.text_muted))
                            .truncate(),
                    );
                } else if let Some((msg, error)) = &self.cmd.feedback {
                    let color = if *error {
                        skin.negative
                    } else {
                        skin.text_muted
                    };
                    ui.add(egui::Label::new(RichText::new(msg).small().color(color)).truncate());
                }
            },
        );
    }

    /// The strip of hub prices under the command line.
    fn ticker(&self, ui: &mut Ui, skin: &Skin, commands: &mut Vec<AppCommand>) {
        let board = self.hub.watch(&self.miso.lmp_board());
        let intraday = self.hub.peek(&self.miso.rt_intraday());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let Some(b) = board.data() else {
                ui.label(
                    RichText::new("hub prices loading…")
                        .small()
                        .color(skin.text_muted),
                );
                return;
            };
            for hub in TRADING_HUBS {
                let Some(row) = b.row(hub) else { continue };
                let Some(rt) = row.rt_5min.map(|p| p.lmp) else {
                    continue;
                };
                let prev = intraday.data().and_then(|d| d.previous(hub)).map(|p| p.lmp);
                let (arrow, color) = match prev.map(|p| rt - p) {
                    Some(d) if d > 0.005 => ("▲", skin.positive),
                    Some(d) if d < -0.005 => ("▼", skin.negative),
                    _ => ("■", skin.text_muted),
                };
                let resp = ui
                    .add(
                        egui::Label::new(
                            RichText::new(format!("{} ", hub_short(hub)))
                                .text_style(label_style())
                                .color(skin.text_muted),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                ui.label(
                    RichText::new(fmt::price(rt))
                        .monospace()
                        .color(skin.text_strong),
                );
                ui.label(RichText::new(arrow).small().color(color));
                if let Some(da) = row.da_expost.map(|p| p.lmp) {
                    ui.label(
                        RichText::new(format!("DA {}", fmt::price(da)))
                            .small()
                            .color(skin.text_muted),
                    );
                }
                ui.add_space(10.0);
                if resp.clicked() {
                    commands.push(AppCommand::Open(Route::new("GP", [hub])));
                }
            }
            if let Some(t) = b.interval {
                ui.label(
                    RichText::new(format!("RT {} {MARKET_TZ_LABEL}", fmt::hm(t)))
                        .small()
                        .color(skin.text_muted),
                );
            }
        });
    }

    fn status_bar(&self, ui: &mut Ui, commands: &mut Vec<AppCommand>) {
        let skin = &self.skin;
        let status = self.hub.status();
        let count = |s: EntryState| status.iter().filter(|e| e.watched && e.state == s).count();
        let (errors, stale) = (count(EntryState::Error), count(EntryState::Stale));
        ui.horizontal(|ui| {
            let (lamp, text) = if self.hub.is_paused() {
                (skin.warning, "PAUSED".to_owned())
            } else if !self.hub.ctx().is_live() {
                (skin.info, "OFFLINE REPLAY".to_owned())
            } else if errors > 0 {
                (skin.negative, format!("{errors} feed(s) failing"))
            } else if stale > 0 {
                (skin.warning, format!("{stale} feed(s) stale"))
            } else {
                (skin.live, "LIVE".to_owned())
            };
            widgets::lamp(ui, lamp);
            ui.label(RichText::new(text).text_style(label_style()).color(lamp));
            if self.alerts.unseen > 0 {
                let label = RichText::new(format!("⚠ {} alert(s)", self.alerts.unseen))
                    .text_style(label_style())
                    .color(skin.warning);
                if ui.add(egui::Button::new(label).small()).clicked() {
                    commands.push(AppCommand::Open(Route::code("ALRT")));
                }
            }
            ui.label(
                RichText::new(format!("· {}", self.hub.ctx().transport_description()))
                    .small()
                    .color(skin.text_muted),
            );
            let busy = self.hub.in_flight();
            if busy > 0 {
                ui.spinner();
                ui.label(
                    RichText::new(format!("{busy} fetching"))
                        .small()
                        .color(skin.text_muted),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let market = now_market().format("%a %b %d  %H:%M:%S");
                if self.config.ui.show_local_clock {
                    let local = mt_core::time::now_utc()
                        .with_timezone(&chrono::Local)
                        .format("%H:%M:%S");
                    ui.label(
                        RichText::new(format!("local {local}"))
                            .small()
                            .color(skin.text_muted),
                    );
                }
                ui.label(
                    RichText::new(format!("{market} {MARKET_TZ_LABEL}"))
                        .monospace()
                        .color(skin.text_strong),
                );
                ui.label(
                    RichText::new(&skin.theme.meta.name)
                        .small()
                        .color(skin.text_muted),
                );
                if self.alpaca.is_ready() {
                    let s = crate::market::status(&self.hub, &self.alpaca);
                    let color = crate::market::status_color(skin, &s);
                    ui.label(
                        RichText::new(format!("US {}", crate::market::status_text(&s)))
                            .small()
                            .color(color),
                    );
                    widgets::lamp(ui, color);
                }
            });
        });
    }

    fn dock_style(&self, ui: &Ui) -> egui_dock::Style {
        let s = &self.skin;
        let mut style = egui_dock::Style::from_egui(ui.style());
        let radius = egui::CornerRadius::same(s.theme.style.rounding as u8);
        style.main_surface_border_stroke = Stroke::NONE;
        style.tab_bar.bg_fill = s.background;
        style.tab_bar.hline_color = s.border;
        style.tab_bar.corner_radius = radius;
        style.separator.color_idle = s.border;
        style.separator.color_hovered = s.accent;
        style.separator.color_dragged = s.live;
        style.tab.tab_body.bg_fill = s.surface;
        style.tab.tab_body.stroke = Stroke::new(1.0, s.border);
        style.tab.tab_body.inner_margin = Margin::same(s.theme.style.padding as i8);
        style.tab.hline_below_active_tab_name = true;
        for (t, bg, text) in [
            (&mut style.tab.active, s.surface, s.text_strong),
            (&mut style.tab.focused, s.surface_alt, s.text_strong),
            (
                &mut style.tab.active_with_kb_focus,
                s.surface_alt,
                s.text_strong,
            ),
            (
                &mut style.tab.focused_with_kb_focus,
                s.surface_alt,
                s.text_strong,
            ),
            (&mut style.tab.hovered, s.surface, s.text),
            (&mut style.tab.inactive, s.background, s.text_muted),
            (
                &mut style.tab.inactive_with_kb_focus,
                s.background,
                s.text_muted,
            ),
        ] {
            t.bg_fill = bg;
            t.text_color = text;
            t.outline_color = s.border;
            t.corner_radius = radius;
        }
        style.buttons.close_tab_color = s.text_muted;
        style.buttons.close_tab_active_color = s.negative;
        style.buttons.close_tab_bg_fill = s.surface_alt;
        style
    }
}

impl eframe::App for TerminalApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.background_work(&ctx);
        let mut commands = std::mem::take(&mut self.pending);
        if let Some(forwarded) = self.remote.as_ref().and_then(|r| r.drain()) {
            // Another launch: come forward (Windows may only flash the taskbar
            // button if focus cannot be taken) and run what it asked for.
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
            commands.extend(forwarded.into_iter().map(AppCommand::Run));
        }
        if let Some(req) = self.capture_next.take() {
            crate::capture::send(&ctx, req);
        }
        for result in crate::capture::deliver(&ctx, &self.paths.exports_dir) {
            match result {
                Ok(msg) => self.feedback(msg, false),
                Err(msg) => self.feedback(msg, true),
            }
        }
        self.shortcuts(&ctx, &mut commands);
        self.check_alerts(&ctx);

        let bar = Frame::new()
            .fill(self.skin.surface)
            .inner_margin(Margin::symmetric(10, 6));
        egui::Panel::top("mt-top")
            .frame(bar)
            .show(ui, |ui| self.top_bar(ui, &mut commands));
        egui::Panel::bottom("mt-status")
            .frame(
                Frame::new()
                    .fill(self.skin.surface)
                    .inner_margin(Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| self.status_bar(ui, &mut commands));
        if self.alpaca.is_ready() {
            // Which account the keys open, across the window, above the status bar.
            let mode = self.alpaca.mode();
            let account = self.hub.watch(&self.alpaca.account());
            egui::Panel::bottom("mt-account-band")
                .frame(
                    Frame::new()
                        .fill(crate::portfolio::band_fill(&self.skin, mode))
                        .inner_margin(Margin::symmetric(10, 1)),
                )
                .show(ui, |ui| {
                    let trading_on = self.config.trading.enabled;
                    if crate::portfolio::band(ui, &self.skin, mode, &account, trading_on) {
                        commands.push(AppCommand::Open(Route::code("ACCT")));
                    }
                });
        }

        let dock_style = self.dock_style(ui);
        let mut cx = PanelCx {
            hub: &self.hub,
            miso: &self.miso,
            nws: &self.nws,
            eia: &self.eia,
            alpaca: &self.alpaca,
            desk: &self.desk,
            skin: &self.skin,
            config: &self.config,
            paths: &self.paths,
            registry: &self.registry,
            themes: &self.themes,
            notices: &self.notices,
            alerts: &self.alerts,
            news_read: &self.news_read,
            can_notify: self.notifier.is_some(),
            commands: &mut commands,
        };
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(self.skin.background)
                    .inner_margin(Margin::same(4)),
            )
            .show(ui, |ui| {
                if let Some(tab) = self.workspace.zoomed_tab() {
                    zoomed_view(ui, &mut cx, tab);
                } else {
                    DockArea::new(&mut self.workspace.dock)
                        .id(egui::Id::new("mt-dock"))
                        .style(dock_style)
                        .show_leaf_close_all_buttons(false)
                        .show_leaf_collapse_buttons(false)
                        .show_inside(ui, &mut Viewer { cx: &mut cx });
                }
            });
        for popped in &mut self.workspace.popped {
            if popout_window(&ctx, &mut cx, popped) {
                cx.send(AppCommand::DockBack(popped.tab.id));
            }
        }

        self.apply_commands(&ctx, commands);
        if let Some(tab) = self.workspace.take_raise() {
            let id = crate::workspace::popout_viewport(tab);
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        // Clocks and "updated 12s ago" labels tick even when no data arrives.
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, WORKSPACE_KEY, &self.workspace);
    }

    fn on_exit(&mut self) {
        self.save_intraday(false);
    }
}

/// A popped-out tab in its own OS window (or, where the platform has no extra
/// windows, a floating one). Returns whether it should go back in the dock.
fn popout_window(
    ctx: &egui::Context,
    cx: &mut PanelCx<'_>,
    popped: &mut crate::workspace::Popped,
) -> bool {
    let skin = cx.skin;
    let title = popped.tab.title(cx.registry);
    let mut builder = egui::ViewportBuilder::default()
        .with_title(format!("{title} · MISO Terminal"))
        .with_inner_size(popped.size.unwrap_or([960.0, 640.0]))
        .with_min_inner_size([360.0, 240.0]);
    if let Some(pos) = popped.pos {
        builder = builder.with_position(pos);
    }
    let id = crate::workspace::popout_viewport(popped.tab.id);
    ctx.show_viewport_immediate(id, builder, |ui, class| {
        let pad = skin.theme.style.padding;
        let mut body = |ui: &mut Ui| {
            let mut dock_back = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new(&title).strong().color(skin.accent));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    dock_back = ui
                        .small_button("Dock")
                        .on_hover_text("Put this panel back in the main window")
                        .clicked();
                });
            });
            popped.tab.set_body_rect(ui.max_rect().expand(pad));
            crate::workspace::draw_tab(ui, cx, &mut popped.tab);
            dock_back
        };
        if class == egui::ViewportClass::EmbeddedWindow {
            // Already inside a floating egui window in the main viewport, whose
            // geometry and close state are the main window's: not ours to keep.
            return body(ui);
        }
        let dock_back = egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(skin.background)
                    .inner_margin(Margin::same(pad as i8)),
            )
            .show(ui, |ui| body(ui))
            .inner;
        // Remember where it is, to reopen it there.
        ui.ctx().input(|i| {
            let v = i.viewport();
            if let Some(r) = v.outer_rect {
                popped.pos = Some([r.min.x, r.min.y]);
            }
            if let Some(r) = v.inner_rect {
                popped.size = Some([r.width(), r.height()]);
            }
            dock_back || v.close_requested()
        })
    })
}

/// One tab filling the window, with a strip to go back to the layout.
fn zoomed_view(ui: &mut Ui, cx: &mut PanelCx<'_>, tab: &mut crate::workspace::Tab) {
    let skin = cx.skin;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(tab.title(cx.registry))
                .strong()
                .color(skin.accent),
        );
        ui.label(
            RichText::new("zoomed · Ctrl+M or Esc for the layout")
                .small()
                .color(skin.text_muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Back to layout").clicked() {
                cx.send(AppCommand::Zoom(None));
            }
        });
    });
    let pad = skin.theme.style.padding;
    Frame::new()
        .fill(skin.surface)
        .stroke(egui::Stroke::new(1.0, skin.border))
        .inner_margin(Margin::same(pad as i8))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            tab.set_body_rect(ui.max_rect().expand(pad));
            crate::workspace::draw_tab(ui, cx, tab);
        });
}

/// Whether Alpaca can be asked: keys are stored, or the data is a replay
/// (which needs none).
fn alpaca_ready(hub: &DataHub) -> bool {
    !hub.ctx().is_live() || mt_alpaca::has_keys(hub.ctx().secrets().as_ref())
}

/// Disk-cache key for a market day's five-minute store.
pub(crate) use mt_miso::intraday_archive_key as intraday_key;

/// Open a folder in the platform file manager.
fn reveal(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        tracing::warn!("could not open {}: {e}", path.display());
    }
}
