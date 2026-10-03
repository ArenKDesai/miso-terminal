//! The application shell: command line and hub ticker on top, the dock in the
//! middle, a status bar at the bottom. Everything else is a function panel.

use std::time::{Duration, Instant};

use egui::{Align, Frame, Key, Layout, Margin, RichText, Stroke, Ui};
use egui_dock::DockArea;
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
const THEME_POLL: Duration = Duration::from_secs(2);
const GC_EVERY: Duration = Duration::from_secs(60);
const GC_IDLE: Duration = Duration::from_secs(15 * 60);
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
}

#[derive(Default)]
struct CommandLine {
    text: String,
    selected: usize,
    focus: bool,
    feedback: Option<(String, bool)>,
    history: Vec<String>,
    history_pos: Option<usize>,
}

pub struct TerminalApp {
    hub: DataHub,
    miso: Miso,
    nws: mt_nws::Nws,
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
        let mut app = Self {
            miso: Miso::new(deps.config.endpoints.clone()),
            nws: mt_nws::Nws::default(),
            hub: deps.hub,
            registry: Registry::builtin(),
            skin: Skin::new(themes.resolve(&deps.config.theme).clone()),
            themes_fingerprint: dir_fingerprint(&deps.paths.themes_dir),
            themes,
            fonts: FontLibrary::new(Some(deps.paths.fonts_dir.clone())),
            notices: deps.config_error.into_iter().collect(),
            workspace: Workspace::restore(saved),
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
            config: deps.config,
            paths: deps.paths,
        };
        app.apply_theme(ctx);
        app
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
            self.notices.push(format!(
                "Theme {wanted:?} not found; using {}.",
                theme.meta.id
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

    fn save_config(&mut self) {
        if let Err(e) = self.config.save(&self.paths.config_file) {
            self.notices.push(format!("Could not save config: {e}"));
        }
    }

    fn feedback(&mut self, msg: impl Into<String>, error: bool) {
        self.cmd.feedback = Some((msg.into(), error));
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
                self.feedback(route.to_string(), false);
                self.workspace.open(route, &self.registry);
            }
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
                AppCommand::AddFavorite(node) => {
                    let node = node.trim().to_ascii_uppercase();
                    if !node.is_empty() && !self.config.ui.favorite_nodes.contains(&node) {
                        self.config.ui.favorite_nodes.push(node);
                        self.save_config();
                    }
                }
                AppCommand::RemoveFavorite(node) => {
                    self.config.ui.favorite_nodes.retain(|n| n != &node);
                    self.save_config();
                }
                AppCommand::ResetLayout => self.workspace = Workspace::default_layout(),
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
                AppCommand::ReplaceConfig(config) => {
                    let endpoints_changed = config.endpoints != self.config.endpoints;
                    self.config = *config;
                    ctx.set_zoom_factor(self.config.ui.zoom.clamp(0.5, 3.0));
                    if endpoints_changed {
                        // Cached values stay until their next refresh, which uses the new URLs.
                        self.miso = Miso::new(self.config.endpoints.clone());
                    }
                    self.save_config();
                    self.feedback("Settings saved", false);
                }
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
            let fp = dir_fingerprint(&self.paths.themes_dir);
            if fp != self.themes_fingerprint {
                self.themes_fingerprint = fp;
                self.themes.reload();
                self.apply_theme(ctx);
                tracing::info!("themes folder changed; reloaded");
            }
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
            if i.consume_key(egui::Modifiers::COMMAND, Key::K)
                || i.consume_key(egui::Modifiers::NONE, Key::Escape)
            {
                self.cmd.focus = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::F1) {
                commands.push(AppCommand::Open(Route::code("HELP")));
            }
            if i.consume_key(egui::Modifiers::NONE, Key::F5) {
                commands.push(AppCommand::RefreshWatched);
            }
            if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L) {
                commands.push(AppCommand::ResetLayout);
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
        let suggestions: Vec<Suggestion> =
            command::suggest(&self.cmd.text, &self.registry, &self.known_nodes(), 10);
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
            .hint_text("type a function or node, e.g. LMP, GP MINN.HUB, FUEL — Enter to go")
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
                let local = chrono::Local::now().format("%H:%M:%S");
                let market = now_market().format("%a %b %d  %H:%M:%S");
                ui.label(
                    RichText::new(format!("local {local}"))
                        .small()
                        .color(skin.text_muted),
                );
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

        let dock_style = self.dock_style(ui);
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(self.skin.background)
                    .inner_margin(Margin::same(4)),
            )
            .show(ui, |ui| {
                let mut cx = PanelCx {
                    hub: &self.hub,
                    miso: &self.miso,
                    nws: &self.nws,
                    skin: &self.skin,
                    config: &self.config,
                    paths: &self.paths,
                    registry: &self.registry,
                    themes: &self.themes,
                    notices: &self.notices,
                    alerts: &self.alerts,
                    commands: &mut commands,
                };
                DockArea::new(&mut self.workspace.dock)
                    .id(egui::Id::new("mt-dock"))
                    .style(dock_style)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_inside(ui, &mut Viewer { cx: &mut cx });
            });

        self.apply_commands(&ctx, commands);
        // Clocks and "updated 12s ago" labels tick even when no data arrives.
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, WORKSPACE_KEY, &self.workspace);
    }
}

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
