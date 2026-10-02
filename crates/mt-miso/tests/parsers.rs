//! Every parser against a recorded MISO response in `fixtures/`.
//!
//! These tests assert structure and sanity (row counts, plausible ranges), not
//! exact values, so re-recording fixtures with `capture_fixtures` keeps them
//! passing unless MISO actually changed a format.

use std::path::PathBuf;

use chrono::NaiveDate;
use mt_core::{DayReportKind, TRADING_HUBS};
use mt_miso::parse::*;

fn fixture(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn api(path: &str) -> String {
    fixture(&format!("public-api.misoenergy.org/api/{path}.json"))
}

/// The newest recorded report with this suffix, and its date.
fn report(suffix: &str) -> (NaiveDate, String) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/docs.misoenergy.org/marketreports");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures/docs.misoenergy.org/marketreports exists")
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(&format!("_{suffix}.csv")))
        .collect();
    names.sort();
    let name = names.pop().unwrap_or_else(|| panic!("no {suffix} fixture"));
    let day =
        NaiveDate::parse_from_str(&name[..8], "%Y%m%d").expect("report names start with yyyymmdd");
    (
        day,
        std::fs::read_to_string(dir.join(name)).expect("readable fixture"),
    )
}

fn plausible_price(p: f64) -> bool {
    (-500.0..=5000.0).contains(&p)
}

#[test]
fn lmp_board() {
    let b = parse_lmp_board(&api("MarketPricing/GetLmpConsolidatedTable")).unwrap();
    assert!(b.rows.len() > 200, "{} rows", b.rows.len());
    assert!(b.interval.is_some());
    assert!(b.rt_hour_ending.is_some() && b.da_hour_ending.is_some());
    assert_eq!(b.hubs().count(), TRADING_HUBS.len());
    for r in &b.rows {
        let p = r.rt_5min.expect("every node has an RT price");
        assert!(plausible_price(p.lmp), "{}: {}", r.node, p.lmp);
        assert!(
            ["North", "Midwest", "South"].contains(&r.region.as_str()),
            "{}",
            r.region
        );
    }
    assert!(b.rows.iter().filter(|r| r.da_expost.is_some()).count() > 200);
}

#[test]
fn exante_hubs() {
    let x = parse_exante_hubs(&api("MarketPricing/GetExAnteLmp")).unwrap();
    assert!(x.interval.is_some());
    assert!(x.hubs.len() >= 8);
    assert!(x.hubs.iter().all(|(_, p)| plausible_price(p.lmp)));
}

#[test]
fn rt_five_min_current_and_rolling() {
    let cur = parse_rt_five_min(&api("MarketPricing/GetRealTimeFiveMinExPost/Current")).unwrap();
    assert!(cur.len() > 1000, "{} rows", cur.len());
    let t = cur[0].interval;
    assert!(
        cur.iter().all(|r| r.interval == t),
        "Current is one interval"
    );

    let rolling =
        parse_rt_five_min(&api("MarketPricing/GetRealTimeFiveMinExPost/Rolling")).unwrap();
    let store = mt_core::RtIntraday::from_rows(rolling);
    assert!(store.intervals().len() > 1);
    assert!(store.contains("MINN.HUB"));
    let (_, latest) = store.latest("MINN.HUB").unwrap();
    assert!(plausible_price(latest.lmp));
}

#[test]
fn ancillary() {
    let a = parse_ancillary(&api("MarketPricing/GetAncillaryServicesMcp")).unwrap();
    assert!(a.interval.is_some());
    assert!(a.zones.len() >= 5);
    assert!(
        a.zones
            .iter()
            .all(|z| z.regulation.is_some() && z.spinning.is_some())
    );
}

#[test]
fn fuel_mix() {
    let now = parse_fuel_mix(&api("FuelMix")).unwrap();
    assert!(now.interval.is_some());
    assert!(now.fuels.len() >= 5);
    assert!(now.total() > 20_000.0, "total {}", now.total());

    let today = parse_fuel_mix_history(&api("FuelMix/Today")).unwrap();
    assert!(today.intervals.len() > 10);
    assert!(today.categories().iter().any(|c| c == "Coal"));
    assert!(!today.series("Wind").is_empty());
}

#[test]
fn load() {
    let l = parse_load(&api("RealTimeTotalLoad")).unwrap();
    assert!(l.market_day.is_some());
    assert_eq!(l.da_cleared.len(), 24);
    assert_eq!(l.forecast.len(), 24);
    assert!(l.actual_5min.len() > 10);
    let (_, mw) = l.latest().unwrap();
    assert!((30_000.0..200_000.0).contains(&mw));
    assert!(l.forecast_peak().is_some());
}

#[test]
fn interchange() {
    let n = parse_nsi(&api("Interchange/GetNsi")).unwrap();
    assert!(n.time.is_some());
    assert!(n.net().is_some(), "MISO total present");
    assert!(n.neighbours().count() >= 5);
    let h = parse_nsi_history(&api("Interchange/GetNsi/FiveMinute")).unwrap();
    assert!(h.points.len() > 10);
    assert!(h.points.windows(2).all(|w| w[0].time <= w[1].time));
}

#[test]
fn binding_constraints() {
    let c = parse_binding_constraints(&api("BindingConstraints/RealTime")).unwrap();
    assert!(c.interval.is_some());
    for k in &c.constraints {
        assert!(!k.name.is_empty());
        assert!(k.shadow_price.is_some());
    }
    // The "nothing binding" placeholder parses to an empty list.
    let none = r#"{"RefId":"02-Oct-2026 - Interval 16:30 EST","Constraint":[{"Name":"None","Period":"2026-10-02T16:30:00","Price":"None"}]}"#;
    assert!(
        parse_binding_constraints(none)
            .unwrap()
            .constraints
            .is_empty()
    );
}

#[test]
fn renewables() {
    let r = parse_renewables(&api("WindSolar/GetCombined")).unwrap();
    assert!(r.market_day.is_some());
    assert!(r.hours.len() >= 24);
    assert!(r.hours.iter().any(|h| h.wind_actual.is_some()));
    assert!(r.hours.iter().all(|h| h.wind_forecast.is_some()));
}

#[test]
fn outages() {
    let o = parse_outages(&api(
        "GenerationOutages/GetGenerationOutagesPlusMinusFiveDays",
    ))
    .unwrap();
    assert!(o.headline.contains("Outage"));
    assert_eq!(o.days.len(), 11);
    assert!(o.days.iter().all(|d| d.planned > 0.0));
}

#[test]
fn capacity() {
    let c = parse_capacity(&api("CsatSupplyDemand")).unwrap();
    assert!(c.points.len() > 24);
    assert!(c.points.iter().any(|p| p.demand.is_some()));
    assert!(c.points.iter().any(|p| p.available.is_some()));
}

#[test]
fn snapshot() {
    let s = parse_snapshot(&api("Snapshot")).unwrap();
    assert!(s.current_demand().and_then(|i| i.value).is_some());
    assert!(s.forecast_peak().and_then(|i| i.value).is_some());
    assert!(s.marginal_energy_cost().and_then(|i| i.value).is_some());
    assert!(s.scheduled_interchange().and_then(|i| i.value).is_some());
}

#[test]
fn day_reports() {
    for (suffix, kind) in [
        ("da_expost_lmp", DayReportKind::DaExPost),
        ("rt_lmp_prelim", DayReportKind::RtPrelim),
        ("rt_lmp_final", DayReportKind::RtFinal),
    ] {
        let (day, body) = report(suffix);
        let r = parse_day_report(kind, day, &body).unwrap();
        let hub = r
            .node("MINN.HUB")
            .unwrap_or_else(|| panic!("{suffix}: MINN.HUB"));
        assert_eq!(hub.node_type, "Hub");
        assert!(hub.lmp.iter().all(|v| v.is_finite()), "{suffix}: 24 LMPs");
        assert!(hub.mcc.iter().all(|v| v.is_finite()), "{suffix}: 24 MCCs");
        assert_eq!(hourly_points(day, &hub.lmp).len(), 24);
    }
    assert!(parse_day_report(DayReportKind::DaExPost, NaiveDate::MIN, "garbage").is_err());
}
