//! The assistant's lookups against the recorded fixtures: every tool answers,
//! answers in the shape its description promises, and only ever reads.

#[path = "common/scene.rs"]
mod scene;

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};

use mt_ai::ToolBox;
use mt_data::{
    BoxFuture, DataHub, EventLog, FetchCtx, FetchCtxOptions, FetchError, FixtureTransport, Method,
    Request, Response, Transport,
};
use mt_ui::AppConfig;
use mt_ui::assistant::Lookups;

/// The fixtures, with every request written down.
struct Recording {
    inner: FixtureTransport,
    asked: Mutex<Vec<(Method, String)>>,
}

impl Transport for Recording {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        self.asked.lock().push((req.method, req.url.clone()));
        self.inner.send(req)
    }
    fn describe(&self) -> String {
        "recording".into()
    }
    fn is_live(&self) -> bool {
        false
    }
}

async fn call(tools: &Lookups, name: &str, input: Value) -> Value {
    mt_ai::tools::run(tools, name, input)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn every_lookup_answers_from_the_fixtures_and_only_reads() {
    let fixtures = scene::fixtures_dir();
    let at = scene::fixtures_time(&fixtures).expect("the fixtures have an LMP table");
    // Process-wide, which is why this is the only test in this binary.
    mt_core::time::freeze_clock(Some(mt_core::time::market_to_utc(at)));
    let today = at.date();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let transport = Arc::new(Recording {
        inner: FixtureTransport::new(&fixtures),
        asked: Mutex::default(),
    });
    let ctx = FetchCtx::new(
        transport.clone(),
        None,
        FetchCtxOptions {
            max_concurrent: 8,
            polite_interval: Duration::ZERO,
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let hub = DataHub::new(runtime.handle().clone(), ctx);
    let tools = Lookups::new(hub.clone(), &AppConfig::default());
    let mut sharing = AppConfig::default();
    sharing.ai.share_paper_account = true;
    let sharing = Lookups::new(hub, &sharing);

    let names: Vec<String> = tools.specs().iter().map(|s| s.name.clone()).collect();
    assert_eq!(
        names,
        [
            "find_nodes",
            "lmp_series",
            "price_extremes",
            "binding_constraints",
            "reserves",
            "system_conditions",
            "outages",
            "weather",
            "gas",
            "news",
            "forecast",
            "securities",
            "load",
            "paper_account"
        ],
        "the order is part of the cached prompt: change it on purpose"
    );
    for spec in tools.specs() {
        assert_eq!(
            spec.input_schema["additionalProperties"], false,
            "{}",
            spec.name
        );
        assert!(
            spec.description.len() > 40,
            "{} needs a description",
            spec.name
        );
    }

    runtime.block_on(async {
        let indiana = call(&tools, "find_nodes", json!({"query": "Indiana"})).await;
        assert_eq!(indiana["nodes"][0]["node"], "INDIANA.HUB", "{indiana}");
        let hubs = call(&tools, "find_nodes", json!({"query": "hub", "limit": 50})).await;
        assert_eq!(hubs["nodes"].as_array().map(Vec::len), Some(8), "{hubs}");
        let named = call(&tools, "find_nodes", json!({"query": "minn.hub"})).await;
        assert_eq!(named["nodes"][0]["node"], "MINN.HUB");

        let series = call(
            &tools,
            "lmp_series",
            json!({"node": "MINN.HUB", "start_date": today.to_string(), "components": true}),
        )
        .await;
        let day = &series["days"][0];
        assert_eq!(day["da"]["lmp"].as_array().map(Vec::len), Some(24), "{series}");
        assert!(day["da"]["mcc"].is_array());
        assert!(day["rt"]["lmp"].is_array(), "today's RT from the five-minute feed");
        assert!(series["da_stats"]["max"]["value"].is_number());
        assert_eq!(series["terminal"], "GP MINN.HUB 1");

        let five = call(
            &tools,
            "lmp_series",
            json!({"node": "MINN.HUB", "start_date": today.to_string(), "five_minute": true}),
        )
        .await;
        assert!(five["intervals"].as_array().is_some_and(|v| !v.is_empty()), "{five}");

        let err = mt_ai::tools::run(
            &tools,
            "lmp_series",
            json!({"node": "NOWHERE.HUB", "start_date": today.to_string(), "market": "da"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("find_nodes"), "{err}");
        let err = mt_ai::tools::run(
            &tools,
            "lmp_series",
            json!({"node": "MINN.HUB", "start_date": "2026-01-01", "end_date": "2026-03-01"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err, "at most 31 days at a time");

        // No store in a replay: the days come from the reports.
        let extremes = call(
            &tools,
            "price_extremes",
            json!({"start_date": today.to_string(), "market": "da", "node_type": "hub", "limit": 3}),
        )
        .await;
        assert_eq!(extremes["highest"].as_array().map(Vec::len), Some(3), "{extremes}");
        let first = &extremes["highest"][0];
        let parts = first["energy"].as_f64().unwrap()
            + first["mcc"].as_f64().unwrap()
            + first["mlc"].as_f64().unwrap();
        assert!((parts - first["lmp"].as_f64().unwrap()).abs() < 0.02, "{first}");
        let above = call(
            &tools,
            "price_extremes",
            json!({"start_date": today.to_string(), "market": "da", "above": -1000.0, "limit": 5}),
        )
        .await;
        assert_eq!(
            above["hours_crossing"].as_u64(),
            above["nodes_crossing"].as_u64().map(|n| n * 24),
            "every hour of every node is above -$1,000: {above}"
        );

        let cons = call(&tools, "binding_constraints", json!({"date": today.to_string()})).await;
        assert!(cons["da"]["constraints"].is_array(), "{cons}");
        assert!(cons["rt_now"]["constraints"].is_array(), "{cons}");

        let reserves = call(&tools, "reserves", json!({})).await;
        assert!(reserves["zones"].as_array().is_some_and(|z| !z.is_empty()));
        let tomorrow = (today + chrono::TimeDelta::days(1)).to_string();
        let da_reserves = call(&tools, "reserves", json!({"date": tomorrow, "market": "da"})).await;
        let spin = &da_reserves["da"]["miso_wide"]["spinning"];
        assert_eq!(spin.as_array().map(Vec::len), Some(24), "{da_reserves}");
        assert_eq!(da_reserves["da"]["report"], "DA ex-post");
        assert!(da_reserves["short_term_reserve_requirement"].is_array(), "{da_reserves}");
        let past = call(&tools, "reserves", json!({"date": "2026-09-30", "market": "rt"})).await;
        assert!(past["rt"]["miso_wide"]["regulation"].is_array(), "{past}");

        let load = call(&tools, "load", json!({"date": "2026-10-01"})).await;
        assert_eq!(load["mtlf"]["MISO"].as_array().map(Vec::len), Some(24), "{load}");
        assert_eq!(load["actual"]["MISO"].as_array().map(Vec::len), Some(24), "{load}");
        assert!(load["mtlf_peak_miso"]["mw"].as_f64().is_some_and(|mw| mw > 40_000.0));
        let next = mt_ai::tools::run(&tools, "load", json!({})).await;
        assert!(next.is_ok() || next.is_err_and(|e| e.contains("does not cover")), "tomorrow from the replayed report");

        let ahead = call(
            &tools,
            "lmp_series",
            json!({"node": "MINN.HUB", "start_date": tomorrow, "market": "both"}),
        )
        .await;
        assert!(ahead["days"][0]["da"].is_object(), "{ahead}");
        assert!(ahead["days"][0].get("rt").is_none(), "no RT for a day not begun: {ahead}");

        let tomorrow_da = call(&tools, "price_extremes", json!({"start_date": tomorrow, "market": "da", "limit": 2})).await;
        assert_eq!(tomorrow_da["days_read"], 1, "tomorrow's DA is searchable: {tomorrow_da}");

        let system = call(&tools, "system_conditions", json!({})).await;
        let first_hour = &system["wind_solar"]["hours"][0][0];
        assert!(first_hour.as_str().is_some_and(|t| t.len() == 16), "dated rows: {first_hour}");
        for part in ["load", "fuel_mix", "wind_solar", "interchange", "ace", "north_south_transfer", "capacity"] {
            assert!(system[part].get("error").is_none(), "{part}: {}", system[part]);
        }

        let outages = call(&tools, "outages", json!({})).await;
        assert!(outages["days"].as_array().is_some_and(|d| !d.is_empty()));

        let wx = call(&tools, "weather", json!({"place": "Indiana"})).await;
        assert_eq!(wx["zone"], "LRZ 6", "{wx}");
        assert!(wx["every_three_hours"].as_array().is_some_and(|h| !h.is_empty()));
        let all = call(&tools, "weather", json!({})).await;
        assert_eq!(all["cities"].as_array().map(Vec::len), Some(mt_nws::MISO_CITIES.len()));

        let gas = call(&tools, "gas", json!({"days": 5})).await;
        assert_eq!(gas["latest_first"].as_array().map(Vec::len), Some(5), "{gas}");

        let fcst = call(&tools, "forecast", json!({"node": "MINN.HUB"})).await;
        assert_eq!(fcst["hours"].as_array().map(Vec::len), Some(24), "{fcst}");
        assert!(fcst["models"].as_array().is_some_and(|m| m.len() > 2));
        let sec = call(&tools, "forecast", json!({"security": "XLU US"})).await;
        assert!(sec["path_by_trading_day"].is_array(), "{sec}");

        let q = call(
            &tools,
            "securities",
            json!({"symbols": ["xlu", "XEL US"], "bar_days": 10, "beta": true}),
        )
        .await;
        assert_eq!(q["securities"][1]["symbol"], "XEL US", "{q}");
        assert!(q["securities"][0]["last"].is_number(), "{q}");
        assert!(q["securities"][0]["daily_bars"]["rows"].is_array());
        assert!(q["securities"][0]["beta_vs_spy_1y"]["beta"].is_number(), "{q}");

        let hidden = mt_ai::tools::run(&tools, "paper_account", json!({})).await;
        assert!(hidden.unwrap_err().contains("has not let"));
        let account = call(&sharing, "paper_account", json!({})).await;
        assert_eq!(account["mode"], "paper");
        assert!(account["positions"].as_array().is_some_and(|p| !p.is_empty()), "{account}");

        // A past day's operating feeds come from what the terminal kept: here
        // a cache holding a day of five-minute load.
        let dir = std::env::temp_dir().join(format!("mt-ops-{}", std::process::id()));
        let cache = mt_data::DiskCache::new(&dir);
        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).expect("date");
        let kept = mt_data::issued::Issued {
            series: vec!["Load".into()],
            versions: (0..288)
                .map(|i| mt_data::issued::Version {
                    issued: day.and_hms_opt(23, 59, 0).expect("time"),
                    target: day.and_hms_opt(0, 0, 0).expect("time")
                        + chrono::TimeDelta::minutes(5 * i),
                    values: vec![Some(60_000.0 + i as f64 * 100.0)],
                })
                .collect(),
        };
        mt_data::issued::write_day(&cache, mt_miso::ops::LOAD, day, &kept).expect("written");
        let with_cache = Lookups::new(
            DataHub::new(
                tokio::runtime::Handle::current(),
                FetchCtx::new(
                    Arc::new(FixtureTransport::new(&fixtures)),
                    Some(cache),
                    FetchCtxOptions::default(),
                    EventLog::default(),
                ),
            ),
            &AppConfig::default(),
        );
        let past = call(&with_cache, "system_conditions", json!({"date": day.to_string()})).await;
        assert_eq!(past["load"]["intervals"], 288, "{past}");
        assert_eq!(past["load"]["peak"]["at"], "2026-09-30 23:55");
        assert_eq!(past["load"]["hourly_mean"][0], 60_550.0);
        assert!(past.get("fuel_mix_hourly_mean").is_none(), "nothing kept, nothing claimed");
        let _ = std::fs::remove_dir_all(&dir);

        let news = call(&tools, "news", json!({"days": 21})).await;
        assert!(news["headlines"].as_array().is_some_and(|h| !h.is_empty()), "{news}");
    });

    let asked = transport.asked.lock().clone();
    assert!(!asked.is_empty());
    for (method, url) in &asked {
        assert_eq!(*method, Method::Get, "a lookup sent {method} {url}");
        assert!(
            !url.contains("/v2/orders"),
            "no lookup reads or writes orders: {url}"
        );
    }
}
