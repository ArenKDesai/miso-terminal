//! Long hourly price history for one node, from a local archive exported by
//! `tools/export_history.py` (the Energy-Pricing-Journalist DuckDB: DA ex-post
//! and RT LMPs since 2023-01-01). Read instead of downloading daily reports.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime};

use crate::constraints::Market;

const MAGIC: &[u8] = b"MTAH1\0";

/// Hourly DA and RT LMP, MCC and MLC for one node on a contiguous hour range.
#[derive(Clone, Debug, PartialEq)]
pub struct HourlyArchive {
    /// The first hour (hour-beginning, market time).
    pub start: NaiveDateTime,
    /// `[lmp, mcc, mlc]` per hour; NaN where missing.
    pub da: [Vec<f32>; 3],
    pub rt: [Vec<f32>; 3],
}

impl HourlyArchive {
    pub fn hours(&self) -> usize {
        self.da[0].len()
    }

    fn columns(&self, market: Market) -> &[Vec<f32>; 3] {
        match market {
            Market::DayAhead => &self.da,
            Market::RealTime => &self.rt,
        }
    }

    /// The last market day with any price in `market`.
    pub fn last_day(&self, market: Market) -> Option<NaiveDate> {
        let lmp = &self.columns(market)[0];
        let i = lmp.iter().rposition(|v| v.is_finite())?;
        Some((self.start + Duration::hours(i as i64)).date())
    }

    pub fn first_day(&self) -> NaiveDate {
        self.start.date()
    }

    /// `(lmp, mcc, mlc)` for every hour of `day` that has a price.
    pub fn day(&self, market: Market, day: NaiveDate) -> Vec<(NaiveDateTime, f32, f32, f32)> {
        let cols = self.columns(market);
        let Some(midnight) = day.and_hms_opt(0, 0, 0) else {
            return Vec::new();
        };
        let offset = (midnight - self.start).num_hours();
        (0..24)
            .filter_map(|h| {
                let i = usize::try_from(offset + h).ok()?;
                let lmp = *cols[0].get(i)?;
                lmp.is_finite()
                    .then(|| (midnight + Duration::hours(h), lmp, cols[1][i], cols[2][i]))
            })
            .collect()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let n = self.hours();
        let mut out = Vec::with_capacity(MAGIC.len() + 12 + n * 24);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.start.and_utc().timestamp().to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        for col in self.da.iter().chain(&self.rt) {
            for v in col {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Inverse of [`Self::to_bytes`] (the exporter's format); `None` if malformed.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let rest = bytes.strip_prefix(MAGIC)?;
        let start = i64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
        let n = u32::from_le_bytes(rest.get(8..12)?.try_into().ok()?) as usize;
        let body = rest.get(12..)?;
        if body.len() != n.checked_mul(24)? {
            return None;
        }
        let column = |k: usize| -> Vec<f32> {
            body[k * n * 4..(k + 1) * n * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect()
        };
        Some(Self {
            start: DateTime::from_timestamp(start, 0)?.naive_utc(),
            da: [column(0), column(1), column(2)],
            rt: [column(3), column(4), column(5)],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_reads_days() {
        let start = NaiveDate::from_ymd_opt(2026, 9, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let n = 48;
        let ramp: Vec<f32> = (0..n).map(|h| h as f32).collect();
        let mut rt_lmp = ramp.clone();
        for v in &mut rt_lmp[30..] {
            *v = f32::NAN; // RT trails DA
        }
        let a = HourlyArchive {
            start,
            da: [ramp.clone(), vec![1.0; n], vec![0.5; n]],
            rt: [rt_lmp, vec![2.0; n], vec![0.25; n]],
        };
        let b = HourlyArchive::from_bytes(&a.to_bytes()).unwrap();
        assert_eq!(b.hours(), n);
        assert_eq!(b.start, start);
        let day2 = start.date() + Duration::days(1);
        assert_eq!(b.last_day(Market::DayAhead), Some(day2));
        assert_eq!(b.last_day(Market::RealTime), Some(day2));
        assert_eq!(b.day(Market::DayAhead, day2).len(), 24);
        assert_eq!(b.day(Market::RealTime, day2).len(), 6, "hours 24-29 only");
        assert_eq!(b.day(Market::DayAhead, day2)[0].1, 24.0);
        assert!(
            b.day(Market::DayAhead, start.date() - Duration::days(1))
                .is_empty()
        );
        assert!(HourlyArchive::from_bytes(b"MTAH1\0short").is_none());
    }
}
