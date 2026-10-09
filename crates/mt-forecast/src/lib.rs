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
//! - [`features`], [`gbm`], [`ridge`] and [`node`]: the learned models
//!   (gradient-boosted trees and ridge regression) on features known at the
//!   time, refitted weekly in their backtest, and a node's whole forecast:
//!   every model, its record, bands, and the best of them.
//! - [`securities`]: a security's close, as a distribution: random walk,
//!   with drift, the volatility cone, GARCH(1,1), and a ridge regression on
//!   recent returns offered only where it beats the random walk.
//! - [`calendar`] and [`hourly`]: holidays, kinds of day, and hourly series
//!   with gaps.

pub mod calendar;
pub mod evaluate;
pub mod features;
pub mod gbm;
pub mod hourly;
pub mod models;
pub mod node;
pub mod ridge;
pub mod securities;

/// A deterministic pseudo-random stream for tests (xorshift, and Box–Muller
/// for normals), so tests need no random crate and never flake.
#[cfg(test)]
pub(crate) struct TestRng(pub u64);

#[cfg(test)]
impl TestRng {
    pub fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    pub fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}
