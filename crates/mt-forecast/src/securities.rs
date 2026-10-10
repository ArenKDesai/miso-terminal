//! Forecasts of a security's close one to twenty trading days out. Daily
//! returns are close to unpredictable, so these forecast a distribution, not
//! a direction: where the price may be, and how widely, by four views of its
//! volatility. Returns are log returns of daily closes.

use crate::evaluate::{QUANTILES, pinball_loss};
use crate::gbm::Matrix;
use crate::hourly::quantile;
use crate::ridge::Ridge;

/// The longest horizon offered, in trading days.
pub const MAX_HORIZON: usize = 20;

/// Trading days of returns the models look back on (a year).
const LOOKBACK: usize = 252;

/// The standard normal's quantiles at [`QUANTILES`].
const Z: [f64; 5] = [
    -1.281_551_565_544_600_4,
    -0.674_489_750_196_081_7,
    0.0,
    0.674_489_750_196_081_7,
    1.281_551_565_544_600_4,
];

/// The close's quantiles ([`QUANTILES`]) one to `horizon` trading days out.
pub type Path = Vec<[f64; 5]>;

pub trait CloseModel: Send + Sync {
    fn name(&self) -> &'static str;

    /// Machine learning, offered only where it beats the random walk.
    fn learned(&self) -> bool {
        false
    }

    /// From closes (oldest first, all positive) up to the last known one.
    fn forecast(&self, closes: &[f64], horizon: usize) -> Option<Path>;
}

fn log_returns(closes: &[f64]) -> Vec<f64> {
    closes
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1] > 0.0)
        .map(|w| (w[1] / w[0]).ln())
        .collect()
}

fn mean_and_sd(x: &[f64]) -> Option<(f64, f64)> {
    if x.len() < 20 {
        return None;
    }
    let n = x.len() as f64;
    let mean = x.iter().sum::<f64>() / n;
    let var = x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some((mean, var.sqrt()))
}

/// A normal distribution of the h-day log return, `drift` and `variance`
/// for each h, as close quantiles.
fn normal_path(last: f64, steps: impl Iterator<Item = (f64, f64)>) -> Path {
    steps
        .map(|(drift, variance)| Z.map(|z| last * (drift + z * variance.sqrt()).exp()))
        .collect()
}

/// No drift: the median is today's close, the spread the past year's daily
/// volatility growing with the square root of time.
pub struct RandomWalk;

impl CloseModel for RandomWalk {
    fn name(&self) -> &'static str {
        "Random walk"
    }

    fn forecast(&self, closes: &[f64], horizon: usize) -> Option<Path> {
        let r = log_returns(closes);
        let (_, sd) = mean_and_sd(&r[r.len().saturating_sub(LOOKBACK)..])?;
        let last = *closes.last()?;
        Some(normal_path(
            last,
            (1..=horizon).map(|h| (0.0, sd * sd * h as f64)),
        ))
    }
}

/// The random walk plus the past year's average daily return.
pub struct Drift;

impl CloseModel for Drift {
    fn name(&self) -> &'static str {
        "Random walk with drift"
    }

    fn forecast(&self, closes: &[f64], horizon: usize) -> Option<Path> {
        let r = log_returns(closes);
        let (mean, sd) = mean_and_sd(&r[r.len().saturating_sub(LOOKBACK)..])?;
        let last = *closes.last()?;
        Some(normal_path(
            last,
            (1..=horizon).map(|h| (mean * h as f64, sd * sd * h as f64)),
        ))
    }
}

/// The volatility cone: for each horizon h, the quantiles of every h-day
/// return in the history (up to three years), fat tails and all, applied to
/// today's close.
pub struct Cone;

impl CloseModel for Cone {
    fn name(&self) -> &'static str {
        "Volatility cone"
    }

    fn forecast(&self, closes: &[f64], horizon: usize) -> Option<Path> {
        let from = closes.len().saturating_sub(3 * LOOKBACK + 1);
        let c = &closes[from..];
        let last = *closes.last()?;
        (1..=horizon)
            .map(|h| {
                let moves: Vec<f64> = c
                    .windows(h + 1)
                    .filter(|w| w[0] > 0.0 && w[h] > 0.0)
                    .map(|w| (w[h] / w[0]).ln())
                    .collect();
                if moves.len() < 60 {
                    return None;
                }
                let mut q = [0.0; 5];
                for (out, level) in q.iter_mut().zip(QUANTILES) {
                    *out = last * quantile(&moves, level)?.exp();
                }
                Some(q)
            })
            .collect()
    }
}

/// GARCH(1,1): today's variance is a constant plus a share of yesterday's
/// squared surprise plus a share of yesterday's variance, fitted by maximum
/// likelihood. A calm spell narrows the near bands and a turbulent one
/// widens them, each easing back to the long-run level.
pub struct Garch;

/// A fitted GARCH(1,1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GarchFit {
    pub mean: f64,
    pub omega: f64,
    pub alpha: f64,
    pub beta: f64,
    /// The variance expected for the next day.
    pub next: f64,
}

impl GarchFit {
    /// The variance it settles to, ω / (1 − α − β).
    pub fn long_run(&self) -> f64 {
        self.omega / (1.0 - self.alpha - self.beta)
    }

    /// Variance of the return over the next `h` days: the sum of each day's
    /// expected variance, which decays towards the long run at α + β a day.
    pub fn variance_over(&self, h: usize) -> f64 {
        let (lr, p) = (self.long_run(), self.alpha + self.beta);
        (0..h)
            .map(|k| lr + p.powi(k as i32) * (self.next - lr))
            .sum()
    }
}

/// Fit GARCH(1,1) to log returns: Gaussian likelihood, maximised by
/// Nelder–Mead over parameters that keep ω > 0, α, β ≥ 0 and α + β < 1.
pub fn fit_garch(returns: &[f64]) -> Option<GarchFit> {
    let (mean, sd) = mean_and_sd(returns)?;
    if returns.len() < 100 || sd <= 0.0 {
        return None;
    }
    let e: Vec<f64> = returns.iter().map(|r| r - mean).collect();
    let var0 = sd * sd;
    // Parameters: ω = var0·e^x0, α and β a softmax share of (x1, x2, 0).
    // Bounded, so e^x never overflows (which would make β NaN).
    let unpack = |x: &[f64; 3]| {
        let (a, b) = (x[1].clamp(-30.0, 30.0).exp(), x[2].clamp(-30.0, 30.0).exp());
        let total = 1.0 + a + b;
        (var0 * x[0].clamp(-30.0, 30.0).exp(), a / total, b / total)
    };
    let nll = |x: &[f64; 3]| {
        let (omega, alpha, beta) = unpack(x);
        let mut v = var0;
        let mut sum = 0.0;
        for &r in &e {
            sum += v.ln() + r * r / v;
            v = omega + alpha * r * r + beta * v;
            if !v.is_finite() || v <= 0.0 {
                return f64::INFINITY;
            }
        }
        sum / 2.0
    };
    // From α = 0.05, β = 0.9 and ω matching the sample variance.
    // (The shares are α : β : 1 − α − β = 0.05 : 0.9 : 0.05.)
    let start = [(0.05f64).ln(), 0.0, (0.9f64 / 0.05).ln()];
    let x = nelder_mead(nll, start, 600)?;
    let (omega, alpha, beta) = unpack(&x);
    let mut v = var0;
    for &r in &e {
        v = omega + alpha * r * r + beta * v;
    }
    Some(GarchFit {
        mean,
        omega,
        alpha,
        beta,
        next: v,
    })
}

impl CloseModel for Garch {
    fn name(&self) -> &'static str {
        "GARCH(1,1)"
    }

    fn forecast(&self, closes: &[f64], horizon: usize) -> Option<Path> {
        let r = log_returns(closes);
        let fit = fit_garch(&r[r.len().saturating_sub(3 * LOOKBACK)..])?;
        let last = *closes.last()?;
        let path = normal_path(
            last,
            (1..=horizon).map(|h| (fit.mean * h as f64, fit.variance_over(h))),
        );
        path.iter().flatten().all(|v| v.is_finite()).then_some(path)
    }
}

/// Minimise `f` from `start` with the Nelder–Mead simplex method.
fn nelder_mead(
    f: impl Fn(&[f64; 3]) -> f64,
    start: [f64; 3],
    iterations: usize,
) -> Option<[f64; 3]> {
    let mut simplex: Vec<([f64; 3], f64)> = (0..4)
        .map(|i| {
            let mut p = start;
            if i > 0 {
                p[i - 1] += 0.5;
            }
            (p, f(&p))
        })
        .collect();
    let lerp =
        |a: &[f64; 3], b: &[f64; 3], t: f64| std::array::from_fn(|k| a[k] + t * (b[k] - a[k]));
    for _ in 0..iterations {
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        if (simplex[3].1 - simplex[0].1).abs() < 1e-10 {
            break;
        }
        let centroid: [f64; 3] =
            std::array::from_fn(|k| simplex[..3].iter().map(|p| p.0[k]).sum::<f64>() / 3.0);
        let worst = simplex[3];
        let reflected = lerp(&centroid, &worst.0, -1.0);
        let fr = f(&reflected);
        if fr < simplex[0].1 {
            let expanded = lerp(&centroid, &worst.0, -2.0);
            let fe = f(&expanded);
            simplex[3] = if fe < fr {
                (expanded, fe)
            } else {
                (reflected, fr)
            };
        } else if fr < simplex[2].1 {
            simplex[3] = (reflected, fr);
        } else {
            let contracted = lerp(&centroid, &worst.0, 0.5);
            let fc = f(&contracted);
            if fc < worst.1 {
                simplex[3] = (contracted, fc);
            } else {
                let best = simplex[0].0;
                for p in &mut simplex[1..] {
                    p.0 = lerp(&best, &p.0, 0.5);
                    p.1 = f(&p.0);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    simplex[0].1.is_finite().then_some(simplex[0].0)
}

/// Ridge regression of the h-day return on the last day's, week's and
/// month's returns and the month's volatility, one fit per horizon, over up
/// to three years of past origins. The band is the quantiles of its own
/// errors on those origins. Machine learning for a security is offered only
/// where it beats the random walk.
pub struct RidgeReturns;

/// The features at close `o`: 1-, 5- and 20-day log returns and the 20-day
/// volatility of daily returns.
fn return_features(closes: &[f64], o: usize) -> Option<[f64; 4]> {
    if o < 20 || closes[o - 20..=o].iter().any(|c| *c <= 0.0) {
        return None;
    }
    let back = |k: usize| (closes[o] / closes[o - k]).ln();
    let daily = log_returns(&closes[o - 20..=o]);
    let mean = daily.iter().sum::<f64>() / daily.len() as f64;
    let vol = (daily.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / daily.len() as f64).sqrt();
    Some([back(1), back(5), back(20), vol])
}

impl CloseModel for RidgeReturns {
    fn name(&self) -> &'static str {
        "Ridge regression"
    }

    fn learned(&self) -> bool {
        true
    }

    fn forecast(&self, closes: &[f64], horizon: usize) -> Option<Path> {
        let last = closes.len().checked_sub(1)?;
        let now = return_features(closes, last)?;
        let first = last.saturating_sub(3 * LOOKBACK);
        (1..=horizon)
            .map(|h| {
                let (mut x, mut y) = (Vec::new(), Vec::new());
                for o in first..=last.checked_sub(h)? {
                    if let Some(f) = return_features(closes, o) {
                        x.extend(f);
                        y.push((closes[o + h] / closes[o]).ln());
                    }
                }
                if y.len() < 120 {
                    return None;
                }
                let m = Matrix {
                    values: &x,
                    features: 4,
                };
                let model = Ridge::fit(&m, &y)?;
                let errors: Vec<f64> = x
                    .chunks(4)
                    .zip(&y)
                    .map(|(row, actual)| actual - model.predict(row))
                    .collect();
                let point = model.predict(&now);
                let mut q = [0.0; 5];
                for (out, level) in q.iter_mut().zip(QUANTILES) {
                    *out = closes[last] * (point + quantile(&errors, level)?).exp();
                }
                Some(q)
            })
            .collect()
    }
}

/// The models, in the order the Skill tab lists them; the random walk first,
/// as the bar to beat, and the learned one last.
pub fn models() -> Vec<Box<dyn CloseModel>> {
    vec![
        Box::new(RandomWalk),
        Box::new(Drift),
        Box::new(Cone),
        Box::new(Garch),
        Box::new(RidgeReturns),
    ]
}

/// Origins in a security's backtest: the last year, a week apart.
const BACKTEST_ORIGINS: usize = 250;
const BACKTEST_STEP: usize = 5;

/// One model's part in a security's forecast.
#[derive(Clone, Debug, PartialEq)]
pub struct CloseModelForecast {
    pub name: &'static str,
    pub path: Option<Path>,
    pub accuracy: Option<CloseAccuracy>,
    /// Shown: the distribution models always, the learned one only where its
    /// median beats the random walk's.
    pub offered: bool,
}

/// A security's forecast from every model, `horizon` trading days out.
#[derive(Clone, Debug, PartialEq)]
pub struct CloseForecast {
    pub last: f64,
    pub horizon: usize,
    pub models: Vec<CloseModelForecast>,
    /// The distribution model (not the learned one) with the lowest pinball
    /// loss: the best-calibrated, sharpest range. A security's forecast is a
    /// distribution, not a direction; the learned model can still be picked
    /// where it is offered.
    pub default: Option<usize>,
}

/// Forecast and backtest a security's closes (oldest first) with every
/// model.
pub fn forecast_closes(closes: &[f64], horizon: usize) -> CloseForecast {
    let models = models();
    let scores = backtest_closes(&models, closes, horizon, BACKTEST_ORIGINS, BACKTEST_STEP);
    let walk = scores.first().and_then(|s| s.1).map(|a| a.mae_pct);
    let out: Vec<CloseModelForecast> = models
        .iter()
        .zip(scores)
        .map(|(m, (name, accuracy))| CloseModelForecast {
            name,
            path: m.forecast(closes, horizon),
            accuracy,
            offered: !m.learned()
                || matches!((accuracy, walk), (Some(a), Some(w)) if a.mae_pct < w),
        })
        .collect();
    let default = out
        .iter()
        .zip(&models)
        .enumerate()
        .filter(|(_, (m, model))| m.offered && m.path.is_some() && !model.learned())
        .map(|(i, (m, _))| (i, m))
        .filter_map(|(i, m)| Some((i, m.accuracy?.pinball_pct)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
        .or_else(|| out.iter().position(|m| m.offered && m.path.is_some()));
    CloseForecast {
        last: closes.last().copied().unwrap_or_default(),
        horizon,
        models: out,
        default,
    }
}

/// A security model's accuracy over past origins: errors of the median and
/// pinball loss in percent of the price, and band coverage, over every
/// horizon up to the one asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloseAccuracy {
    pub origins: usize,
    pub mae_pct: f64,
    pub pinball_pct: f64,
    pub coverage50: f64,
    pub coverage80: f64,
}

/// Forecast from every `step`-th close of the last `origins_back` that has
/// `horizon` closes after it, and score each model against what happened.
pub fn backtest_closes(
    models: &[Box<dyn CloseModel>],
    closes: &[f64],
    horizon: usize,
    origins_back: usize,
    step: usize,
) -> Vec<(&'static str, Option<CloseAccuracy>)> {
    let last_origin = closes.len().saturating_sub(horizon + 1);
    let first_origin = last_origin.saturating_sub(origins_back);
    models
        .iter()
        .map(|m| {
            let (mut n, mut abs, mut pin, mut in50, mut in80, mut origins) =
                (0usize, 0.0, 0.0, 0, 0, 0);
            for o in (first_origin..=last_origin).step_by(step.max(1)) {
                let Some(path) = m.forecast(&closes[..=o], horizon) else {
                    continue;
                };
                origins += 1;
                for (h, q) in path.iter().enumerate() {
                    let actual = closes[o + 1 + h];
                    let scale = 100.0 / closes[o];
                    abs += (actual - q[2]).abs() * scale;
                    pin += QUANTILES
                        .iter()
                        .zip(q)
                        .map(|(level, at)| pinball_loss(actual, *at, *level))
                        .sum::<f64>()
                        / QUANTILES.len() as f64
                        * scale;
                    in50 += usize::from((q[1]..=q[3]).contains(&actual));
                    in80 += usize::from((q[0]..=q[4]).contains(&actual));
                    n += 1;
                }
            }
            let nf = n as f64;
            (
                m.name(),
                (n > 0).then(|| CloseAccuracy {
                    origins,
                    mae_pct: abs / nf,
                    pinball_pct: pin / nf,
                    coverage50: in50 as f64 / nf,
                    coverage80: in80 as f64 / nf,
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestRng;

    fn walk(n: usize, daily_sd: f64, seed: u64) -> Vec<f64> {
        let mut z = TestRng(seed);
        let mut c = vec![100.0];
        for _ in 1..n {
            let last = *c.last().unwrap();
            c.push(last * (daily_sd * z.normal()).exp());
        }
        c
    }

    #[test]
    fn random_walk_by_hand() {
        // Returns alternate ±r: mean 0, and the sample sd is r·√(n/(n−1)).
        let r = 0.01f64;
        let mut c = vec![100.0];
        for i in 0..40 {
            let last = *c.last().unwrap();
            c.push(last * if i % 2 == 0 { r.exp() } else { (-r).exp() });
        }
        let sd = r * (40.0f64 / 39.0).sqrt();
        let p = RandomWalk.forecast(&c, 4).unwrap();
        assert_eq!(p.len(), 4);
        let last = *c.last().unwrap();
        assert!((p[0][2] - last).abs() < 1e-9, "the median stays put");
        // Four days out the 90th percentile is e^(1.2816·sd·2) above.
        assert!((p[3][4] - last * (Z[4] * sd * 2.0).exp()).abs() < 1e-9);
        assert!(p[3][0] < p[0][0] && p[3][4] > p[0][4], "wider further out");
        assert!(RandomWalk.forecast(&c[..10], 4).is_none(), "too short");
    }

    #[test]
    fn drift_moves_the_median() {
        // A steady 0.1% a day with a little wobble.
        let c: Vec<f64> = (0..300)
            .map(|i| 100.0 * (0.001 * i as f64 + 0.002 * ((i % 2) as f64)).exp())
            .collect();
        let p = Drift.forecast(&c, 10).unwrap();
        let last = *c.last().unwrap();
        assert!(
            p[9][2] > last * 1.005 && p[9][2] < last * 1.015,
            "{}",
            p[9][2] / last
        );
    }

    #[test]
    fn cone_quantiles_are_past_moves() {
        let c = walk(800, 0.01, 7);
        let p = Cone.forecast(&c, 20).unwrap();
        let last = *c.last().unwrap();
        // About 1% a day: twenty days out, the 80% band is roughly ±1.28·√20%.
        let width = (p[19][4] / last).ln() - (p[19][0] / last).ln();
        assert!((0.08..0.16).contains(&width), "{width}");
        assert!(Cone.forecast(&c[..50], 20).is_none());
    }

    #[test]
    fn garch_recovers_its_parameters() {
        // Simulate ω = 0.00001, α = 0.1, β = 0.85 (long-run sd ≈ 1.4% a day).
        let (omega, alpha, beta): (f64, f64, f64) = (1e-5, 0.1, 0.85);
        let mut z = TestRng(42);
        let mut v = omega / (1.0 - alpha - beta);
        let mut returns = Vec::new();
        for _ in 0..4000 {
            let r = v.sqrt() * z.normal();
            returns.push(r);
            v = omega + alpha * r * r + beta * v;
        }
        let fit = fit_garch(&returns).unwrap();
        assert!(
            [fit.omega, fit.alpha, fit.beta, fit.next]
                .iter()
                .all(|v| v.is_finite())
        );
        assert!((fit.alpha - alpha).abs() < 0.05, "{fit:?}");
        assert!((fit.alpha + fit.beta - 0.95).abs() < 0.04, "{fit:?}");
        assert!(
            (fit.long_run() / (omega / 0.05) - 1.0).abs() < 0.35,
            "{fit:?}"
        );
        // The multi-day variance heads to h times the long run.
        let far = fit.variance_over(2000) / 2000.0;
        assert!((far / fit.long_run() - 1.0).abs() < 0.05);
        assert!(fit_garch(&returns[..50]).is_none());
    }

    #[test]
    fn bands_cover_a_random_walk() {
        // On a true random walk, every model's 80% band should hold about 80%.
        let c = walk(900, 0.012, 99);
        let scores = backtest_closes(&models(), &c, 10, 400, 5);
        assert_eq!(scores.len(), 5);
        for (name, a) in scores {
            let a = a.unwrap();
            assert!(a.origins >= 70, "{name}");
            assert!((0.65..=0.93).contains(&a.coverage80), "{name}: {a:?}");
            assert!(a.coverage50 < a.coverage80, "{name}");
            assert!(a.mae_pct > 0.0 && a.pinball_pct > 0.0, "{name}");
        }
    }
}

#[cfg(test)]
mod ridge_tests {
    use super::*;

    #[test]
    fn ridge_finds_momentum_where_there_is_some() {
        // Each day's return is half the day before's plus noise: there is
        // something to learn, and the ridge's median should lean the way the
        // last move went.
        let mut rng = crate::TestRng(5);
        let mut c = vec![100.0];
        let mut r = 0.0f64;
        for _ in 0..700 {
            r = 0.5 * r + 0.01 * rng.normal();
            let last = *c.last().unwrap();
            c.push(last * r.exp());
        }
        let path = RidgeReturns.forecast(&c, 5).unwrap();
        assert_eq!(path.len(), 5);
        let last = *c.last().unwrap();
        let last_move = (c[c.len() - 1] / c[c.len() - 2]).ln();
        assert_eq!(
            path[0][2] > last,
            last_move > 0.0,
            "{} {last_move}",
            path[0][2] / last
        );
        assert!(path.iter().all(|q| q[0] < q[2] && q[2] < q[4]));
        assert!(RidgeReturns.forecast(&c[..100], 5).is_none());
    }
}

#[cfg(test)]
mod forecast_tests {
    use super::*;
    use crate::TestRng;

    #[test]
    fn every_model_and_the_learned_one_only_if_it_wins() {
        // A pure random walk: nothing to learn, so the ridge is not offered.
        let mut z = TestRng(3);
        let mut c = vec![50.0];
        for _ in 0..800 {
            let last = *c.last().unwrap();
            c.push(last * (0.01 * z.normal()).exp());
        }
        let f = forecast_closes(&c, 10);
        assert_eq!(f.models.len(), 5);
        assert_eq!(f.last, *c.last().unwrap());
        assert!(
            f.models
                .iter()
                .all(|m| m.path.as_ref().is_some_and(|p| p.len() == 10))
        );
        let ridge = f
            .models
            .iter()
            .find(|m| m.name == "Ridge regression")
            .unwrap();
        let walk = f.models[0].accuracy.unwrap().mae_pct;
        assert_eq!(ridge.offered, ridge.accuracy.unwrap().mae_pct < walk);
        let chosen = &f.models[f.default.unwrap()];
        assert!(chosen.offered && chosen.name != "Ridge regression");
        // Too short a history: nothing to show.
        let f = forecast_closes(&c[..10], 10);
        assert!(f.default.is_none());
    }
}
