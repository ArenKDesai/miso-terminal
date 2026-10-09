//! Domain model for MISO market data, and for the securities beside it.
//!
//! This crate has no I/O and no knowledge of where data comes from. Sources (see
//! `mt-miso`) parse their wire formats *into* these types, and the UI renders them.
//! Keeping the model here means a second source (a local archive, another API)
//! can feed the same panels without the panels changing.
//!
//! MISO data runs on fixed EST ([`time`]); securities run on New York time
//! ([`exchange`]), are named `XLU US` ([`instrument`]), are quoted as trades,
//! quotes and bars ([`equity`]) and are priced in exact decimals ([`money`])
//! wherever an order is involved; the studies charts draw over them (moving
//! averages, Bollinger bands, RSI, MACD) are in [`studies`]. Options
//! contracts, chains and the price steps they trade in are in [`options`]. A
//! brokerage account's balances, positions,
//! equity curve and activities are in [`account`]; orders are in [`order`], and
//! the guardrails every order ticket passes in [`guard`]. News headlines and
//! keyword topics are in [`news`].

pub mod account;
pub mod beta;
pub mod constraints;
pub mod equity;
pub mod exchange;
pub mod fuels;
pub mod geo;
pub mod grid;
pub mod guard;
pub mod instrument;
pub mod money;
pub mod news;
pub mod num;
pub mod options;
pub mod order;
pub mod prices;
pub mod seams;
pub mod studies;
pub mod time;
pub mod weather;

pub use constraints::*;
pub use fuels::*;
pub use grid::*;
pub use prices::*;
pub use seams::*;
pub use weather::*;
