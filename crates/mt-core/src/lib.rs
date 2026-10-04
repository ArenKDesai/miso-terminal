//! Domain model for MISO market data, and for the securities beside it.
//!
//! This crate has no I/O and no knowledge of where data comes from. Sources (see
//! `mt-miso`) parse their wire formats *into* these types, and the UI renders them.
//! Keeping the model here means a second source (a local archive, another API)
//! can feed the same panels without the panels changing.
//!
//! MISO data runs on fixed EST ([`time`]); securities run on New York time
//! ([`exchange`]), are named `XLU US` ([`instrument`]) and are priced in exact
//! decimals ([`money`]). News headlines and keyword topics are in [`news`].

pub mod archive;
pub mod constraints;
pub mod exchange;
pub mod fuels;
pub mod geo;
pub mod grid;
pub mod instrument;
pub mod money;
pub mod news;
pub mod num;
pub mod prices;
pub mod seams;
pub mod time;
pub mod weather;

pub use archive::*;
pub use constraints::*;
pub use fuels::*;
pub use grid::*;
pub use prices::*;
pub use seams::*;
pub use weather::*;
