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
    /// Follow the OS light/dark setting with these two themes (or stop following).
    SetThemeFollow {
        follow: bool,
        light: String,
        dark: String,
    },
    /// Add a node to the watchlist (`config.ui.favorite_nodes`).
    AddFavorite(String),
    RemoveFavorite(String),
    ResetLayout,
    /// Close the active tab in the focused pane.
    CloseTab,
    /// Activate the next (`true`) or previous tab in the focused pane.
    CycleTab(bool),
    /// Fill the window with the focused tab, or go back to the layout.
    ToggleZoom,
    /// Fill the window with this tab (or go back to the layout with `None`).
    Zoom(Option<u64>),
    /// Move this tab into its own OS window.
    PopOut(u64),
    /// Put a popped-out tab back in the dock.
    DockBack(u64),
    RefreshWatched,
    SetPaused(bool),
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
    /// Open a folder in the system file manager.
    RevealPath(std::path::PathBuf),
}

/// Everything a panel can use while drawing. Built fresh per panel per frame.
pub struct PanelCx<'a> {
    pub hub: &'a DataHub,
    pub miso: &'a Miso,
    /// Weather (National Weather Service).
    pub nws: &'a mt_nws::Nws,
    /// Fuel prices (EIA).
    pub eia: &'a mt_eia::Eia,
    pub skin: &'a Skin,
    pub config: &'a AppConfig,
    pub paths: &'a AppPaths,
    pub registry: &'a Registry,
    pub themes: &'a ThemeRegistry,
    /// Problems worth showing in LOG (config parse errors, missing fonts).
    pub notices: &'a [String],
    /// Alert state and the history of fired alerts.
    pub alerts: &'a AlertEngine,
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
