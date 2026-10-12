//! MISO's operating feeds kept by day: what the system did, from feeds MISO
//! publishes for the current day (or the last two hours) only, so that a past
//! day can be looked at later. Kept with the forecasts as issued
//! ([`mt_data::issued`], the same file format, window and switch), one kind
//! per feed; each value is stored once, when it first appears or changes.
//!
//! Only what the terminal saw while it ran is kept: a day it was closed has
//! gaps. Load, reserve prices and constraints have MISO's own daily reports
//! for any day; these do not.

use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDateTime, Timelike};
use mt_data::issued::{Batch, Source};
use mt_data::{BoxFuture, FetchCtx, FetchError, Query};

use crate::{Miso, MisoEndpoints};

pub const FUEL_MIX: &str = "ops-fuelmix";
pub const LOAD: &str = "ops-load";
pub const INTERCHANGE: &str = "ops-nsi";
pub const ACE: &str = "ops-ace";
pub const TRANSFER: &str = "ops-transfer";
pub const CAPACITY: &str = "ops-capacity";

/// Every kind, for readers.
pub const KINDS: [&str; 6] = [FUEL_MIX, LOAD, INTERCHANGE, ACE, TRANSFER, CAPACITY];

/// The sources, for [`mt_data::issued::Collector::configure`].
pub fn sources(endpoints: &MisoEndpoints) -> Vec<Arc<dyn Source>> {
    let miso = Miso::new(endpoints.clone());
    vec![
        Arc::new(FuelMixSource(miso.clone())),
        Arc::new(LoadSource(miso.clone())),
        Arc::new(InterchangeSource(miso.clone())),
        // The ACE feed holds two hours: ask well inside that.
        Arc::new(AceSource(miso.clone())),
        Arc::new(TransferSource(miso.clone())),
        Arc::new(CapacitySource(miso)),
    ]
}

fn now() -> NaiveDateTime {
    let t = mt_core::time::now_market();
    t.with_second(0)
        .unwrap_or(t)
        .with_nanosecond(0)
        .unwrap_or(t)
}

fn series(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

struct FuelMixSource(Miso);

impl Source for FuelMixSource {
    fn name(&self) -> &'static str {
        "MISO fuel mix (kept by day)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let h = self.0.fuel_mix_today().fetch(ctx.clone(), None).await?;
            let mut names: Vec<String> = Vec::new();
            for m in &h.intervals {
                for (fuel, _) in &m.fuels {
                    if !names.contains(fuel) {
                        names.push(fuel.clone());
                    }
                }
            }
            let rows = h
                .intervals
                .iter()
                .filter_map(|m| {
                    let t = m.interval?;
                    let values = names
                        .iter()
                        .map(|n| m.fuels.iter().find(|(f, _)| f == n).map(|(_, v)| *v))
                        .collect();
                    Some((t, values))
                })
                .collect();
            Ok(vec![Batch {
                kind: FUEL_MIX,
                series: names,
                issued: now(),
                rows,
            }])
        })
    }
}

struct LoadSource(Miso);

impl Source for LoadSource {
    fn name(&self) -> &'static str {
        "MISO load, five-minute (kept by day)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let l = self.0.load().fetch(ctx.clone(), None).await?;
            Ok(vec![Batch {
                kind: LOAD,
                series: series(&["Load"]),
                issued: now(),
                rows: l
                    .actual_5min
                    .iter()
                    .map(|(t, v)| (*t, vec![Some(*v)]))
                    .collect(),
            }])
        })
    }
}

struct InterchangeSource(Miso);

impl Source for InterchangeSource {
    fn name(&self) -> &'static str {
        "MISO scheduled interchange (kept by day)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let h = self
                .0
                .interchange_history()
                .fetch(ctx.clone(), None)
                .await?;
            let mut names: Vec<String> = Vec::new();
            for p in &h.points {
                for (ba, _) in &p.by_ba {
                    if !names.contains(ba) {
                        names.push(ba.clone());
                    }
                }
            }
            let rows = h
                .points
                .iter()
                .filter_map(|p| {
                    let t = p.time?;
                    let values = names
                        .iter()
                        .map(|n| p.by_ba.iter().find(|(b, _)| b == n).map(|(_, v)| *v))
                        .collect();
                    Some((t, values))
                })
                .collect();
            Ok(vec![Batch {
                kind: INTERCHANGE,
                series: names,
                issued: now(),
                rows,
            }])
        })
    }
}

struct AceSource(Miso);

impl Source for AceSource {
    fn name(&self) -> &'static str {
        "MISO area control error (kept by day)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let a = self.0.ace().fetch(ctx.clone(), None).await?;
            Ok(vec![Batch {
                kind: ACE,
                series: series(&["ACE"]),
                issued: now(),
                rows: a.points.iter().map(|(t, v)| (*t, vec![Some(*v)])).collect(),
            }])
        })
    }
}

struct TransferSource(Miso);

impl Source for TransferSource {
    fn name(&self) -> &'static str {
        "MISO North-South transfer (kept by day)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let t = self.0.regional_transfer().fetch(ctx.clone(), None).await?;
            Ok(vec![Batch {
                kind: TRANSFER,
                series: series(&["Flow", "North to south limit", "South to north limit"]),
                issued: now(),
                rows: t
                    .points
                    .iter()
                    .map(|p| {
                        (
                            p.time,
                            vec![p.flow, p.north_south_limit, p.south_north_limit],
                        )
                    })
                    .collect(),
            }])
        })
    }
}

struct CapacitySource(Miso);

impl Source for CapacitySource {
    fn name(&self) -> &'static str {
        "MISO committed capacity (kept by day)"
    }

    fn every(&self) -> Duration {
        Duration::from_secs(30 * 60)
    }

    fn poll<'a>(&'a self, ctx: &'a FetchCtx) -> BoxFuture<'a, Result<Vec<Batch>, FetchError>> {
        Box::pin(async move {
            let c = self.0.capacity().fetch(ctx.clone(), None).await?;
            Ok(vec![Batch {
                kind: CAPACITY,
                series: series(&["Demand", "Committed", "Available"]),
                issued: now(),
                rows: c
                    .points
                    .iter()
                    .filter(|p| p.demand.is_some() || p.committed.is_some())
                    .map(|p| (p.time, vec![p.demand, p.committed, p.available]))
                    .collect(),
            }])
        })
    }
}
