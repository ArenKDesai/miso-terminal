//! The National Weather Service (api.weather.gov) as a data source: hourly
//! forecasts for one city per MISO local resource zone.
//!
//! This crate is the template for a non-MISO source: it depends only on
//! `mt-core` (for the types) and `mt-data` (for `Query`), and the hub, cache,
//! polite interval and LOG function apply to it unchanged.

pub mod issued;
pub mod parse;

use std::sync::Arc;
use std::time::Duration;

use mt_core::CityForecast;
use mt_data::{FetchCtx, FetchError, Freshness, Query};

/// A forecast location.
#[derive(Debug, PartialEq)]
pub struct City {
    pub name: &'static str,
    pub state: &'static str,
    /// The MISO local resource zone it stands in for.
    pub zone: &'static str,
    pub lat: f64,
    pub lon: f64,
}

impl City {
    pub fn label(&self) -> String {
        format!("{}, {}", self.name, self.state)
    }
}

/// One load centre per MISO local resource zone (LRZ 1-10), plus MISO Texas.
pub const MISO_CITIES: &[City] = &[
    City {
        name: "Minneapolis",
        state: "MN",
        zone: "LRZ 1",
        lat: 44.9778,
        lon: -93.2650,
    },
    City {
        name: "Madison",
        state: "WI",
        zone: "LRZ 2",
        lat: 43.0731,
        lon: -89.4012,
    },
    City {
        name: "Des Moines",
        state: "IA",
        zone: "LRZ 3",
        lat: 41.5868,
        lon: -93.6250,
    },
    City {
        name: "Springfield",
        state: "IL",
        zone: "LRZ 4",
        lat: 39.7817,
        lon: -89.6501,
    },
    City {
        name: "St. Louis",
        state: "MO",
        zone: "LRZ 5",
        lat: 38.6270,
        lon: -90.1994,
    },
    City {
        name: "Indianapolis",
        state: "IN",
        zone: "LRZ 6",
        lat: 39.7684,
        lon: -86.1581,
    },
    City {
        name: "Detroit",
        state: "MI",
        zone: "LRZ 7",
        lat: 42.3314,
        lon: -83.0458,
    },
    City {
        name: "Little Rock",
        state: "AR",
        zone: "LRZ 8",
        lat: 34.7465,
        lon: -92.2896,
    },
    City {
        name: "New Orleans",
        state: "LA",
        zone: "LRZ 9",
        lat: 29.9511,
        lon: -90.0715,
    },
    City {
        name: "Jackson",
        state: "MS",
        zone: "LRZ 10",
        lat: 32.2988,
        lon: -90.1848,
    },
    City {
        name: "Beaumont",
        state: "TX",
        zone: "LRZ 9 (TX)",
        lat: 30.0802,
        lon: -94.1266,
    },
];

/// Forecasts change a few times a day; NWS asks clients not to poll hard.
pub const FORECAST_REFRESH: Duration = Duration::from_secs(30 * 60);

/// Entry point for building NWS queries.
#[derive(Clone, Debug)]
pub struct Nws {
    base: Arc<str>,
}

impl Default for Nws {
    fn default() -> Self {
        Self {
            base: "https://api.weather.gov".into(),
        }
    }
}

impl Nws {
    /// `https://api.weather.gov/points/43.0731,-89.4012`
    pub fn points_url(&self, city: &City) -> String {
        format!(
            "{}/points/{:.4},{:.4}",
            self.base.trim_end_matches('/'),
            city.lat,
            city.lon
        )
    }

    pub fn hourly(&self, city: &'static City) -> HourlyQuery {
        HourlyQuery {
            nws: self.clone(),
            city,
        }
    }
}

/// The hourly forecast for one city.
#[derive(Clone, Debug)]
pub struct HourlyQuery {
    nws: Nws,
    pub city: &'static City,
}

impl Query for HourlyQuery {
    type Output = CityForecast;

    fn key(&self) -> String {
        format!("nws/hourly/{:.4},{:.4}", self.city.lat, self.city.lon)
    }

    fn label(&self) -> String {
        format!("Weather, {}", self.city.label())
    }

    fn freshness(&self, _: &CityForecast) -> Freshness {
        Freshness::Every(FORECAST_REFRESH)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<CityForecast>>,
    ) -> Result<CityForecast, FetchError> {
        // The points lookup maps a location to its forecast grid; it practically
        // never changes, so it is cached on disk like a settled report.
        let points = ctx
            .get_text_immutable(&self.nws.points_url(self.city))
            .await?;
        let url = parse::parse_points(&points)?;
        parse::parse_hourly(&ctx.get_text(&url).await?)
    }
}
