use mt_data::DataHub;
use mt_miso::Miso;
use mt_theme::ThemeRegistry;

use crate::alerts::{AlertEngine, AlertRule};
use crate::config::{AppConfig, AppPaths};
use crate::function::{Registry, Route};
use crate::skin::Skin;

/// Requests from panels to the app, applied after the frame's panels have drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum AppCommand {
    /// Open (or focus) a function.
    Open(Route),
    /// Run a command-line string as if typed.
    Run(String),
    SetTheme(String),
    /// Write a gallery theme into the themes folder (and switch to it).
    InstallTheme {
        theme: Box<mt_theme::GalleryTheme>,
        activate: bool,
    },
    /// Delete an installed theme's `<id>.toml` from the themes folder.
    UninstallTheme(String),
    /// Follow the OS light/dark setting with these two themes (or stop following).
    SetThemeFollow {
        follow: bool,
        light: String,
        dark: String,
    },
    /// Add a node or a security (`XLU US`) to the watchlist
    /// (`config.ui.favorite_nodes` or `favorite_securities`).
    AddFavorite(String),
    RemoveFavorite(String),
    /// API keys were stored or removed (SET): check for them again.
    CredentialsChanged,
    /// Let tickets send orders, or stop them (the kill switch). Saved to
    /// `[trading] enabled`, so it holds across restarts.
    SetTradingEnabled(bool),
    ResetLayout,
    /// Close the active tab in the focused pane.
    CloseTab,
    /// Activate the next (`true`) or previous tab in the focused pane.
    CycleTab(bool),
    /// Fill the window with the focused tab, or go back to the layout.
    ToggleZoom,
    /// Fill the window with this tab (or go back to the layout with `None`).
    Zoom(Option<u64>),
    /// Put the main window in full screen (borderless, over the taskbar),
    /// or take it out.
    ToggleFullscreen,
    /// Move this tab into its own OS window.
    PopOut(u64),
    /// Put a popped-out tab back in the dock.
    DockBack(u64),
    RefreshWatched,
    SetPaused(bool),
    /// Start or pause downloading the price history (SET's *Download now* and
    /// *Pause*). Saved to `[price_history] backfill`, so it carries on after
    /// a restart.
    SetBackfill(bool),
    ClearCache,
    AddAlert(AlertRule),
    /// Remove the alert rule at this index in `config.alerts`.
    RemoveAlert(usize),
    /// The ALRT function has shown the latest alerts.
    AlertsSeen,
    /// Turn system notifications for alerts on or off (saved to config).
    SetNotifyAlerts(bool),
    /// Show a sample system notification.
    TestNotification,
    /// Copy or save a panel as an image (captured on the next frame).
    Capture(crate::capture::Request),
    /// Replace and save the whole configuration (the SET function).
    ReplaceConfig(Box<AppConfig>),
    /// Back up config.toml to config.toml.bak, then replace it with the
    /// defaults (SET's *Reset to defaults*). API keys and the layout stay.
    ResetConfig,
    /// Open a folder in the system file manager.
    RevealPath(std::path::PathBuf),
    /// Open a headline's article in the default browser and mark it read.
    OpenHeadline {
        id: String,
        link: String,
    },
    /// Mark headlines read (or unread).
    MarkRead {
        ids: Vec<String>,
        read: bool,
    },
}

/// Everything a panel can use while drawing. Built fresh per panel per frame.
pub struct PanelCx<'a> {
    pub hub: &'a DataHub,
    pub miso: &'a Miso,
    /// Weather (National Weather Service).
    pub nws: &'a mt_nws::Nws,
    /// Fuel prices (EIA).
    pub eia: &'a mt_eia::Eia,
    /// Stocks and ETFs (Alpaca); `alpaca.is_ready()` once keys are stored.
    pub alpaca: &'a mt_alpaca::Alpaca,
    /// Sends, replaces and cancels orders: only ever from a click in a
    /// ticket or ORD.
    pub desk: &'a mt_alpaca::OrderDesk,
    /// Fills the price history in the background; its progress for SET and LOG.
    pub backfill: &'a mt_miso::Backfill,
    /// Keeps forecasts as issued in the background; its status for SET and LOG.
    pub collector: &'a mt_data::issued::Collector,
    pub skin: &'a Skin,
    pub config: &'a AppConfig,
    pub paths: &'a AppPaths,
    pub registry: &'a Registry,
    pub themes: &'a ThemeRegistry,
    /// Problems worth showing in LOG (config parse errors, missing fonts).
    pub notices: &'a [String],
    /// Alert state and the history of fired alerts.
    pub alerts: &'a AlertEngine,
    /// Which headlines have been opened.
    pub news_read: &'a mt_news::ReadMarks,
    /// Whether system notifications are available on this platform.
    pub can_notify: bool,
    pub(crate) commands: &'a mut Vec<AppCommand>,
}

impl PanelCx<'_> {
    pub fn send(&mut self, cmd: AppCommand) {
        self.commands.push(cmd);
    }

    pub fn open(&mut self, route: Route) {
        self.send(AppCommand::Open(route));
    }

    /// Text colour for a price, using the configured alert thresholds.
    pub fn price_color(&self, v: f64) -> egui::Color32 {
        if v >= self.config.ui.price_extreme {
            self.skin.negative
        } else if v >= self.config.ui.price_alert {
            self.skin.warning
        } else if v < 0.0 {
            self.skin.info
        } else {
            self.skin.text
        }
    }
}
