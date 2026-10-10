//! FCST's data: a node's inputs gathered from the price history, Henry Hub
//! gas and the forecasts kept as issued, and the forecasts run as hub queries
//! on blocking threads, so no panel waits for a model to train.
//!
//! A forecast is recomputed only when what it is made from changes: the
//! inputs hold complete days only (today's partial RT is left out), and a
//! fingerprint of them is part of the query's key. Results are cached on disk
//! under `local://models/` with [`MODEL_VERSION`] and that fingerprint, so a
//! restart the same day does not train again; a version change discards them.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use mt_data::{FetchCtx, FetchError, Freshness, Query};
use mt_forecast::features::{Issued, NodeInputs, Target};
use mt_forecast::node::{NodeForecast, Settings};
use mt_forecast::securities::{self, CloseForecast};
use serde::{Deserialize, Serialize};

use crate::context::PanelCx;
use crate::series::{self, Component};

/// Bump when models or features change: cached results are then discarded.
pub const MODEL_VERSION: u32 = 1;

/// Days of prices read for training (older days come only from the price
/// history; charts download at most [`series::DOWNLOAD_DAYS`]).
pub const HISTORY_DAYS: u32 = 365;

/// Trading days a security's forecast reaches.
pub const SECURITY_HORIZON: usize = 20;

/// Years of daily closes a security's forecast reads.
const SECURITY_YEARS: i64 = 4;

/// How far a node's forecast has got.
#[derive(Clone, Debug)]
pub enum NodeState {
    /// Daily reports or other inputs still loading.
    Gathering(usize),
    /// Training and backtesting, on a background thread.
    Training,
    Ready(Arc<NodeForecast>, Arc<NodeInputs>),
    Failed(String),
}

/// Gathers a node's inputs and watches its forecast; one per panel.
#[derive(Default)]
pub struct NodeForecaster {
    inputs: Option<(u64, Arc<NodeInputs>)>,
}

/// FNV-1a: a fingerprint stable across runs (the disk cache compares it).
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn points(&mut self, pts: &[(NaiveDateTime, f64)]) {
        self.u64(pts.len() as u64);
        for (t, v) in pts {
            self.u64(t.and_utc().timestamp() as u64);
            self.u64(v.to_bits());
        }
    }
}

/// The day a node's forecasts are for: tomorrow, market time.
pub fn target_day() -> NaiveDate {
    mt_core::time::market_today() + Duration::days(1)
}

impl NodeForecaster {
    /// Gather, then watch the forecast of `target` at `node` for tomorrow.
    pub fn watch(&mut self, cx: &PanelCx<'_>, node: &str, target: Target) -> NodeState {
        let today = mt_core::time::market_today();
        let day = target_day();
        let lmp = series::node_history(cx, node, Component::Lmp, HISTORY_DAYS);
        let mcc = series::node_history(cx, node, Component::Congestion, HISTORY_DAYS);
        let gas = cx.hub.watch(&cx.eia.henry_hub());
        let issued = cx.hub.watch(&IssuedQuery {
            first: today - Duration::days(i64::from(HISTORY_DAYS) + 14),
            last: today,
        });
        let waiting = lmp.pending
            + usize::from(gas.data.is_none() && gas.error.is_none())
            + usize::from(issued.data.is_none() && issued.error.is_none());
        if waiting > 0 {
            return NodeState::Gathering(waiting);
        }
        // Complete days only: today's RT is still coming in.
        let rt: Vec<(NaiveDateTime, f64)> = lmp
            .rt
            .iter()
            .copied()
            .filter(|(t, _)| t.date() < today)
            .collect();
        let gas_points = gas.data().map(|g| g.points.clone()).unwrap_or_default();
        let issued_list: Arc<Vec<Issued>> = issued.data.unwrap_or_default();
        let mut fp = Fnv::new();
        fp.bytes(node.as_bytes());
        fp.bytes(target.label().as_bytes());
        fp.u64(
            day.and_hms_opt(0, 0, 0)
                .unwrap_or_default()
                .and_utc()
                .timestamp() as u64,
        );
        fp.u64(u64::from(MODEL_VERSION));
        fp.points(&lmp.da);
        fp.points(&rt);
        fp.points(&mcc.da);
        fp.u64(gas_points.len() as u64);
        if let Some((d, v)) = gas_points.last() {
            fp.u64(
                d.and_hms_opt(0, 0, 0)
                    .unwrap_or_default()
                    .and_utc()
                    .timestamp() as u64,
            );
            fp.u64(v.to_bits());
        }
        for i in issued_list.iter() {
            fp.bytes(i.name.as_bytes());
            fp.u64(i.versions.values().map(Vec::len).sum::<usize>() as u64);
            if let Some(latest) = i.versions.values().flatten().map(|v| v.0).max() {
                fp.u64(latest.and_utc().timestamp() as u64);
            }
        }
        let fingerprint = fp.0;
        if self.inputs.as_ref().is_none_or(|(f, _)| *f != fingerprint) {
            let inputs = NodeInputs {
                da: mt_forecast::hourly::hourly(lmp.da.iter().copied()),
                rt: mt_forecast::hourly::hourly(rt),
                congestion: mt_forecast::hourly::hourly(mcc.da.iter().copied()),
                gas: gas_points.into_iter().collect(),
                issued: issued_list.as_ref().clone(),
            };
            self.inputs = Some((fingerprint, Arc::new(inputs)));
        }
        let Some((_, inputs)) = &self.inputs else {
            return NodeState::Gathering(1);
        };
        let snap = cx.hub.watch(&NodeForecastQuery {
            node: node.to_owned(),
            target,
            day,
            fingerprint,
            inputs: inputs.clone(),
        });
        match (snap.data, snap.error) {
            (Some(f), _) => NodeState::Ready(f, inputs.clone()),
            (None, Some(e)) => NodeState::Failed(e.to_string()),
            (None, None) => NodeState::Training,
        }
    }
}

/// The forecasts kept as issued, as the models' inputs: one series per
/// column, named for people ("Load forecast, MISO", "Wind forecast",
/// "Temperature, Minneapolis, MN", "Planned outages"). A daily kind covers
/// every hour of its day.
#[derive(Clone, Debug)]
pub struct IssuedQuery {
    pub first: NaiveDate,
    pub last: NaiveDate,
}

/// A kind kept, how its columns are named, and whether it is daily.
type Kind = (&'static str, fn(&str) -> String, bool);

const KINDS: [Kind; 4] = [
    (
        mt_miso::issued::MTLF,
        |s| format!("Load forecast, {s}"),
        false,
    ),
    (
        mt_miso::issued::WIND_SOLAR,
        |s| format!("{s} forecast"),
        false,
    ),
    (
        mt_nws::issued::TEMPERATURES,
        |s| format!("Temperature, {s}"),
        false,
    ),
    (mt_miso::issued::OUTAGES, |s| format!("{s} outages"), true),
];

impl Query for IssuedQuery {
    type Output = Vec<Issued>;

    fn key(&self) -> String {
        format!("forecast/issued/{}..{}", self.first, self.last)
    }

    fn label(&self) -> String {
        "Forecasts kept as issued".into()
    }

    fn freshness(&self, _: &Self::Output) -> Freshness {
        Freshness::Every(std::time::Duration::from_secs(3600))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Self::Output>>,
    ) -> Result<Self::Output, FetchError> {
        let Some(cache) = ctx.cache().cloned() else {
            return Ok(Vec::new());
        };
        let (first, last) = (self.first, self.last);
        ctx.blocking(move || {
            let mut out = Vec::new();
            for (kind, name, daily) in KINDS {
                let file = mt_data::issued::read(&cache, kind, first, last);
                for (col, series) in file.series.iter().enumerate() {
                    let mut issued = Issued {
                        name: name(series),
                        versions: BTreeMap::new(),
                    };
                    for v in &file.versions {
                        let Some(value) = v.values.get(col).copied().flatten() else {
                            continue;
                        };
                        let hours = if daily { 24 } else { 1 };
                        for h in 0..hours {
                            issued
                                .versions
                                .entry(v.target + Duration::hours(h))
                                .or_default()
                                .push((v.issued, value));
                        }
                    }
                    if !issued.versions.is_empty() {
                        out.push(issued);
                    }
                }
            }
            out
        })
        .await
    }
}

/// A node's forecast for one day, from given inputs.
#[derive(Clone)]
pub struct NodeForecastQuery {
    node: String,
    target: Target,
    day: NaiveDate,
    fingerprint: u64,
    inputs: Arc<NodeInputs>,
}

impl std::fmt::Debug for NodeForecastQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeForecastQuery({})", self.key())
    }
}

/// What the disk cache keeps.
#[derive(Serialize, Deserialize)]
struct Cached {
    version: u32,
    fingerprint: u64,
    forecast: NodeForecast,
}

impl NodeForecastQuery {
    fn cache_key(&self) -> String {
        format!(
            "local://models/node/{}/{}/{}",
            self.node,
            self.target.label(),
            self.day
        )
    }
}

impl Query for NodeForecastQuery {
    type Output = NodeForecast;

    fn key(&self) -> String {
        format!(
            "forecast/node/{}/{}/{}/{:016x}",
            self.node,
            self.target.label(),
            self.day,
            self.fingerprint
        )
    }

    fn label(&self) -> String {
        format!(
            "Forecast, {} {} {}",
            self.node,
            self.target.label(),
            self.day
        )
    }

    /// A function of its key: never refreshed, recomputed under a new key.
    fn freshness(&self, _: &Self::Output) -> Freshness {
        Freshness::Forever
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Self::Output>>,
    ) -> Result<Self::Output, FetchError> {
        let key = self.cache_key();
        if let Some(bytes) = ctx.local_get(&key).await
            && let Ok(c) = serde_json::from_slice::<Cached>(&bytes)
            && c.version == MODEL_VERSION
            && c.fingerprint == self.fingerprint
        {
            return Ok(c.forecast);
        }
        let (inputs, target, day) = (self.inputs.clone(), self.target, self.day);
        let started = std::time::Instant::now();
        let forecast = ctx
            .blocking(move || {
                mt_forecast::node::forecast(&inputs, target, day, &Settings::default())
            })
            .await?;
        ctx.events().info(format!(
            "trained the forecast of {} {} for {} in {:.1} s ({:016x})",
            self.node,
            self.target.label(),
            self.day,
            started.elapsed().as_secs_f64(),
            self.fingerprint
        ));
        if let Ok(json) = serde_json::to_vec(&Cached {
            version: MODEL_VERSION,
            fingerprint: self.fingerprint,
            forecast: forecast.clone(),
        }) {
            ctx.local_put(&key, json).await;
        }
        Ok(forecast)
    }
}

/// A security's forecast from its daily closes, on a blocking thread.
#[derive(Clone)]
pub struct SecurityForecastQuery {
    symbol: String,
    last: Option<NaiveDate>,
    closes: Arc<Vec<f64>>,
}

impl std::fmt::Debug for SecurityForecastQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecurityForecastQuery({})", self.key())
    }
}

impl Query for SecurityForecastQuery {
    type Output = CloseForecast;

    fn key(&self) -> String {
        let mut fp = Fnv::new();
        for c in self.closes.iter() {
            fp.u64(c.to_bits());
        }
        format!(
            "forecast/security/{}/{}/{:016x}",
            self.symbol,
            self.last.map_or_else(String::new, |d| d.to_string()),
            fp.0
        )
    }

    fn label(&self) -> String {
        format!("Forecast, {} US", self.symbol)
    }

    fn freshness(&self, _: &Self::Output) -> Freshness {
        Freshness::Forever
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Self::Output>>,
    ) -> Result<Self::Output, FetchError> {
        let closes = self.closes.clone();
        ctx.blocking(move || securities::forecast_closes(&closes, SECURITY_HORIZON))
            .await
    }
}

/// How far a security's forecast has got.
#[derive(Clone, Debug)]
pub enum SecurityState {
    Loading(Option<String>),
    Training,
    /// The closes (date, close) it was made from, and the forecast.
    Ready(Arc<Vec<(NaiveDate, f64)>>, Arc<CloseForecast>),
    Failed(String),
}

/// Watch a security's daily closes and their forecast.
pub fn security(cx: &PanelCx<'_>, symbol: &str) -> SecurityState {
    let today = mt_core::exchange::now_exchange().date_naive();
    let bars = cx.hub.watch(&cx.alpaca.bars(
        [symbol],
        mt_alpaca::Timeframe::Day1,
        today - Duration::days(SECURITY_YEARS * 365),
        None,
    ));
    let Some(set) = bars.data() else {
        return SecurityState::Loading(bars.error.as_ref().map(ToString::to_string));
    };
    let closes: Vec<(NaiveDate, f64)> = mt_core::beta::daily_closes(set.get(symbol))
        .into_iter()
        .filter(|c| c.1 > 0.0)
        .collect();
    let values: Arc<Vec<f64>> = Arc::new(closes.iter().map(|c| c.1).collect());
    let snap = cx.hub.watch(&SecurityForecastQuery {
        symbol: symbol.to_owned(),
        last: closes.last().map(|c| c.0),
        closes: values,
    });
    match (snap.data, snap.error) {
        (Some(f), _) => SecurityState::Ready(Arc::new(closes), f),
        (None, Some(e)) => SecurityState::Failed(e.to_string()),
        (None, None) => SecurityState::Training,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_and_sensitive() {
        let t = |h: u32| {
            "2026-10-01"
                .parse::<NaiveDate>()
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap()
        };
        let a = [(t(0), 1.0), (t(1), 2.0)];
        let mut x = Fnv::new();
        x.points(&a);
        let mut y = Fnv::new();
        y.points(&a);
        assert_eq!(x.0, y.0);
        let mut z = Fnv::new();
        z.points(&[(t(0), 1.0), (t(1), 2.000_001)]);
        assert_ne!(x.0, z.0);
        // A known value, so a change to the hash (which would orphan every
        // cached forecast) is deliberate.
        let mut k = Fnv::new();
        k.bytes(b"a");
        assert_eq!(k.0, 0xaf63_dc4c_8601_ec8c);
    }
}
