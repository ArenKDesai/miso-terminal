//! Weather: hourly forecasts for MISO load centres. Temperature drives load,
//! so these sit next to the market data.

use chrono::{NaiveDate, NaiveDateTime};

/// One forecast hour.
#[derive(Clone, Debug, PartialEq)]
pub struct WxHour {
    /// Start of the hour in market time (EST), for lining up with prices.
    pub time: NaiveDateTime,
    /// Start of the hour in the city's own local time, for "today" and "tomorrow".
    pub local: NaiveDateTime,
    pub temp_f: f64,
    pub dewpoint_f: Option<f64>,
    pub humidity_pct: Option<f64>,
    /// Upper end of the forecast wind speed, mph.
    pub wind_mph: Option<f64>,
    pub precip_pct: Option<f64>,
    pub short: String,
}

/// An hourly forecast for one place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CityForecast {
    /// When the forecaster last updated it (market time).
    pub updated: Option<NaiveDateTime>,
    pub hours: Vec<WxHour>,
}

/// Forecast high and low for one local day, °F.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayRange {
    pub day: NaiveDate,
    pub high: f64,
    pub low: f64,
}

impl CityForecast {
    /// The hour in effect now (the first one the forecast still covers).
    pub fn current(&self) -> Option<&WxHour> {
        self.hours.first()
    }

    /// Highs and lows per local day, in order. The first day is partial.
    pub fn daily(&self) -> Vec<DayRange> {
        let mut out: Vec<DayRange> = Vec::new();
        for h in &self.hours {
            let day = h.local.date();
            match out.last_mut() {
                Some(d) if d.day == day => {
                    d.high = d.high.max(h.temp_f);
                    d.low = d.low.min(h.temp_f);
                }
                _ => out.push(DayRange {
                    day,
                    high: h.temp_f,
                    low: h.temp_f,
                }),
            }
        }
        out
    }
}

/// Fahrenheit from Celsius.
pub fn c_to_f(c: f64) -> f64 {
    c * 9.0 / 5.0 + 32.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hour(d: u32, h: u32, temp: f64) -> WxHour {
        let t = NaiveDate::from_ymd_opt(2026, 10, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap();
        WxHour {
            time: t,
            local: t,
            temp_f: temp,
            dewpoint_f: None,
            humidity_pct: None,
            wind_mph: None,
            precip_pct: None,
            short: String::new(),
        }
    }

    #[test]
    fn daily_ranges_follow_local_days() {
        let f = CityForecast {
            updated: None,
            hours: vec![
                hour(2, 22, 50.0),
                hour(2, 23, 48.0),
                hour(3, 0, 47.0),
                hour(3, 15, 70.0),
            ],
        };
        let d = f.daily();
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].high, d[0].low), (50.0, 48.0));
        assert_eq!((d[1].high, d[1].low), (70.0, 47.0));
        assert_eq!(f.current().map(|h| h.temp_f), Some(50.0));
        assert!((c_to_f(100.0) - 212.0).abs() < 1e-9);
    }
}
