//! User configuration (`config.toml`) and on-disk locations.
//!
//! Every field has a default and unknown fields are ignored, so old config files
//! keep working as options are added. Layout and window state are not here;
//! eframe persists those separately.

use std::path::{Path, PathBuf};

use mt_miso::MisoEndpoints;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Theme id, e.g. `default`. See the THEME function.
    pub theme: String,
    pub ui: UiConfig,
    pub data: DataConfig,
    pub endpoints: MisoEndpoints,
    /// News feeds and topics (TOP, NEWS, NI): built-ins to turn off, feeds
    /// and topics to add, how long headlines are kept.
    pub news: mt_news::NewsConfig,
    /// Stocks and ETFs from Alpaca (Q, GP, DES, CN): the price feed, the
    /// stream's subscription limit and lists for Q (`[[markets.lists]]`).
    pub markets: mt_alpaca::MarketsConfig,
    /// Paper trading's guardrails (BUY, SELL, ORD): whether tickets may send
    /// orders, per-order, daily and per-position caps in dollars, the price
    /// collar, the fat-finger check and the restricted list.
    pub trading: mt_core::guard::Limits,
    /// Alert rules (see the ALRT function), as `[[alerts]]` tables. Left out
    /// of the file when empty, so a hand-written `[[alerts]]` never clashes
    /// with an `alerts = []` the terminal wrote.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<crate::alerts::AlertRule>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: mt_theme::DEFAULT_THEME_ID.into(),
            ui: UiConfig::default(),
            data: DataConfig::default(),
            endpoints: MisoEndpoints::default(),
            news: mt_news::NewsConfig::default(),
            markets: mt_alpaca::MarketsConfig::default(),
            trading: mt_core::guard::Limits::default(),
            alerts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Extra zoom on top of the OS scale factor (1.0 = none).
    pub zoom: f32,
    /// Prices at or above this are highlighted with the theme's warning colour ($/MWh).
    pub price_alert: f64,
    /// Prices at or above this are highlighted with the theme's negative colour ($/MWh).
    pub price_extreme: f64,
    /// Nodes listed first in pickers and shown in the GP quick list.
    pub favorite_nodes: Vec<String>,
    /// Securities on the watchlist (`XLU US`).
    pub favorite_securities: Vec<String>,
    /// Switch between `light_theme` and `dark_theme` with the Windows setting
    /// (Settings > Personalization > Colors). Overrides `theme` when on.
    pub follow_system_theme: bool,
    /// A missing theme falls back to the default.
    pub light_theme: String,
    pub dark_theme: String,
    /// Function keys that run commands, e.g. `F2 = "HOME"`, `F9 = "GP ALTE.ALTE"`.
    /// F1 (help) and F5 (refresh) are fixed.
    pub hotkeys: std::collections::BTreeMap<String, String>,
    /// Show a system notification (a Windows toast) when an alert fires while
    /// the terminal is not the focused window.
    pub notify_alerts: bool,
    /// Show the local time next to market time in the status bar.
    pub show_local_clock: bool,
}

/// The default function-key bar.
pub fn default_hotkeys() -> std::collections::BTreeMap<String, String> {
    [
        ("F2", "HOME"),
        ("F3", "LMP"),
        ("F4", "MAP"),
        ("F6", "WL"),
        ("F7", "HUBS"),
        ("F8", "WX"),
        ("F9", "ALRT"),
        ("F10", "LOG"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            price_alert: 100.0,
            price_extreme: 500.0,
            favorite_nodes: mt_core::TRADING_HUBS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            favorite_securities: Vec::new(),
            follow_system_theme: false,
            light_theme: "default-light".into(),
            dark_theme: mt_theme::DEFAULT_THEME_ID.into(),
            hotkeys: default_hotkeys(),
            notify_alerts: true,
            show_local_clock: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DataConfig {
    /// Maximum simultaneous HTTP requests.
    pub max_concurrent_requests: usize,
    /// Minimum seconds between requests for the same URL (MISO asks for >= 60 s
    /// per real-time link; the refresh cadence already honours that).
    pub polite_interval_secs: u64,
    /// Default look-back for GP history charts, in days.
    pub history_days: u32,
    /// Size cap for the market-report cache, in MB. Least-recently-used files
    /// are pruned at startup. The archives (five-minute prices, the day store,
    /// headlines) are not counted.
    pub cache_max_mb: u64,
    /// Days of five-minute prices to keep in the local archive (about 3 MB a
    /// day); 0 keeps them all.
    pub archive_days: u32,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: 4,
            polite_interval_secs: 55,
            history_days: 7,
            cache_max_mb: 2048,
            archive_days: 90,
        }
    }
}

impl AppConfig {
    /// Load from `path`; a missing file yields defaults, a broken one yields
    /// defaults plus the error (shown in the LOG function, never fatal).
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(src) => match toml::from_str(&src) {
                Ok(cfg) => (cfg, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let header = "# MISO Terminal configuration. Every key is optional; delete one to get its default.\n\n";
        std::fs::write(path, format!("{header}{body}"))
    }

    /// Where [`AppConfig::back_up`] copies the file: `config.toml.bak`.
    pub fn backup_path(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".bak");
        path.with_file_name(name)
    }

    /// Copy the config file to its backup (replacing an older one) before it
    /// is reset. `None` when there is no file to keep.
    pub fn back_up(path: &Path) -> std::io::Result<Option<PathBuf>> {
        if !path.exists() {
            return Ok(None);
        }
        let backup = Self::backup_path(path);
        std::fs::copy(path, &backup)?;
        Ok(Some(backup))
    }

    /// Replace the file with the defaults, keeping the old one as
    /// `config.toml.bak` (`--reset-config`). API keys live in the credential
    /// store and the layout in the window state, so neither is touched.
    pub fn reset(path: &Path) -> std::io::Result<Option<PathBuf>> {
        let backup = Self::back_up(path)?;
        Self::default().save(path)?;
        Ok(backup)
    }
}

/// Where the app keeps things. Built by the binary (which decides between the
/// per-user locations and portable mode) and handed to the UI.
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_file: PathBuf,
    /// User themes (`*.toml`), hot-reloaded.
    pub themes_dir: PathBuf,
    /// Extra font files that themes can reference by family name.
    pub fonts_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    /// Window geometry and workspace layout (written by eframe).
    pub state_file: PathBuf,
    /// Where "Save panel as PNG" writes.
    pub exports_dir: PathBuf,
    /// The order audit log (`orders-YYYY-MM.jsonl`): local, never roaming.
    pub audit_dir: PathBuf,
}

impl AppPaths {
    /// Everything under one directory (portable mode, tests).
    pub fn under(root: &Path) -> Self {
        Self {
            config_file: root.join("config.toml"),
            themes_dir: root.join("themes"),
            fonts_dir: root.join("fonts"),
            cache_dir: root.join("cache"),
            log_dir: root.join("logs"),
            state_file: root.join("state.ron"),
            exports_dir: root.join("exports"),
            audit_dir: root.join("audit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_and_unknown_keys_are_fine() {
        let cfg: AppConfig =
            toml::from_str("theme = 'high-contrast'\nfuture_option = 3\n[ui]\nzoom = 1.25\n")
                .unwrap();
        assert_eq!(cfg.theme, "high-contrast");
        assert_eq!(cfg.ui.zoom, 1.25);
        assert_eq!(cfg.ui.price_alert, UiConfig::default().price_alert);
        assert_eq!(cfg.endpoints, MisoEndpoints::default());
        assert_eq!(cfg.news, mt_news::NewsConfig::default());
        assert_eq!(cfg.markets, mt_alpaca::MarketsConfig::default());
        assert_eq!(cfg.trading, mt_core::guard::Limits::default());
        assert!(cfg.trading.enabled, "trading is on until the kill switch");
        let traded: AppConfig =
            toml::from_str("[trading]\nmax_order_value = 2500\nrestricted = ['MGEE US']\n")
                .unwrap();
        assert_eq!(traded.trading.max_order_value, 2500);
        assert!(traded.trading.is_restricted("MGEE") && traded.trading.enabled);
    }

    #[test]
    fn default_hotkeys_are_real_keys_and_old_configs_get_them() {
        assert!(
            default_hotkeys()
                .keys()
                .all(|k| egui::Key::from_name(k).is_some())
        );
        let cfg: AppConfig = toml::from_str(
            "[ui]
zoom = 1.0
",
        )
        .unwrap();
        assert_eq!(cfg.ui.hotkeys, default_hotkeys());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("mt-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let cfg = AppConfig {
            theme: "default-light".into(),
            ..Default::default()
        };
        cfg.save(&path).unwrap();
        let (back, err) = AppConfig::load(&path);
        assert!(err.is_none());
        assert_eq!(back, cfg);
        std::fs::write(&path, "theme = [").unwrap();
        let (fallback, err) = AppConfig::load(&path);
        assert_eq!(fallback, AppConfig::default());
        assert!(err.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hand_written_tables_append_to_a_saved_file() {
        // The tutorials say to add [[alerts]], [[news.topics]] and
        // [[markets.lists]] at the end of the file the terminal wrote.
        let dir = std::env::temp_dir().join(format!("mt-config-append-{}", std::process::id()));
        let path = dir.join("config.toml");
        AppConfig::default().save(&path).unwrap();
        let mut src = std::fs::read_to_string(&path).unwrap();
        src.push_str(
            r#"
[[alerts]]
kind = "price_above"
node = "MINN.HUB"
value = 100.0

[[news.topics]]
name = "DATACENTERS"
keywords = ["data center*"]

[[news.feeds]]
id = "my-feed"
source = "Energy News Network"
section = "Midwest"
url = "https://example.org/feed.xml"

[[markets.lists]]
name = "MINE"
symbols = ["XEL", "WEC"]
"#,
        );
        std::fs::write(&path, src).unwrap();
        let (cfg, err) = AppConfig::load(&path);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(cfg.alerts.len(), 1);
        assert_eq!(cfg.news.topics[0].name, "DATACENTERS");
        assert_eq!(cfg.news.feeds[0].id, "my-feed");
        assert_eq!(cfg.markets.lists[0].name, "MINE");
        // And they survive the terminal saving the file again.
        cfg.save(&path).unwrap();
        assert_eq!(AppConfig::load(&path).0, cfg);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reset_keeps_a_backup() {
        let dir = std::env::temp_dir().join(format!("mt-config-reset-{}", std::process::id()));
        let path = dir.join("config.toml");
        assert_eq!(AppConfig::reset(&path).unwrap(), None, "nothing to back up");
        assert_eq!(AppConfig::load(&path).0, AppConfig::default());
        let mine = AppConfig {
            theme: "high-contrast".into(),
            ..Default::default()
        };
        mine.save(&path).unwrap();
        let backup = AppConfig::reset(&path).unwrap().expect("a backup");
        assert_eq!(backup, dir.join("config.toml.bak"));
        assert_eq!(AppConfig::load(&backup).0, mine);
        assert_eq!(AppConfig::load(&path).0, AppConfig::default());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Every `config.toml` an earlier version wrote (`fixtures/compat/`, one
    /// per release) still loads, strictly: `load` would fall back to the
    /// defaults and hide the break. What it holds survives a save, too.
    #[test]
    fn configs_from_earlier_versions_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        assert!(files.len() >= 2, "{}", dir.display());
        for path in files {
            let src = std::fs::read_to_string(&path).unwrap();
            let cfg: AppConfig = toml::from_str(&src)
                .unwrap_or_else(|e| panic!("{} no longer loads: {e}", path.display()));
            let again: AppConfig = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
            assert_eq!(again, cfg, "{}", path.display());
        }
    }
}
