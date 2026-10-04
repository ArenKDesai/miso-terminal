//! The MISO Terminal user interface (egui).
//!
//! - [`app`]: the shell (command line, ticker, dock, status bar).
//! - [`function`] + [`functions`]: the function registry and every panel.
//! - [`command`]: command-line parsing and completion.
//! - [`workspace`]: the docking layout and its persistence.
//! - [`skin`] + [`fonts`]: applying an `mt_theme::Theme` to egui.
//! - [`widgets`]: shared building blocks (tiles, tables, charts, formatting).

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
pub mod notify;
pub mod remote;
pub mod series;
pub mod skin;
pub mod widgets;
pub mod workspace;

pub use app::{Deps, TerminalApp};
pub use config::{AppConfig, AppPaths};

#[cfg(test)]
mod smoke_tests;
