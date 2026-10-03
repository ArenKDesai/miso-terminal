//! Domain model for MISO market data.
//!
//! This crate has no I/O and no knowledge of where data comes from. Sources (see
//! `mt-miso`) parse their wire formats *into* these types, and the UI renders them.
//! Keeping the model here means a second source (a local archive, another API)
//! can feed the same panels without the panels changing.

pub mod constraints;
pub mod geo;
pub mod grid;
pub mod num;
pub mod prices;
pub mod seams;
pub mod time;
pub mod weather;

pub use constraints::*;
pub use grid::*;
pub use prices::*;
pub use seams::*;
pub use weather::*;
