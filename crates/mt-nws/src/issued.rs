//! The NWS's hourly temperature forecasts for the city standing in for each
//! MISO zone, kept as issued (see [`mt_data::issued`]) for FCST's models.

use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;
use mt_data::issued::{Batch, Source};
use mt_data::{BoxFuture, FetchCtx, FetchError, Query};

use crate::{MISO_CITIES, Nws};

/// The kind stored.
pub const TEMPERATURES: &str = "temperatures";

/// The source, for [`mt_data::issued::Collector::configure`].
pub fn sources(nws: &Nws) -> Vec<Arc<dyn Source>> {
    vec![Arc::new(Temperatures { nws: nws.clone() })]
}

/// Hourly temperatures, °F, a week ahead, one series per city.
struct Temperatures {
    nws: Nws,
}

impl Source for Temperatures {
    fn name(&self) -> &'static str {
        "NWS temperature forecasts"
    }

    /// The NWS updates its grids a few times a day and asks clients not to
    /// poll hard.
    fn every(&self) -> Duration {
        Duration::from_secs(3 * 3600)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let now = mt_core::time::now_market();
            let issued = now
                .with_second(0)
                .unwrap_or(now)
                .with_nanosecond(0)
                .unwrap_or(now);
            let from = issued.with_minute(0).unwrap_or(issued);
            let series: Vec<String> = MISO_CITIES.iter().map(|c| c.label()).collect();
            let mut by_hour: std::collections::BTreeMap<_, Vec<Option<f64>>> =
                std::collections::BTreeMap::new();
            let mut failed = None;
            for (i, city) in MISO_CITIES.iter().enumerate() {
                match self.nws.hourly(city).fetch(ctx.clone(), None).await {
                    Ok(f) => {
                        for h in f.hours.iter().filter(|h| h.time >= from) {
                            by_hour
                                .entry(h.time)
                                .or_insert_with(|| vec![None; series.len()])[i] = Some(h.temp_f);
                        }
                    }
                    // One city's grid can be briefly down; keep the others.
                    Err(e) => failed = Some(e),
                }
            }
            if by_hour.is_empty()
                && let Some(e) = failed
            {
                return Err(e);
            }
            Ok(vec![Batch {
                kind: TEMPERATURES,
                series,
                issued,
                rows: by_hour.into_iter().collect(),
            }])
        })
    }
}
