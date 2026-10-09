//! The MISO Terminal user interface (egui).
//!
//! - [`app`]: the shell (command line, ticker, dock, status bar).
//! - [`function`] + [`functions`]: the function registry and every panel.
//! - [`command`]: command-line parsing and completion.
//! - [`workspace`]: the docking layout and its persistence.
//! - [`skin`] + [`fonts`]: applying an `mt_theme::Theme` to egui.
//! - [`widgets`]: shared building blocks (tiles, tables, charts, formatting).
//! - [`news`]: combined headlines and the headline browser (TOP, NEWS, NI, CN).
//! - [`market`]: shared pieces for securities (market status, live rows, formats).
//! - [`portfolio`]: shared pieces for the account (live marks, re-sync, the band).
//! - [`trading`]: shared pieces for order tickets and the blotter (orders, checks, outcomes).
//! - [`history`]: the price history's window and backfill, as SET, LOG and the status bar show them.

pub mod alerts;
pub mod app;
pub mod capture;
pub mod command;
pub mod config;
pub mod context;
pub mod fonts;
pub mod function;
pub mod functions;
pub mod gallery;
pub mod geo;
pub mod history;
pub mod market;
pub mod news;
pub mod notify;
pub mod options;
pub mod portfolio;
pub mod remote;
pub mod series;
pub mod skin;
pub mod trading;
pub mod widgets;
pub mod workspace;

pub use app::{Deps, TerminalApp};
pub use config::{AppConfig, AppPaths};

static VERSION: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The version HELP and LOG show: the binary's (`0.2.0`, or `0.2.0+3f2a1c9`
/// outside releases, from its build script), set once at startup with
/// [`set_version`]. Tests and examples see this crate's version.
pub fn version() -> &'static str {
    VERSION.get().copied().unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Set the version [`version`] reports. Only the first call counts.
pub fn set_version(version: &'static str) {
    let _ = VERSION.set(version);
}

/// The disk cache's directories the app keeps on purpose (the five-minute
/// archive, the day store, saved headlines), each with its own retention: the
/// size cap and LOG's *Clear cache* leave them alone.
pub fn kept_cache_dirs(cache: &mt_data::DiskCache) -> Vec<std::path::PathBuf> {
    let mut keep = vec![mt_miso::archive_dir(cache), mt_miso::day_store_dir(cache)];
    keep.extend(mt_news::archive_dirs(cache));
    keep
}

#[cfg(test)]
mod smoke_tests;
