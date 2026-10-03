//! NWS GeoJSON responses -> `mt-core` weather types. Pure and fixture-tested.

use chrono::{DateTime, FixedOffset};
use mt_core::time::to_market;
use mt_core::{CityForecast, WxHour, c_to_f};
use mt_data::FetchError;
use serde::Deserialize;

fn json<'a, T: Deserialize<'a>>(what: &str, body: &'a str) -> Result<T, FetchError> {
    serde_json::from_str(body).map_err(|e| FetchError::parse(what, e))
}

#[derive(Deserialize)]
struct PointsRaw {
    properties: PointsProps,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PointsProps {
    forecast_hourly: Option<String>,
}

/// The hourly-forecast URL for a `/points/{lat},{lon}` response.
pub fn parse_points(body: &str) -> Result<String, FetchError> {
    let raw: PointsRaw = json("NWS points", body)?;
    raw.properties
        .forecast_hourly
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            FetchError::parse(
                "NWS points",
                "no forecastHourly URL (location outside NWS coverage?)",
            )
        })
}

#[derive(Deserialize)]
struct HourlyRaw {
    properties: HourlyProps,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HourlyProps {
    update_time: Option<String>,
    #[serde(default)]
    periods: Vec<PeriodRaw>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeriodRaw {
    start_time: String,
    temperature: Option<f64>,
    temperature_unit: Option<String>,
    probability_of_precipitation: Option<Measure>,
    dewpoint: Option<Measure>,
    relative_humidity: Option<Measure>,
    wind_speed: Option<String>,
    #[serde(default)]
    short_forecast: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Measure {
    unit_code: Option<String>,
    value: Option<f64>,
}

impl Measure {
    /// Temperature in °F whatever unit NWS used.
    fn fahrenheit(&self) -> Option<f64> {
        let v = self.value?;
        Some(
            if self
                .unit_code
                .as_deref()
                .is_some_and(|u| u.ends_with("degC"))
            {
                c_to_f(v)
            } else {
                v
            },
        )
    }
}

/// The largest number in "5 to 10 mph" style strings.
fn wind_mph(s: &str) -> Option<f64> {
    s.split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter_map(|t| t.parse::<f64>().ok())
        .max_by(f64::total_cmp)
}

fn rfc3339(s: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(s).ok()
}

/// Parse a `/gridpoints/.../forecast/hourly` response.
pub fn parse_hourly(body: &str) -> Result<CityForecast, FetchError> {
    let raw: HourlyRaw = json("NWS hourly forecast", body)?;
    let hours = raw
        .properties
        .periods
        .into_iter()
        .filter_map(|p| {
            let start = rfc3339(&p.start_time)?;
            let temp = p.temperature?;
            let temp_f = if p.temperature_unit.as_deref() == Some("C") {
                c_to_f(temp)
            } else {
                temp
            };
            Some(WxHour {
                time: to_market(start.to_utc()),
                local: start.naive_local(),
                temp_f,
                dewpoint_f: p.dewpoint.as_ref().and_then(Measure::fahrenheit),
                humidity_pct: p.relative_humidity.and_then(|m| m.value),
                wind_mph: p.wind_speed.as_deref().and_then(wind_mph),
                precip_pct: p.probability_of_precipitation.and_then(|m| m.value),
                short: p.short_forecast,
            })
        })
        .collect::<Vec<_>>();
    if hours.is_empty() {
        return Err(FetchError::parse("NWS hourly forecast", "no periods"));
    }
    Ok(CityForecast {
        updated: raw
            .properties
            .update_time
            .as_deref()
            .and_then(rfc3339)
            .map(|t| to_market(t.to_utc())),
        hours,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_takes_the_upper_bound() {
        assert_eq!(wind_mph("5 to 10 mph"), Some(10.0));
        assert_eq!(wind_mph("0 mph"), Some(0.0));
        assert_eq!(wind_mph(""), None);
    }
}
