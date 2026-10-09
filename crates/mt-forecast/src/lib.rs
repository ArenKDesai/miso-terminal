//! Forecasts for FCST: tomorrow's DA and RT prices at a node, and a
//! security's close a few weeks out. Pure computation, no I/O and no UI, so
//! CI tests it on Linux.
//!
//! - [`models`]: statistical models of a day's 24 hours, always computed and
//!   always shown as the bar to beat (the same hour on the last day known and
//!   a week earlier, an hour-by-day profile, exponential smoothing with daily
//!   and weekly seasonality).
//! - [`evaluate`]: rolling-origin backtests from what was known at the time,
//!   accuracy (MAE, RMSE, pinball loss, band coverage) and conformal bands
//!   from past errors.
//! - [`securities`]: a security's close, as a distribution: random walk,
//!   with drift, the volatility cone and GARCH(1,1).
//! - [`calendar`] and [`hourly`]: holidays, kinds of day, and hourly series
//!   with gaps.

pub mod calendar;
pub mod evaluate;
pub mod hourly;
pub mod models;
pub mod securities;
