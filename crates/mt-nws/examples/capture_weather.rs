//! Record NWS responses for every MISO city into `fixtures/` (offline mode and
//! parser tests).
//!
//!     cargo run -p mt-nws --example capture_weather
//!
//! Two requests per city. Responses are trimmed to the fields the parsers read
//! and to 72 forecast hours, to keep the repository small.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mt_data::{EventLog, FetchCtx, FetchCtxOptions, FixtureTransport, HttpTransport};
use mt_nws::{MISO_CITIES, Nws};
use serde_json::{Value, json};

const HOURS: usize = 72;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures").to_owned()),
    );
    let ctx = FetchCtx::new(
        Arc::new(HttpTransport::new(
            "MISO-Terminal fixture recorder (github.com/ArenKDesai/miso-terminal)",
        )?),
        None,
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let fixtures = FixtureTransport::new(&out);
    let nws = Nws::default();
    for city in MISO_CITIES {
        let points_url = nws.points_url(city);
        let points: Value = serde_json::from_str(&ctx.get_text(&points_url).await?)?;
        let p = &points["properties"];
        let hourly_url = p["forecastHourly"]
            .as_str()
            .ok_or("no forecastHourly")?
            .to_owned();
        let trimmed = json!({ "properties": {
            "forecastHourly": hourly_url,
            "gridId": p["gridId"], "gridX": p["gridX"], "gridY": p["gridY"],
            "relativeLocation": { "properties": p["relativeLocation"]["properties"] },
        }});
        write(&fixtures.path_for(&points_url), &trimmed)?;

        let hourly: Value = serde_json::from_str(&ctx.get_text(&hourly_url).await?)?;
        let periods: Vec<Value> = hourly["properties"]["periods"]
            .as_array()
            .map(|a| a.iter().take(HOURS).map(trim_period).collect())
            .unwrap_or_default();
        let trimmed = json!({ "properties": {
            "updateTime": hourly["properties"]["updateTime"],
            "periods": periods,
        }});
        write(&fixtures.path_for(&hourly_url), &trimmed)?;
    }
    Ok(())
}

fn trim_period(p: &Value) -> Value {
    let keep = [
        "startTime",
        "temperature",
        "temperatureUnit",
        "probabilityOfPrecipitation",
        "dewpoint",
        "relativeHumidity",
        "windSpeed",
        "shortForecast",
    ];
    Value::Object(
        keep.iter()
            .map(|k| ((*k).to_owned(), p[*k].clone()))
            .collect(),
    )
}

fn write(path: &Path, v: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string(v)?;
    std::fs::write(path, &body)?;
    println!("{:>6} KB  {}", body.len() / 1024, path.display());
    Ok(())
}
