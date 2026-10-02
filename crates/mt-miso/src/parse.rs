//! Response parsers: MISO wire formats -> `mt-core` types.
//!
//! Every function here is pure (`&str` in, value out) and covered by a fixture
//! test in `tests/parsers.rs`. Raw structs mirror MISO's JSON loosely: every
//! field is optional, so an added or dropped field degrades a column instead of
//! failing the whole dataset.

use std::borrow::Cow;
use std::collections::HashMap;

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use mt_core::num::{parse_num, parse_opt};
use mt_core::time::{hour_ending_start, parse_market_datetime, parse_market_day, parse_ref_id};
use mt_core::*;
use mt_data::FetchError;
use serde::Deserialize;
use serde::de::DeserializeOwned;

fn json<T: DeserializeOwned>(what: &str, body: &str) -> Result<T, FetchError> {
    serde_json::from_str(body).map_err(|e| FetchError::parse(what, e))
}

fn dt(s: Option<&str>) -> Option<NaiveDateTime> {
    s.and_then(|s| parse_market_datetime(s).ok())
}

/// MISO is inconsistent about numbers: usually strings, sometimes JSON numbers.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum Num {
    N(f64),
    S(String),
}

fn num(v: &Option<Num>) -> Option<f64> {
    match v {
        Some(Num::N(n)) => Some(*n).filter(|n| n.is_finite()),
        Some(Num::S(s)) => parse_num(s),
        None => None,
    }
}

/// `"HE 16"` -> 16
fn hour_ending(s: Option<&str>) -> Option<u8> {
    s?.trim().trim_start_matches("HE").trim().parse().ok()
}

// --- LMP consolidated table ---------------------------------------------------

#[derive(Deserialize)]
struct LmpConsolidatedRaw {
    #[serde(rename = "LMPData")]
    data: LmpDataRaw,
}

#[derive(Deserialize)]
struct LmpDataRaw {
    #[serde(rename = "RefId")]
    ref_id: Option<String>,
    #[serde(rename = "FiveMinLMP")]
    five_min: Option<LmpBlockRaw>,
    #[serde(rename = "HourlyIntegratedLMP")]
    hourly: Option<LmpBlockRaw>,
    #[serde(rename = "DayAheadExAnteLMP")]
    da_exante: Option<LmpBlockRaw>,
    #[serde(rename = "DayAheadExPostLMP")]
    da_expost: Option<LmpBlockRaw>,
}

#[derive(Deserialize)]
struct LmpBlockRaw {
    #[serde(rename = "HourAndMin")]
    hour_and_min: Option<String>,
    #[serde(rename = "PricingNode", default)]
    nodes: Vec<PricingNodeRaw>,
}

#[derive(Deserialize)]
struct PricingNodeRaw {
    name: String,
    region: Option<String>,
    #[serde(rename = "LMP")]
    lmp: Option<String>,
    #[serde(rename = "MCC")]
    mcc: Option<String>,
    #[serde(rename = "MLC")]
    mlc: Option<String>,
}

impl PricingNodeRaw {
    fn price(&self) -> Option<Lmp> {
        Some(Lmp::new(
            parse_opt(self.lmp.as_deref())?,
            parse_opt(self.mcc.as_deref()).unwrap_or(0.0),
            parse_opt(self.mlc.as_deref()).unwrap_or(0.0),
        ))
    }
}

pub fn parse_lmp_board(body: &str) -> Result<LmpBoard, FetchError> {
    let raw: LmpConsolidatedRaw = json("LMP consolidated table", body)?;
    let d = raw.data;
    let index = |block: &Option<LmpBlockRaw>| -> HashMap<String, Lmp> {
        block
            .iter()
            .flat_map(|b| &b.nodes)
            .filter_map(|n| Some((n.name.clone(), n.price()?)))
            .collect()
    };
    let (hourly, exante, expost) = (index(&d.hourly), index(&d.da_exante), index(&d.da_expost));
    let he =
        |b: &Option<LmpBlockRaw>| hour_ending(b.as_ref().and_then(|b| b.hour_and_min.as_deref()));

    // Row order follows the five-minute block; nodes only in other blocks are appended.
    let mut order: Vec<(&str, Option<&str>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in [&d.five_min, &d.hourly, &d.da_exante, &d.da_expost]
        .into_iter()
        .flatten()
    {
        for n in &block.nodes {
            if seen.insert(n.name.as_str()) {
                order.push((&n.name, n.region.as_deref()));
            }
        }
    }
    let five = index(&d.five_min);
    let rows = order
        .into_iter()
        .map(|(name, region)| LmpBoardRow {
            node: name.to_owned(),
            region: region.unwrap_or_default().to_owned(),
            rt_5min: five.get(name).copied(),
            rt_hourly: hourly.get(name).copied(),
            da_exante: exante.get(name).copied(),
            da_expost: expost.get(name).copied(),
        })
        .collect();
    Ok(LmpBoard {
        interval: d.ref_id.as_deref().and_then(parse_ref_id),
        rt_hour_ending: he(&d.hourly),
        da_hour_ending: he(&d.da_expost).or_else(|| he(&d.da_exante)),
        rows,
    })
}

// --- Ex-ante hub prices -------------------------------------------------------

#[derive(Deserialize)]
struct ExAnteRaw {
    #[serde(rename = "LMPData")]
    data: ExAnteDataRaw,
}

#[derive(Deserialize)]
struct ExAnteDataRaw {
    #[serde(rename = "RefId")]
    ref_id: Option<String>,
    #[serde(rename = "ExAnteLMP")]
    exante: Option<ExAnteBlockRaw>,
}

#[derive(Deserialize)]
struct ExAnteBlockRaw {
    #[serde(rename = "Hub", default)]
    hubs: Vec<ExAnteHubRaw>,
}

#[derive(Deserialize)]
struct ExAnteHubRaw {
    name: String,
    #[serde(rename = "LMP")]
    lmp: Option<String>,
    loss: Option<String>,
    congestion: Option<String>,
}

pub fn parse_exante_hubs(body: &str) -> Result<HubExAnte, FetchError> {
    let raw: ExAnteRaw = json("ex-ante hub LMPs", body)?;
    let hubs = raw
        .data
        .exante
        .map(|b| b.hubs)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|h| {
            let p = Lmp::new(
                parse_opt(h.lmp.as_deref())?,
                parse_opt(h.congestion.as_deref()).unwrap_or(0.0),
                parse_opt(h.loss.as_deref()).unwrap_or(0.0),
            );
            Some((h.name, p))
        })
        .collect();
    Ok(HubExAnte {
        interval: raw.data.ref_id.as_deref().and_then(parse_ref_id),
        hubs,
    })
}

// --- Five-minute ex-post feed (Current / Rolling) ---------------------------

#[derive(Deserialize)]
struct TableRaw<'a> {
    headers: Vec<String>,
    #[serde(borrow)]
    data: Vec<Vec<Option<Cow<'a, str>>>>,
}

/// Parse MISO's `{headers, data}` table of five-minute LMPs. Columns are found
/// by header name, so a reordering on MISO's side is harmless.
pub fn parse_rt_five_min(body: &str) -> Result<Vec<RtRow>, FetchError> {
    const WHAT: &str = "five-minute ex-post LMPs";
    let raw: TableRaw<'_> = serde_json::from_str(body).map_err(|e| FetchError::parse(WHAT, e))?;
    let col = |name: &str| {
        raw.headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
            .ok_or_else(|| FetchError::parse(WHAT, format!("no {name} column")))
    };
    let (ti, ni, li, ci, mi) = (
        col("INTERVAL")?,
        col("CPNODE")?,
        col("LMP")?,
        col("MCC")?,
        col("MLC")?,
    );
    fn cell<'r>(row: &'r [Option<Cow<'_, str>>], i: usize) -> Option<&'r str> {
        row.get(i).and_then(|c| c.as_deref())
    }
    let mut rows = Vec::with_capacity(raw.data.len());
    // Parse each distinct timestamp once; the rolling feed repeats ~2,600 per interval.
    let mut last_t: Option<(String, NaiveDateTime)> = None;
    for r in &raw.data {
        let (Some(ts), Some(node)) = (cell(r, ti), cell(r, ni)) else {
            continue;
        };
        let interval = match &last_t {
            Some((s, t)) if s == ts => *t,
            _ => {
                let Ok(t) = parse_market_datetime(ts) else {
                    continue;
                };
                last_t = Some((ts.to_owned(), t));
                t
            }
        };
        let Some(lmp) = cell(r, li).and_then(parse_num) else {
            continue;
        };
        rows.push(RtRow {
            interval,
            node: node.to_owned(),
            lmp,
            mcc: cell(r, ci).and_then(parse_num).unwrap_or(0.0),
            mlc: cell(r, mi).and_then(parse_num).unwrap_or(0.0),
        });
    }
    Ok(rows)
}

// --- Ancillary services -------------------------------------------------------

#[derive(Deserialize)]
struct McpRaw {
    #[serde(rename = "MCPData")]
    data: McpDataRaw,
}

#[derive(Deserialize)]
struct McpDataRaw {
    #[serde(rename = "RefId")]
    ref_id: Option<String>,
    #[serde(rename = "RealTimeMCP")]
    rt: Option<McpBlockRaw>,
}

#[derive(Deserialize)]
struct McpBlockRaw {
    #[serde(rename = "Zone", default)]
    zones: Vec<HashMap<String, Option<Num>>>,
}

pub fn parse_ancillary(body: &str) -> Result<AncillaryMcps, FetchError> {
    let raw: McpRaw = json("ancillary-service MCPs", body)?;
    let zones = raw
        .data
        .rt
        .map(|b| b.zones)
        .unwrap_or_default()
        .into_iter()
        .map(|z| {
            // Keys vary in case between fields (GenRegMCP vs StrMcp); match loosely.
            let get = |key: &str| {
                z.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .and_then(|(_, v)| num(v))
            };
            ZoneMcp {
                zone: match z.iter().find(|(k, _)| k.eq_ignore_ascii_case("number")) {
                    Some((_, Some(Num::S(s)))) => s.clone(),
                    Some((_, Some(Num::N(n)))) => n.to_string(),
                    _ => "?".into(),
                },
                regulation: get("GenRegMCP"),
                spinning: get("GenSpinMCP"),
                supplemental: get("GenSuppMCP"),
                short_term: get("StrMcp"),
                ramp_up: get("RcpUpMcp"),
                ramp_down: get("RcpDownMcp"),
            }
        })
        .collect();
    Ok(AncillaryMcps {
        interval: raw.data.ref_id.as_deref().and_then(parse_ref_id),
        zones,
    })
}

// --- Fuel mix -----------------------------------------------------------------

#[derive(Deserialize)]
struct FuelMixRaw {
    #[serde(rename = "TotalMW")]
    total_mw: Option<String>,
    #[serde(rename = "Fuel")]
    fuel: Option<FuelBlockRaw>,
}

#[derive(Deserialize)]
struct FuelBlockRaw {
    #[serde(rename = "Type", default)]
    types: Vec<FuelTypeRaw>,
}

#[derive(Deserialize)]
struct FuelTypeRaw {
    #[serde(rename = "INTERVALEST")]
    interval: Option<String>,
    #[serde(rename = "CATEGORY")]
    category: Option<String>,
    #[serde(rename = "ACT")]
    actual: Option<String>,
}

/// Parse a fuel-mix payload into one entry per interval (the `Today` feed has
/// the whole day; the plain feed has one interval).
pub fn parse_fuel_mix_history(body: &str) -> Result<FuelMixHistory, FetchError> {
    let raw: FuelMixRaw = json("fuel mix", body)?;
    let mut intervals: Vec<FuelMix> = Vec::new();
    for t in raw.fuel.map(|f| f.types).unwrap_or_default() {
        let (Some(cat), Some(mw)) = (t.category, parse_opt(t.actual.as_deref())) else {
            continue;
        };
        let at = dt(t.interval.as_deref());
        match intervals.iter_mut().find(|m| m.interval == at) {
            Some(m) => m.fuels.push((cat.trim().to_owned(), mw)),
            None => intervals.push(FuelMix {
                interval: at,
                total_mw: None,
                fuels: vec![(cat.trim().to_owned(), mw)],
            }),
        }
    }
    intervals.sort_by_key(|m| m.interval);
    if let Some(last) = intervals.last_mut() {
        last.total_mw = parse_opt(raw.total_mw.as_deref());
    }
    Ok(FuelMixHistory { intervals })
}

/// The latest interval of a fuel-mix payload.
pub fn parse_fuel_mix(body: &str) -> Result<FuelMix, FetchError> {
    parse_fuel_mix_history(body)?
        .intervals
        .pop()
        .ok_or_else(|| FetchError::parse("fuel mix", "no intervals"))
}

// --- Load ---------------------------------------------------------------------

#[derive(Deserialize)]
struct LoadRaw {
    #[serde(rename = "LoadInfo")]
    info: LoadInfoRaw,
}

#[derive(Deserialize)]
struct LoadInfoRaw {
    #[serde(rename = "RefId")]
    ref_id: Option<String>,
    #[serde(rename = "ClearedMW", default)]
    cleared: Vec<ClearedRaw>,
    #[serde(rename = "MediumTermLoadForecast", default)]
    forecast: Vec<ForecastRaw>,
    #[serde(rename = "FiveMinTotalLoad", default)]
    five_min: Vec<FiveMinLoadRaw>,
}

#[derive(Deserialize)]
struct ClearedRaw {
    #[serde(rename = "ClearedMWHourly")]
    v: HourValueRaw,
}

#[derive(Deserialize)]
struct HourValueRaw {
    #[serde(rename = "Hour")]
    hour: Option<String>,
    #[serde(rename = "Value")]
    value: Option<String>,
}

#[derive(Deserialize)]
struct ForecastRaw {
    #[serde(rename = "Forecast")]
    v: ForecastValueRaw,
}

#[derive(Deserialize)]
struct ForecastValueRaw {
    #[serde(rename = "HourEnding")]
    hour_ending: Option<String>,
    #[serde(rename = "LoadForecast")]
    value: Option<String>,
}

#[derive(Deserialize)]
struct FiveMinLoadRaw {
    #[serde(rename = "Load")]
    v: TimeValueRaw,
}

#[derive(Deserialize)]
struct TimeValueRaw {
    #[serde(rename = "Time")]
    time: Option<String>,
    #[serde(rename = "Value")]
    value: Option<String>,
}

pub fn parse_load(body: &str) -> Result<SystemLoad, FetchError> {
    let raw: LoadRaw = json("real-time load", body)?;
    let i = raw.info;
    let as_of = i.ref_id.as_deref().and_then(parse_ref_id);
    let day = as_of.map(|t| t.date());
    let hourly =
        |h: Option<&str>, v: Option<&str>| Some((h?.trim().parse::<u8>().ok()?, parse_opt(v)?));
    let mut actual_5min = Vec::new();
    if let Some(day) = day {
        for p in &i.five_min {
            let (Some(t), Some(v)) = (p.v.time.as_deref(), parse_opt(p.v.value.as_deref())) else {
                continue;
            };
            if let Ok(t) = NaiveTime::parse_from_str(t.trim(), "%H:%M") {
                actual_5min.push((day.and_time(t), v));
            }
        }
    }
    Ok(SystemLoad {
        market_day: day,
        as_of,
        actual_5min,
        da_cleared: i
            .cleared
            .iter()
            .filter_map(|c| hourly(c.v.hour.as_deref(), c.v.value.as_deref()))
            .collect(),
        forecast: i
            .forecast
            .iter()
            .filter_map(|f| hourly(f.v.hour_ending.as_deref(), f.v.value.as_deref()))
            .collect(),
    })
}

// --- Interchange --------------------------------------------------------------

#[derive(Deserialize)]
struct NsiRaw {
    #[serde(default)]
    instance: Vec<serde_json::Map<String, serde_json::Value>>,
}

fn interchange_point(m: &serde_json::Map<String, serde_json::Value>) -> Interchange {
    let mut time = None;
    let mut by_ba = Vec::new();
    for (k, v) in m {
        if k.eq_ignore_ascii_case("timestamp") {
            time = dt(v.as_str());
            continue;
        }
        let value = match v {
            serde_json::Value::String(s) => parse_num(s),
            serde_json::Value::Number(n) => n.as_f64(),
            _ => None,
        };
        if let Some(value) = value {
            // MISO's own net total is "MISO" in one feed and "NSI" in another.
            let ba = if k == "NSI" {
                "MISO".to_owned()
            } else {
                k.clone()
            };
            by_ba.push((ba, value));
        }
    }
    Interchange { time, by_ba }
}

/// Latest net scheduled interchange by neighbour.
pub fn parse_nsi(body: &str) -> Result<Interchange, FetchError> {
    parse_nsi_history(body)?
        .points
        .pop()
        .ok_or_else(|| FetchError::parse("interchange", "no instances"))
}

pub fn parse_nsi_history(body: &str) -> Result<InterchangeHistory, FetchError> {
    let raw: NsiRaw = json("interchange", body)?;
    let mut points: Vec<Interchange> = raw.instance.iter().map(interchange_point).collect();
    points.sort_by_key(|p| p.time);
    Ok(InterchangeHistory { points })
}

// --- Binding constraints ------------------------------------------------------

#[derive(Deserialize)]
struct ConstraintsRaw {
    #[serde(rename = "RefId")]
    ref_id: Option<String>,
    #[serde(rename = "Constraint", default)]
    constraints: Vec<ConstraintRaw>,
}

#[derive(Deserialize)]
struct ConstraintRaw {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Period")]
    period: Option<String>,
    #[serde(rename = "Price")]
    price: Option<String>,
    #[serde(rename = "OVERRIDE")]
    overridden: Option<String>,
    #[serde(rename = "CURVETYPE")]
    curve_type: Option<String>,
}

pub fn parse_binding_constraints(body: &str) -> Result<BindingConstraints, FetchError> {
    let raw: ConstraintsRaw = json("binding constraints", body)?;
    let constraints = raw
        .constraints
        .into_iter()
        .filter_map(|c| {
            let name = c.name?.trim().to_owned();
            // MISO sends a single "None" row when nothing binds.
            if name.is_empty() || name.eq_ignore_ascii_case("none") || name.starts_with("None -") {
                return None;
            }
            Some(BindingConstraint {
                name,
                period: dt(c.period.as_deref()),
                shadow_price: parse_opt(c.price.as_deref()),
                overridden: match c.overridden.as_deref().map(str::trim) {
                    Some("1") => Some(true),
                    Some("0") => Some(false),
                    _ => None,
                },
                curve_type: c.curve_type.filter(|s| !s.trim().is_empty() && s != "None"),
            })
        })
        .collect();
    Ok(BindingConstraints {
        interval: raw.ref_id.as_deref().and_then(parse_ref_id),
        constraints,
    })
}

// --- Wind and solar -----------------------------------------------------------

#[derive(Deserialize)]
struct WindSolarRaw {
    #[serde(rename = "MktDay")]
    market_day: Option<String>,
    #[serde(default)]
    instance: Vec<WindSolarHourRaw>,
}

#[derive(Deserialize)]
struct WindSolarHourRaw {
    #[serde(rename = "ForecastDateTimeEST")]
    forecast_time: Option<String>,
    #[serde(rename = "ForecastHourEndingEST")]
    forecast_he: Option<String>,
    #[serde(rename = "ForecastWindValue")]
    wind_forecast: Option<String>,
    #[serde(rename = "ForecastSolarValue")]
    solar_forecast: Option<String>,
    #[serde(rename = "ActualWindValue")]
    wind_actual: Option<String>,
    #[serde(rename = "ActualSolarValue")]
    solar_actual: Option<String>,
}

pub fn parse_renewables(body: &str) -> Result<Renewables, FetchError> {
    let raw: WindSolarRaw = json("wind and solar", body)?;
    let hours = raw
        .instance
        .into_iter()
        .filter_map(|h| {
            let start = dt(h.forecast_time.as_deref())?;
            Some(RenewableHour {
                start,
                hour_ending: h
                    .forecast_he
                    .as_deref()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0),
                wind_forecast: parse_opt(h.wind_forecast.as_deref()),
                solar_forecast: parse_opt(h.solar_forecast.as_deref()),
                wind_actual: parse_opt(h.wind_actual.as_deref()),
                solar_actual: parse_opt(h.solar_actual.as_deref()),
            })
        })
        .collect();
    Ok(Renewables {
        market_day: raw
            .market_day
            .as_deref()
            .and_then(|s| parse_market_day(s).ok()),
        hours,
    })
}

// --- Outages ------------------------------------------------------------------

#[derive(Deserialize)]
struct OutagesRaw {
    #[serde(rename = "RefId")]
    ref_id: Option<String>,
    #[serde(rename = "Days", default)]
    days: Vec<OutageDayRaw>,
}

#[derive(Deserialize)]
struct OutageDayRaw {
    #[serde(rename = "OutageDate")]
    date: Option<String>,
    #[serde(rename = "Planned")]
    planned: Option<Num>,
    #[serde(rename = "Unplanned")]
    unplanned: Option<Num>,
    #[serde(rename = "Forced")]
    forced: Option<Num>,
    #[serde(rename = "Derated")]
    derated: Option<Num>,
}

pub fn parse_outages(body: &str) -> Result<Outages, FetchError> {
    let raw: OutagesRaw = json("generation outages", body)?;
    let days = raw
        .days
        .iter()
        .filter_map(|d| {
            Some(OutageDay {
                day: dt(d.date.as_deref())?.date(),
                planned: num(&d.planned).unwrap_or(0.0),
                unplanned: num(&d.unplanned).unwrap_or(0.0),
                forced: num(&d.forced).unwrap_or(0.0),
                derated: num(&d.derated).unwrap_or(0.0),
            })
        })
        .collect();
    let headline = raw
        .ref_id
        .as_deref()
        .and_then(|r| r.split_once(" - ").map(|(_, h)| h.trim().to_owned()))
        .unwrap_or_default();
    Ok(Outages { headline, days })
}

// --- Capacity (CSAT supply/demand) --------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityRaw {
    time_est: Option<String>,
    demand: Option<Num>,
    committed_capacity: Option<Num>,
    demand_forecast: Option<Num>,
    committed_capacity_forecast: Option<Num>,
    available_capacity: Option<Num>,
}

pub fn parse_capacity(body: &str) -> Result<Capacity, FetchError> {
    let raw: Vec<CapacityRaw> = json("supply and demand", body)?;
    let points = raw
        .iter()
        .filter_map(|p| {
            Some(CapacityPoint {
                time: dt(p.time_est.as_deref())?,
                demand: num(&p.demand),
                committed: num(&p.committed_capacity),
                demand_forecast: num(&p.demand_forecast),
                committed_forecast: num(&p.committed_capacity_forecast),
                available: num(&p.available_capacity),
            })
        })
        .collect();
    Ok(Capacity { points })
}

// --- Snapshot -----------------------------------------------------------------

#[derive(Deserialize)]
struct SnapshotRaw {
    t: Option<String>,
    v: Option<String>,
    d: Option<String>,
}

pub fn parse_snapshot(body: &str) -> Result<GridSnapshot, FetchError> {
    let raw: Vec<SnapshotRaw> = json("grid snapshot", body)?;
    let items = raw
        .into_iter()
        .filter_map(|i| {
            let title = i.t?;
            let raw = i.v.unwrap_or_default();
            Some(SnapshotItem {
                title,
                value: parse_num(&raw),
                raw,
                time: dt(i.d.as_deref()),
            })
        })
        .collect();
    Ok(GridSnapshot { items })
}

// --- Daily market report CSVs -------------------------------------------------

/// Parse a daily LMP report (`<yyyymmdd>_da_expost_lmp.csv` and friends).
///
/// The file starts with a few banner lines; the table begins at the line that
/// starts `Node,`. Each node has three rows (LMP, MCC, MLC) of 24 hour-endings.
pub fn parse_day_report(
    kind: DayReportKind,
    day: NaiveDate,
    body: &str,
) -> Result<DayLmpReport, FetchError> {
    let what = || format!("{} report for {day}", kind.label());
    let start = body
        .find("\nNode,")
        .map(|i| i + 1)
        .or_else(|| body.starts_with("Node,").then_some(0))
        .ok_or_else(|| FetchError::parse(what(), "no 'Node,' header line"))?;
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .from_reader(&body.as_bytes()[start..]);
    let headers = reader
        .headers()
        .map_err(|e| FetchError::parse(what(), e))?
        .clone();
    let he_col: Vec<Option<usize>> = (1..=24)
        .map(|he| headers.iter().position(|h| h.trim() == format!("HE {he}")))
        .collect();

    let mut rows: Vec<DayNodeRow> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for rec in reader.records() {
        let rec = rec.map_err(|e| FetchError::parse(what(), e))?;
        let (Some(node), Some(node_type), Some(value)) = (rec.get(0), rec.get(1), rec.get(2))
        else {
            continue;
        };
        if node.is_empty() {
            continue;
        }
        let i = *index.entry(node.to_owned()).or_insert_with(|| {
            rows.push(DayNodeRow {
                node: node.to_owned(),
                node_type: node_type.to_owned(),
                lmp: [f32::NAN; 24],
                mcc: [f32::NAN; 24],
                mlc: [f32::NAN; 24],
            });
            rows.len() - 1
        });
        let target = match value.trim() {
            "LMP" => &mut rows[i].lmp,
            "MCC" => &mut rows[i].mcc,
            "MLC" => &mut rows[i].mlc,
            _ => continue,
        };
        for (h, col) in he_col.iter().enumerate() {
            if let Some(v) = col.and_then(|c| rec.get(c)).and_then(parse_num) {
                target[h] = v as f32;
            }
        }
    }
    if rows.is_empty() {
        return Err(FetchError::parse(what(), "no rows"));
    }
    Ok(DayLmpReport::new(kind, day, rows))
}

/// Hourly `(start, value)` points from a report row, skipping missing hours.
pub fn hourly_points(day: NaiveDate, values: &[f32; 24]) -> Vec<(NaiveDateTime, f64)> {
    values
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .map(|(h, v)| (hour_ending_start(day, h as u8 + 1), f64::from(*v)))
        .collect()
}

/// End of a report day, for chart bounds.
pub fn day_end(day: NaiveDate) -> NaiveDateTime {
    day.and_time(NaiveTime::MIN) + Duration::days(1)
}
