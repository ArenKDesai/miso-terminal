//! Beta: how a security's returns move with a benchmark's, for BETA.
//!
//! Returns are simple returns from one close to the next, taken on the days
//! both series have ([`paired_returns`]): a day only one of them traded is
//! skipped, so a return can span a gap in either. Weekly returns use each
//! week's last close ([`weekly`]). The fit is ordinary least squares of the
//! security's returns on the benchmark's ([`fit`]); alpha is the intercept,
//! over zero rather than over a risk-free rate, and annualised by multiplying
//! by the periods in a year. Adjusted beta is Blume's: two thirds of the
//! fitted beta plus one third of 1, since betas drift towards 1 over time.

use chrono::{Datelike, NaiveDate};

use crate::equity::Bar;

/// A close on a day, oldest first.
pub type Close = (NaiveDate, f64);

/// How often returns are taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    Daily,
    Weekly,
}

impl Frequency {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
        }
    }

    /// Returns in a year, for annualising alpha.
    pub fn periods_per_year(self) -> f64 {
        match self {
            Self::Daily => 252.0,
            Self::Weekly => 52.0,
        }
    }

    /// Returns in a rolling beta's window: about three months of days, or
    /// half a year of weeks.
    pub fn rolling_window(self) -> usize {
        match self {
            Self::Daily => 63,
            Self::Weekly => 26,
        }
    }

    /// Which returns a window of `years` uses unless asked otherwise: daily
    /// for a year, weekly from two (fewer, less noisy observations, and less
    /// sensitive to the two markets closing at different moments).
    pub fn default_for(years: u32) -> Self {
        if years >= 2 {
            Self::Weekly
        } else {
            Self::Daily
        }
    }
}

/// Daily bars as closes by New York date.
pub fn daily_closes(bars: &[Bar]) -> Vec<Close> {
    bars.iter()
        .map(|b| (crate::exchange::to_exchange(b.time).date_naive(), b.close))
        .collect()
}

/// Each week's last close (weeks run Monday to Sunday), dated by its day.
pub fn weekly(closes: &[Close]) -> Vec<Close> {
    let mut out: Vec<Close> = Vec::new();
    for &(date, close) in closes {
        match out.last_mut() {
            Some(last) if last.0.iso_week() == date.iso_week() => *last = (date, close),
            _ => out.push((date, close)),
        }
    }
    out
}

/// The closes on or after `first`.
pub fn since(closes: &[Close], first: NaiveDate) -> &[Close] {
    &closes[closes.partition_point(|c| c.0 < first)..]
}

/// A security's return and its benchmark's over the same span, ending on
/// `date`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub date: NaiveDate,
    pub security: f64,
    pub benchmark: f64,
}

/// Returns from each date both series have to the next. Closes must be
/// oldest first; a close of zero or less is left out.
pub fn paired_returns(security: &[Close], benchmark: &[Close]) -> Vec<Pair> {
    let (mut i, mut j) = (0, 0);
    let mut prev: Option<(f64, f64)> = None;
    let mut out = Vec::new();
    while i < security.len() && j < benchmark.len() {
        let ((ds, s), (db, b)) = (security[i], benchmark[j]);
        if ds < db || s <= 0.0 {
            i += 1;
        } else if db < ds || b <= 0.0 {
            j += 1;
        } else {
            if let Some((ps, pb)) = prev {
                out.push(Pair {
                    date: ds,
                    security: s / ps - 1.0,
                    benchmark: b / pb - 1.0,
                });
            }
            prev = Some((s, b));
            i += 1;
            j += 1;
        }
    }
    out
}

/// An equal-weighted basket of `members`, rebalanced every day: an index
/// starting at 100 on the first member's first day, moving each day by the
/// average return of the members that traded that day and the one before.
pub fn basket(members: &[&[Close]]) -> Vec<Close> {
    use std::collections::BTreeMap;
    let mut moves: BTreeMap<NaiveDate, (f64, u32)> = BTreeMap::new();
    let mut first: Option<NaiveDate> = None;
    for member in members {
        let closes: Vec<&Close> = member.iter().filter(|c| c.1 > 0.0).collect();
        if let Some(c) = closes.first() {
            first = Some(first.map_or(c.0, |f| f.min(c.0)));
        }
        for w in closes.windows(2) {
            let entry = moves.entry(w[1].0).or_default();
            entry.0 += w[1].1 / w[0].1 - 1.0;
            entry.1 += 1;
        }
    }
    let Some(first) = first else {
        return Vec::new();
    };
    let mut level = 100.0;
    let mut out = vec![(first, level)];
    for (date, (sum, count)) in moves {
        level *= 1.0 + sum / f64::from(count);
        out.push((date, level));
    }
    out
}

/// A least-squares fit of a security's returns on a benchmark's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub observations: usize,
    pub beta: f64,
    /// The intercept, per period (a day or a week), over zero.
    pub alpha: f64,
    pub r_squared: f64,
    pub correlation: f64,
    /// Beta's standard error.
    pub beta_error: f64,
}

impl Fit {
    /// Blume's adjustment: two thirds of beta plus one third of 1.
    pub fn adjusted_beta(&self) -> f64 {
        (2.0 * self.beta + 1.0) / 3.0
    }

    /// Alpha over a year: the per-period intercept times the periods in a
    /// year.
    pub fn annual_alpha(&self, frequency: Frequency) -> f64 {
        self.alpha * frequency.periods_per_year()
    }
}

/// Fit the security's returns to the benchmark's. `None` with fewer than
/// three pairs, or a benchmark that never moved.
pub fn fit(pairs: &[Pair]) -> Option<Fit> {
    let n = pairs.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let mx = pairs.iter().map(|p| p.benchmark).sum::<f64>() / nf;
    let my = pairs.iter().map(|p| p.security).sum::<f64>() / nf;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in pairs {
        let (dx, dy) = (p.benchmark - mx, p.security - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    if sxx <= 0.0 {
        return None;
    }
    let beta = sxy / sxx;
    let correlation = if syy > 0.0 {
        sxy / (sxx * syy).sqrt()
    } else {
        0.0
    };
    // What the line leaves unexplained, over the n − 2 degrees of freedom.
    let residual = (syy - beta * sxy).max(0.0) / (nf - 2.0);
    Some(Fit {
        observations: n,
        beta,
        alpha: my - beta * mx,
        r_squared: correlation * correlation,
        correlation,
        beta_error: (residual / sxx).sqrt(),
    })
}

/// Beta over each run of `window` pairs, dated by the run's last pair.
pub fn rolling(pairs: &[Pair], window: usize) -> Vec<(NaiveDate, f64)> {
    if window < 3 {
        return Vec::new();
    }
    pairs
        .windows(window)
        .filter_map(|w| Some((w[window - 1].date, fit(w)?.beta)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn pairs(xy: &[(f64, f64)]) -> Vec<Pair> {
        xy.iter()
            .enumerate()
            .map(|(i, &(benchmark, security))| Pair {
                date: day("2026-01-01") + chrono::Duration::days(i as i64),
                security,
                benchmark,
            })
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn fit_worked_by_hand() {
        // x 1..5, y 2 4 5 4 5: means 3 and 4, Sxx 10, Sxy 6, Syy 6. Beta 0.6,
        // alpha 4 − 0.6·3 = 2.2, r = 6/√60, R² 0.6, residual 6 − 0.6·6 = 2.4
        // over 3 degrees of freedom = 0.8, standard error √(0.8/10).
        let f = fit(&pairs(&[
            (1.0, 2.0),
            (2.0, 4.0),
            (3.0, 5.0),
            (4.0, 4.0),
            (5.0, 5.0),
        ]))
        .unwrap();
        assert_eq!(f.observations, 5);
        assert!(close(f.beta, 0.6) && close(f.alpha, 2.2), "{f:?}");
        assert!(close(f.correlation, 6.0 / 60f64.sqrt()));
        assert!(close(f.r_squared, 0.6));
        assert!(close(f.beta_error, 0.08f64.sqrt()));
        // Two thirds of 0.6 plus one third.
        assert!(close(f.adjusted_beta(), 0.4 + 1.0 / 3.0));
        assert!(close(f.annual_alpha(Frequency::Weekly), 2.2 * 52.0));
    }

    #[test]
    fn a_perfect_line_has_no_error() {
        // y = 0.01 + 2x exactly.
        let f = fit(&pairs(&[
            (0.01, 0.03),
            (-0.02, -0.03),
            (0.03, 0.07),
            (0.0, 0.01),
        ]))
        .unwrap();
        assert!(close(f.beta, 2.0) && close(f.alpha, 0.01), "{f:?}");
        assert!(close(f.correlation, 1.0) && close(f.r_squared, 1.0));
        // Only rounding is left over.
        assert!(f.beta_error < 1e-6, "{}", f.beta_error);
        // Moving against the benchmark: a negative correlation.
        let f = fit(&pairs(&[(0.01, -0.01), (0.02, -0.02), (-0.01, 0.01)])).unwrap();
        assert!(close(f.beta, -1.0) && close(f.correlation, -1.0) && close(f.r_squared, 1.0));
    }

    #[test]
    fn too_little_to_fit() {
        assert_eq!(fit(&pairs(&[(0.01, 0.02), (0.02, 0.01)])), None);
        // A benchmark that never moved says nothing about beta.
        assert_eq!(fit(&pairs(&[(0.0, 0.02), (0.0, 0.01), (0.0, 0.03)])), None);
        // A security that never moved has a beta of 0.
        let f = fit(&pairs(&[(0.01, 0.0), (0.02, 0.0), (-0.01, 0.0)])).unwrap();
        assert!(close(f.beta, 0.0) && close(f.correlation, 0.0));
    }

    #[test]
    fn returns_pair_on_shared_days() {
        let security = [
            (day("2026-09-28"), 100.0),
            (day("2026-09-29"), 110.0),
            (day("2026-09-30"), 99.0),
            (day("2026-10-01"), 108.9),
        ];
        // The benchmark has no bar on the 30th, so the last return spans it
        // for both: 108.9/110 − 1 and 60.5/55 − 1.
        let benchmark = [
            (day("2026-09-27"), 1.0),
            (day("2026-09-28"), 50.0),
            (day("2026-09-29"), 55.0),
            (day("2026-10-01"), 60.5),
        ];
        let got = paired_returns(&security, &benchmark);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].date, day("2026-09-29"));
        assert!(close(got[0].security, 0.1) && close(got[0].benchmark, 0.1));
        assert_eq!(got[1].date, day("2026-10-01"));
        assert!(close(got[1].security, -0.01) && close(got[1].benchmark, 0.1));
        // A close of zero is left out rather than divided by.
        let got = paired_returns(
            &[
                (day("2026-09-28"), 0.0),
                (day("2026-09-29"), 10.0),
                (day("2026-09-30"), 11.0),
            ],
            &[
                (day("2026-09-28"), 5.0),
                (day("2026-09-29"), 5.0),
                (day("2026-09-30"), 6.0),
            ],
        );
        assert_eq!(got.len(), 1);
        assert!(close(got[0].security, 0.1) && close(got[0].benchmark, 0.2));
        assert!(paired_returns(&security, &[]).is_empty());
    }

    #[test]
    fn weeks_end_on_their_last_close() {
        // Monday 2026-09-28 to Friday 10-02, then Monday 10-05 to Thursday 10-08.
        let closes: Vec<Close> = (0..9)
            .map(|i| {
                let d = day("2026-09-28") + chrono::Duration::days(i + i / 5 * 2);
                (d, i as f64)
            })
            .collect();
        assert_eq!(closes[5].0, day("2026-10-05"));
        assert_eq!(
            weekly(&closes),
            [(day("2026-10-02"), 4.0), (day("2026-10-08"), 8.0)]
        );
        // The same week number a year apart is a different week.
        assert_eq!(
            weekly(&[(day("2025-10-01"), 1.0), (day("2026-09-30"), 2.0)]).len(),
            2
        );
        assert!(weekly(&[]).is_empty());
    }

    #[test]
    fn basket_averages_the_members_that_moved() {
        // A rises 10% twice; B falls 10%, misses a day, then rises 10%.
        let a = [
            (day("2026-09-28"), 100.0),
            (day("2026-09-29"), 110.0),
            (day("2026-09-30"), 121.0),
        ];
        let b = [
            (day("2026-09-28"), 50.0),
            (day("2026-09-29"), 45.0),
            (day("2026-10-01"), 49.5),
        ];
        let got = basket(&[&a, &b]);
        let want = [
            (day("2026-09-28"), 100.0),
            // (10% − 10%) / 2 = 0.
            (day("2026-09-29"), 100.0),
            // Only A moved: +10%.
            (day("2026-09-30"), 110.0),
            // Only B: +10% from its last close.
            (day("2026-10-01"), 121.0),
        ];
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want) {
            assert!(g.0 == w.0 && close(g.1, w.1), "{g:?} != {w:?}");
        }
        assert!(basket(&[]).is_empty());
        assert!(basket(&[&[]]).is_empty());
    }

    #[test]
    fn rolling_beta_dates_each_window_by_its_end() {
        // y = 2x throughout, so every window's beta is 2.
        let p = pairs(&[
            (0.01, 0.02),
            (0.02, 0.04),
            (-0.01, -0.02),
            (0.03, 0.06),
            (0.0, 0.0),
        ]);
        let got = rolling(&p, 3);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].0, p[2].date);
        assert!(got.iter().all(|(_, b)| close(*b, 2.0)));
        assert!(rolling(&p, 6).is_empty());
        assert!(rolling(&p, 2).is_empty());
    }

    #[test]
    fn frequencies() {
        assert_eq!(Frequency::default_for(1), Frequency::Daily);
        assert_eq!(Frequency::default_for(2), Frequency::Weekly);
        assert_eq!(Frequency::default_for(5), Frequency::Weekly);
        let closes = [(day("2026-01-02"), 1.0), (day("2026-02-02"), 2.0)];
        assert_eq!(since(&closes, day("2026-01-03")), &closes[1..]);
        assert_eq!(since(&closes, day("2026-03-01")), &[]);
    }
}
