//! MISO Terminal: wiring. Decides where files live, starts logging and the data
//! runtime, then hands everything to the UI. No product logic lives here.

// Release builds are GUI apps on Windows: no console window behind them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use mt_data::{
    DataHub, DiskCache, EventLog, FetchCtx, FetchCtxOptions, FixtureTransport, HttpTransport,
    Transport,
};
use mt_ui::{AppConfig, AppPaths, Deps, TerminalApp};

mod instance;
mod toast;

// Orders without a review exist only for the order desk's tests; a release
// that could make one does not build.
#[cfg(not(debug_assertions))]
const _: () = assert!(
    !mt_core::guard::UNREVIEWED_ORDERS,
    "mt-core's unreviewed-orders feature must never reach a release build"
);

const APP_NAME: &str = "MISO Terminal";
const USAGE: &str = "\
MISO Terminal - a Bloomberg-style information terminal for MISO

USAGE: miso-terminal [OPTIONS]

OPTIONS:
  --offline [DIR]   Replay recorded responses from DIR (default: bundled fixtures)
                    instead of calling MISO. For demos, development and outages.
  --home DIR        Keep config, themes, cache, logs and layout in DIR (portable mode).
                    Also set by MISO_TERMINAL_HOME, or a file named `portable`
                    next to the executable.
  --reset-layout    Start with the default layout.
  --reset-config    Start with the default settings: config.toml is replaced
                    with the defaults and the old file kept as config.toml.bak.
                    API keys and the layout are kept. Ignored if the terminal
                    is already open (use SET there instead).
  --run COMMAND     Run a command-line command at startup (repeatable),
                    e.g. --run \"GP ALTE.ALTE\" for a desktop shortcut. If the
                    terminal is already open, the command runs in that window.
  --new-instance    Open another window even if one is running (live mode
                    normally allows one window per home, so MISO is polled once).
  --version         Print the version.
  --help            Print this help.";

#[derive(Default)]
struct Args {
    offline: Option<Option<PathBuf>>,
    home: Option<PathBuf>,
    reset_layout: bool,
    reset_config: bool,
    run: Vec<String>,
    new_instance: bool,
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1).peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--offline" => {
                let dir = it.next_if(|n| !n.starts_with("--")).map(PathBuf::from);
                args.offline = Some(dir);
            }
            "--home" => args.home = Some(it.next().context("--home needs a directory")?.into()),
            "--reset-layout" => args.reset_layout = true,
            "--reset-config" => args.reset_config = true,
            "--new-instance" => args.new_instance = true,
            "--run" => args.run.push(it.next().context("--run needs a command")?),
            "--version" | "-V" => {
                println!("miso-terminal {}", env!("MT_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => anyhow::bail!("unknown argument {other:?}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

/// Per-user locations, or one directory in portable mode.
fn resolve_paths(home: Option<PathBuf>) -> Result<AppPaths> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let portable = home
        .or_else(|| std::env::var_os("MISO_TERMINAL_HOME").map(PathBuf::from))
        .or_else(|| {
            exe_dir
                .filter(|d| d.join("portable").exists())
                .map(|d| d.join("data"))
        });
    if let Some(root) = portable {
        return Ok(AppPaths::under(&root));
    }
    // Windows: config, themes and fonts roam (%APPDATA%\MISO Terminal\config);
    // cache, logs and window state stay local (%LOCALAPPDATA%\MISO Terminal).
    let dirs = directories::ProjectDirs::from("", "", APP_NAME).context("no home directory")?;
    let config = dirs.config_dir();
    let local = dirs.data_local_dir();
    Ok(AppPaths {
        config_file: config.join("config.toml"),
        themes_dir: config.join("themes"),
        fonts_dir: config.join("fonts"),
        cache_dir: dirs.cache_dir().to_path_buf(),
        log_dir: local.join("logs"),
        state_file: local.join("state.ron"),
        exports_dir: directories::UserDirs::new()
            .and_then(|u| u.picture_dir().map(|p| p.join(APP_NAME)))
            .unwrap_or_else(|| local.join("exports")),
        audit_dir: local.join("audit"),
    })
}

fn init_logging(dir: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;
    let filter =
        tracing_subscriber::EnvFilter::try_from_env("MISO_TERMINAL_LOG").unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new("info,mt_data=debug,wgpu=warn,naga=warn")
        });
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let (file, guard) = match std::fs::create_dir_all(dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(dir, "miso-terminal.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(writer),
                ),
                Some(guard),
            )
        }
        Err(_) => (None, None),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .init();
    guard
}

/// The recorded fixtures: next to the executable when packaged, else the repo copy.
fn default_fixtures() -> PathBuf {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("fixtures")));
    let dir = beside_exe
        .filter(|d| d.exists())
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures")));
    // Resolve `..` for display, without Windows' `\\?\` verbatim prefix.
    match std::fs::canonicalize(&dir) {
        Ok(p) => PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\")),
        Err(_) => dir,
    }
}

fn main() -> Result<()> {
    // `0.2.0`, or `0.2.0+3f2a1c9` outside releases (build.rs).
    mt_ui::set_version(env!("MT_VERSION"));
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    let paths = resolve_paths(args.home)?;
    let _log_guard = init_logging(&paths.log_dir);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
    // One live window per home: a second launch hands its commands over.
    // Offline replay is for development and demos, so it never forwards.
    let primary = if args.offline.is_none() && !args.new_instance {
        let dir = paths
            .state_file
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        match instance::claim(&dir, &args.run) {
            Ok(instance::Claim::Forwarded) => {
                tracing::info!("already running; handed over {:?}", args.run);
                if args.reset_config {
                    // The open window would save its settings over the reset.
                    tracing::warn!("--reset-config ignored: the terminal is already open; use SET");
                }
                return Ok(());
            }
            Ok(instance::Claim::Primary(p)) => Some(p),
            Err(e) => {
                tracing::warn!("could not reach the running window ({e}); opening another");
                None
            }
        }
    } else {
        None
    };
    tracing::info!(
        "MISO Terminal {} starting; config {}",
        env!("MT_VERSION"),
        paths.config_file.display()
    );

    if args.reset_config {
        match AppConfig::reset(&paths.config_file) {
            Ok(Some(backup)) => tracing::info!(
                "config reset to defaults; the old file is {}",
                backup.display()
            ),
            Ok(None) => tracing::info!("config reset to defaults"),
            // Leave the file alone rather than lose it without a copy.
            Err(e) => tracing::warn!("--reset-config failed, keeping the current config: {e}"),
        }
    }
    let (config, config_error) = AppConfig::load(&paths.config_file);
    if !paths.config_file.exists() {
        // Write the defaults so there is a file to find and edit.
        if let Err(e) = config.save(&paths.config_file) {
            tracing::warn!("could not write default config: {e}");
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("mt-data")
        .enable_all()
        .build()
        .context("starting the data runtime")?;

    let replaying = args.offline.is_some();
    let (transport, cache): (Arc<dyn Transport>, Option<DiskCache>) = match args.offline {
        Some(dir) => {
            let dir = dir.unwrap_or_else(default_fixtures);
            tracing::info!("offline mode: replaying {}", dir.display());
            (Arc::new(FixtureTransport::new(dir)), None)
        }
        None => {
            let ua = format!(
                "MISO-Terminal/{} (+{})",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_REPOSITORY")
            );
            (
                Arc::new(HttpTransport::new(&ua)?),
                Some(DiskCache::new(paths.cache_dir.join("http"))),
            )
        }
    };
    let opts = FetchCtxOptions {
        max_concurrent: config.data.max_concurrent_requests,
        polite_interval: std::time::Duration::from_secs(config.data.polite_interval_secs),
        // API keys: Windows Credential Manager live; memory (nothing saved) when replaying.
        secrets: if replaying {
            Arc::new(mt_data::MemorySecrets::default())
        } else {
            mt_data::os_store("miso-terminal")
        },
        // Pacing publishers ask for (FT's robots.txt: one request a second)
        // and Alpaca's per-key limit; a replay has nobody to be polite to.
        budgets: if replaying {
            Vec::new()
        } else {
            mt_news::budgets()
                .into_iter()
                .chain(mt_alpaca::budgets())
                .collect()
        },
    };
    if let Some(cache) = cache.clone() {
        let max = config.data.cache_max_mb.saturating_mul(1024 * 1024);
        let archive_days = config.data.archive_days;
        let news_days = config.news.keep_days;
        runtime.spawn_blocking(move || {
            let mut keep = vec![
                mt_miso::archive_dir(&cache),
                mt_miso::lmp_archive_dir(&cache),
            ];
            keep.extend(mt_news::archive_dirs(&cache));
            let (files, bytes) = cache.prune(max, &keep);
            if files > 0 {
                tracing::info!("pruned {files} cached reports ({} MB)", bytes / 1_048_576);
            }
            let days = mt_miso::prune_archive(&cache, archive_days, mt_core::time::market_today());
            if days > 0 {
                tracing::info!("removed {days} days from the five-minute archive");
            }
            let feeds = mt_news::prune_archives(&cache, news_days);
            if feeds > 0 {
                tracing::info!("removed {feeds} news feeds not fetched in {news_days} days");
            }
        });
    }
    let hub = DataHub::new(
        runtime.handle().clone(),
        FetchCtx::new(transport, cache, opts, EventLog::default()),
    );

    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon/miso-terminal.png"))
            .context("decoding the window icon")?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("miso-terminal")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([900.0, 560.0])
            .with_icon(Arc::new(icon)),
        persistence_path: Some(paths.state_file.clone()),
        ..Default::default()
    };
    let notifier = paths.state_file.parent().and_then(toast::notifier);
    let (remote, inbox) = mt_ui::remote::channel_pair();
    let _instance_lock = primary.map(|p| p.serve(remote));
    let deps = Deps {
        hub,
        config,
        config_error,
        paths,
        reset_layout: args.reset_layout,
        startup_commands: args.run,
        remote: Some(inbox),
        notifier,
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(TerminalApp::new(cc, deps)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!("exiting");
    drop(runtime);
    Ok(())
}
