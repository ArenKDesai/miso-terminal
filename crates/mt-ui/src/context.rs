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
    RefreshWatched,
    SetPaused(bool),
    ClearCache,
    AddAlert(AlertRule),
    /// Remove the alert rule at this index in `config.alerts`.
    RemoveAlert(usize),
    /// The ALRT function has shown the latest alerts.
    AlertsSeen,
    /// Open a folder in the system file manager.
    RevealPath(std::path::PathBuf),
}

/// Everything a panel can use while drawing. Built fresh per panel per frame.
pub struct PanelCx<'a> {
    pub hub: &'a DataHub,
    pub miso: &'a Miso,
    pub skin: &'a Skin,
    pub config: &'a AppConfig,
    pub paths: &'a AppPaths,
    pub registry: &'a Registry,
    pub themes: &'a ThemeRegistry,
    /// Problems worth showing in LOG (config parse errors, missing fonts).
    pub notices: &'a [String],
    /// Alert state and the history of fired alerts.
    pub alerts: &'a AlertEngine,
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
