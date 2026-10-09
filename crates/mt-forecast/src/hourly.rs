//! Hourly series in market time (EST, hour beginning), with the gaps a price
//! history has: a day not stored, an hour MISO did not publish.

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate, NaiveDateTime};

/// Values by hour beginning, market time. A missing hour is absent.
pub type Hourly = BTreeMap<NaiveDateTime, f64>;

/// A day's 24 values, hour beginning 0 to 23.
pub type Day = [f64; 24];

/// An [`Hourly`] from points, keeping only finite values.
pub fn hourly(points: impl IntoIterator<Item = (NaiveDateTime, f64)>) -> Hourly {
    points.into_iter().filter(|p| p.1.is_finite()).collect()
}

/// The start of hour `hour` (0 to 23) of `day`.
pub fn hour_start(day: NaiveDate, hour: usize) -> NaiveDateTime {
    day.and_hms_opt(0, 0, 0).unwrap_or_default() + Duration::hours(hour as i64)
}

/// A day's values, hour by hour.
pub fn day_values(series: &Hourly, day: NaiveDate) -> [Option<f64>; 24] {
    std::array::from_fn(|h| series.get(&hour_start(day, h)).copied())
}

/// A day's values if every hour is there.
pub fn full_day(series: &Hourly, day: NaiveDate) -> Option<Day> {
    let v = day_values(series, day);
    v.iter()
        .all(Option::is_some)
        .then(|| v.map(Option::unwrap_or_default))
}

/// The `hours` hours before `until`, oldest first, with gaps filled from the
/// same hour a day earlier (else a week earlier, else the hour before).
/// `None` if more than a tenth are missing, or the first is.
pub fn contiguous(series: &Hourly, until: NaiveDateTime, hours: usize) -> Option<Vec<f64>> {
    let start = until - Duration::hours(hours as i64);
    let mut out: Vec<f64> = Vec::with_capacity(hours);
    let mut missing = 0;
    for i in 0..hours {
        let t = start + Duration::hours(i as i64);
        let v = match series.get(&t) {
            Some(v) => *v,
            None => {
                missing += 1;
                let back = |k: usize| i.checked_sub(k).map(|j| out[j]);
                back(24).or_else(|| back(168)).or_else(|| back(1))?
            }
        };
        out.push(v);
    }
    (missing * 10 <= hours).then_some(out)
}

/// Clamp values to their `p` and `1 − p` quantiles: one scarcity hour should
/// not drag a smoothing model for days.
pub fn winsorize(values: &mut [f64], p: f64) {
    let (Some(lo), Some(hi)) = (quantile(values, p), quantile(values, 1.0 - p)) else {
        return;
    };
    for v in values {
        *v = v.clamp(lo, hi);
    }
}

/// The `q` quantile (0 to 1) by linear interpolation between order
/// statistics; `None` for no finite values.
pub fn quantile(values: &[f64], q: f64) -> Option<f64> {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let pos = q.clamp(0.0, 1.0) * (v.len() - 1) as f64;
    let (i, frac) = (pos.floor() as usize, pos.fract());
    let next = v.get(i + 1).copied().unwrap_or(v[i]);
    Some(v[i] + frac * (next - v[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn days_and_gaps() {
        let d = day("2026-10-01");
        let mut s = hourly((0..48).map(|h| (hour_start(d, h), h as f64)));
        s.insert(hour_start(d, 3), f64::NAN);
        assert_eq!(hourly([(hour_start(d, 0), f64::NAN)]).len(), 0);
        assert_eq!(full_day(&s, d).map(|v| v[5]), Some(5.0));
        assert_eq!(full_day(&s, day("2026-10-02")).map(|v| v[0]), Some(24.0));
        s.remove(&hour_start(d, 7));
        assert_eq!(full_day(&s, d), None);
        assert_eq!(day_values(&s, d)[7], None);
    }

    #[test]
    fn contiguous_fills_from_the_day_before() {
        let d = day("2026-10-01");
        let mut s = hourly((0..48).map(|h| (hour_start(d, h), h as f64)));
        // Hour 30 is missing: filled from hour 6, a day earlier.
        s.remove(&hour_start(d, 30));
        let v = contiguous(&s, hour_start(d, 48), 48).unwrap();
        assert_eq!(v.len(), 48);
        assert_eq!(v[30], 6.0);
        // With the first hour missing there is nothing to fill from.
        s.remove(&hour_start(d, 0));
        assert_eq!(contiguous(&s, hour_start(d, 48), 48), None);
        // More than a tenth missing.
        let sparse = hourly((0..48).step_by(2).map(|h| (hour_start(d, h), 1.0)));
        assert_eq!(contiguous(&sparse, hour_start(d, 48), 48), None);
    }

    #[test]
    fn quantiles_and_winsorizing() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(quantile(&v, 0.5), Some(3.0));
        assert_eq!(quantile(&v, 0.25), Some(2.0));
        // Between the 4th and 5th order statistics: 4 + 0.6.
        assert!((quantile(&v, 0.9).unwrap() - 4.6).abs() < 1e-12);
        assert_eq!(quantile(&[], 0.5), None);
        let mut w = [0.0, 1.0, 2.0, 3.0, 1000.0];
        winsorize(&mut w, 0.25);
        assert_eq!(w, [1.0, 1.0, 2.0, 3.0, 3.0]);
    }
}
