//! MISO's forecasts kept as issued (see [`mt_data::issued`]): its load
//! forecast (MTLF) by zone from the daily `df_al` report, its wind and solar
//! forecasts, and its outage schedule.

use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use mt_data::issued::{self, Batch, Source};
use mt_data::{BoxFuture, FetchCtx, FetchError, Query};

use crate::endpoints::reports;
use crate::{Miso, MisoEndpoints, parse};

/// The kinds of forecast stored.
pub const MTLF: &str = "mtlf";
pub const WIND_SOLAR: &str = "windsolar";
pub const OUTAGES: &str = "outages";

/// MISO posts the daily load forecast report at about 01:20 EST. Its versions
/// count as issued at 06:00 on its date: late enough to be safe, early
/// enough for a DA forecast made that morning.
const MTLF_ISSUED_HOUR: u32 = 6;

/// Days of past load forecast reports filled in.
pub const MTLF_BACKFILL_DAYS: u32 = 365;

/// The sources, for [`mt_data::issued::Collector::configure`].
pub fn sources(endpoints: &MisoEndpoints) -> Vec<Arc<dyn Source>> {
    let miso = Miso::new(endpoints.clone());
    vec![
        Arc::new(LoadForecast {
            endpoints: endpoints.clone(),
        }),
        Arc::new(WindSolar { miso: miso.clone() }),
        Arc::new(OutageSchedule { miso }),
    ]
}

fn now() -> NaiveDateTime {
    let t = mt_core::time::now_market();
    t.with_second(0)
        .unwrap_or(t)
        .with_nanosecond(0)
        .unwrap_or(t)
}

/// The daily `df_al` report: MTLF by zone group and for MISO.
struct LoadForecast {
    endpoints: MisoEndpoints,
}

impl LoadForecast {
    async fn report(&self, ctx: &FetchCtx, day: NaiveDate) -> Result<Option<Batch>, FetchError> {
        let url = self.endpoints.report_file(day, reports::DF_AL, "xls");
        // The archive is the record; the workbook itself is not cached.
        let body = match ctx.get_uncached(&url).await {
            Ok(b) => b,
            Err(FetchError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let r = parse::parse_load_forecast_report(&body)?;
        if r.published != day && ctx.is_live() {
            return Err(FetchError::parse(
                "load forecast report",
                format!("the file for {day} says {}", r.published),
            ));
        }
        Ok(Some(Batch {
            kind: MTLF,
            series: r.zones,
            issued: day.and_hms_opt(MTLF_ISSUED_HOUR, 0, 0).unwrap_or_default(),
            rows: r.hours,
        }))
    }
}

impl Source for LoadForecast {
    fn name(&self) -> &'static str {
        "MISO load forecast (MTLF)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(3600)
    }

    /// Today's report, until it is stored (the backfill covers the days
    /// before).
    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let today = now().date();
            let stored = ctx
                .cache()
                .is_some_and(|c| issued::days(c, MTLF).contains(&today));
            if stored || now().hour() < 2 {
                return Ok(Vec::new());
            }
            Ok(self.report(ctx, today).await?.into_iter().collect())
        })
    }

    fn backfill(&self) -> Option<(&'static str, u32)> {
        Some((MTLF, MTLF_BACKFILL_DAYS))
    }

    fn fetch_day<'a>(
        &'a self,
        ctx: &'a FetchCtx,
        day: NaiveDate,
    ) -> BoxFuture<'a, Result<Option<Batch>, FetchError>> {
        Box::pin(self.report(ctx, day))
    }
}

/// Wind and solar forecasts for the rest of today and tomorrow, hourly.
struct WindSolar {
    miso: Miso,
}

impl Source for WindSolar {
    fn name(&self) -> &'static str {
        "MISO wind and solar forecast"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(3600)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let r = self.miso.renewables().fetch(ctx.clone(), None).await?;
            let issued = now();
            let from = issued.with_minute(0).unwrap_or(issued);
            let rows = r
                .hours
                .iter()
                .filter(|h| h.start >= from)
                .filter(|h| h.wind_forecast.is_some() || h.solar_forecast.is_some())
                .map(|h| (h.start, vec![h.wind_forecast, h.solar_forecast]))
                .collect();
            Ok(vec![Batch {
                kind: WIND_SOLAR,
                series: vec!["Wind".into(), "Solar".into()],
                issued,
                rows,
            }])
        })
    }
}

/// Generation outages scheduled for the next five days, MW by kind.
struct OutageSchedule {
    miso: Miso,
}

impl Source for OutageSchedule {
    fn name(&self) -> &'static str {
        "MISO outage schedule"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(3 * 3600)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let o = self.miso.outages().fetch(ctx.clone(), None).await?;
            let issued = now();
            let rows = o
                .days
                .iter()
                .filter(|d| d.day >= issued.date())
                .map(|d| {
                    (
                        d.day.and_hms_opt(0, 0, 0).unwrap_or_default(),
                        vec![
                            Some(d.planned),
                            Some(d.unplanned),
                            Some(d.forced),
                            Some(d.derated),
                        ],
                    )
                })
                .collect();
            Ok(vec![Batch {
                kind: OUTAGES,
                series: ["Planned", "Unplanned", "Forced", "Derated"]
                    .map(String::from)
                    .to_vec(),
                issued,
                rows,
            }])
        })
    }
}
