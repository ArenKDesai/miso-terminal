//! NWS parsers against recorded responses for every MISO city.

use std::path::PathBuf;

use mt_data::FixtureTransport;
use mt_nws::parse::{parse_hourly, parse_points};
use mt_nws::{MISO_CITIES, Nws};

fn root() -> PathBuf {
    std::env::var_os("MT_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
}

fn read(url: &str) -> String {
    let path = FixtureTransport::new(root()).path_for(url);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_city_has_a_parseable_forecast() {
    let nws = Nws::default();
    for city in MISO_CITIES {
        let hourly_url = parse_points(&read(&nws.points_url(city))).expect("points");
        assert!(
            hourly_url.starts_with("https://api.weather.gov/gridpoints/"),
            "{hourly_url}"
        );
        let f = parse_hourly(&read(&hourly_url)).unwrap_or_else(|e| panic!("{}: {e}", city.name));
        assert!(
            f.hours.len() >= 48,
            "{}: {} hours",
            city.name,
            f.hours.len()
        );
        assert!(f.updated.is_some());
        for h in &f.hours {
            assert!(
                (-60.0..130.0).contains(&h.temp_f),
                "{}: {} °F",
                city.name,
                h.temp_f
            );
            // Market time is EST: never ahead of the city's local clock.
            assert!(h.time <= h.local + chrono::Duration::hours(1));
        }
        assert!(f.hours.windows(2).all(|w| w[0].time < w[1].time));
        let days = f.daily();
        assert!(days.len() >= 2 && days.iter().all(|d| d.high >= d.low));
    }
}

#[test]
fn points_without_a_forecast_are_an_error() {
    assert!(parse_points(r#"{"properties":{}}"#).is_err());
    assert!(parse_hourly(r#"{"properties":{"periods":[]}}"#).is_err());
}
