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
    let tools = Lookups::new(hub, &AppConfig::default());

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
            "news"
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

        let system = call(&tools, "system_conditions", json!({})).await;
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

        let news = call(&tools, "news", json!({"days": 21})).await;
        assert!(news["headlines"].as_array().is_some_and(|h| !h.is_empty()), "{news}");
    });

    let asked = transport.asked.lock().clone();
    assert!(!asked.is_empty());
    for (method, url) in &asked {
        assert_eq!(*method, Method::Get, "a lookup sent {method} {url}");
        assert!(
            !url.contains("alpaca.markets"),
            "no lookup here touches the broker: {url}"
        );
    }
}
