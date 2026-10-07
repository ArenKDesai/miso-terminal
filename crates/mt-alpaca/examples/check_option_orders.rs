//! Checks option orders against Alpaca's live **paper** API: one contract,
//! then a two-leg spread (Alpaca's `mleg` class), each priced so it cannot
//! fill. Alpaca must accept both as the order desk sends them (with
//! `position_intent`s, and the spread's legs, ratios and signed net limit),
//! answer in a shape the parsers read (the spread with its legs nested, under
//! `nested=true` in the order list too), and find each by its client id.
//! Every order it places is cancelled before it exits, and the audit log must
//! hold no keys. Run by hand with a paper account's keys; it refuses anything
//! but the paper endpoint.
//!
//! ```powershell
//! $env:APCA_API_KEY_ID = "PK…"; $env:APCA_API_SECRET_KEY = "…"
//! cargo run -p mt-alpaca --example check_option_orders
//! ```
//!
//! It buys one XLU call about 10% out of the money at $0.01, and a 1×1 call
//! spread just above the money at a net debit of $0.01, both good for the day.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mt_alpaca::orders::parse_order;
use mt_alpaca::{
    ActionState, Alpaca, AuditLog, KEY_ID, OrderDesk, Outcome, SECRET_KEY, new_client_order_id,
};
use mt_core::account::{OrderSide, to_f64};
use mt_core::guard::Approved;
use mt_core::money::Decimal;
use mt_core::options::{Leg, PositionIntent, chain_rows, is_monthly};
use mt_core::order::{Order, OrderRequest, OrderType, TimeInForce};
use mt_data::{
    EventLog, FetchCtx, FetchCtxOptions, HttpTransport, MemorySecrets, Query, Request, Secret,
};
use serde_json::Value;

type Error = Box<dyn std::error::Error>;

const UNDERLYING: &str = "XLU";

fn keys() -> Result<(String, String), Error> {
    match (
        std::env::var("APCA_API_KEY_ID"),
        std::env::var("APCA_API_SECRET_KEY"),
    ) {
        (Ok(id), Ok(secret)) if !id.trim().is_empty() && !secret.trim().is_empty() => {
            Ok((id.trim().to_owned(), secret.trim().to_owned()))
        }
        // Else the paper keys stored in SET (Windows Credential Manager).
        _ => {
            let store = mt_data::os_store("miso-terminal");
            match (store.get(KEY_ID)?, store.get(SECRET_KEY)?) {
                (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => {
                    println!("using the paper keys stored in SET");
                    Ok((id.expose().to_owned(), secret.expose().to_owned()))
                }
                _ => Err(
                    "no paper keys: set APCA_API_KEY_ID and APCA_API_SECRET_KEY, \
                          or store them in SET"
                        .into(),
                ),
            }
        }
    }
}

fn settle(desk: &OrderDesk, id: &str) -> Outcome {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(o) = desk.outcome(id).filter(|o| !o.is_pending()) {
            return o;
        }
        if Instant::now() > deadline {
            return Outcome::Unknown {
                reason: "no outcome after a minute".into(),
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn settle_action(desk: &OrderDesk, key: &str) -> ActionState {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match desk.action(key) {
            Some(ActionState::Working) | None if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Some(s) => return s,
            None => return ActionState::Failed("never started".into()),
        }
    }
}

fn request(
    id: String,
    symbol: &str,
    legs: Vec<Leg>,
    intent: Option<PositionIntent>,
) -> OrderRequest {
    OrderRequest {
        client_order_id: id,
        symbol: symbol.into(),
        side: OrderSide::Buy,
        qty: Decimal::ONE,
        order_type: OrderType::Limit,
        limit_price: Some(Decimal::new(1, 2)),
        stop_price: None,
        tif: TimeInForce::Day,
        extended_hours: false,
        position_intent: intent,
        legs,
    }
}

fn main() -> Result<(), Error> {
    let (id, secret) = keys()?;
    let alpaca = Alpaca::new(&mt_alpaca::MarketsConfig::default(), true);
    let trading = alpaca.endpoints().trading.clone();
    if mt_data::host_of(&trading) != "paper-api.alpaca.markets" {
        return Err(format!("refusing to run against {trading}: paper only").into());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let ctx = FetchCtx::new(
        Arc::new(HttpTransport::new(concat!(
            "MISO-Terminal-checks/",
            env!("CARGO_PKG_VERSION")
        ))?),
        None,
        FetchCtxOptions {
            budgets: mt_alpaca::budgets(),
            secrets: Arc::new(MemorySecrets::with([
                (KEY_ID, Secret::new(id.clone())),
                (SECRET_KEY, Secret::new(secret.clone())),
            ])),
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let desk = OrderDesk::new(
        &alpaca,
        ctx.clone(),
        rt.handle().clone(),
        AuditLog::in_memory(),
    );
    // These checks exercise the desk, not the guardrails: their orders are
    // sent unreviewed (a feature only examples and tests enable).
    desk.set_enabled(true);

    let account = rt.block_on(alpaca.account().fetch(ctx.clone(), None))?;
    let level = account
        .options_trading_level
        .or(account.options_approved_level);
    println!(
        "paper account {}: {}, options level {level:?}",
        mt_core::account::mask_number(&account.number),
        account.status
    );
    if !account.restrictions().is_empty() {
        return Err(format!("the account cannot trade: {:?}", account.restrictions()).into());
    }

    // A chain: the soonest monthly expiry a week or more out.
    let snaps = rt.block_on(alpaca.snapshots([UNDERLYING]).fetch(ctx.clone(), None))?;
    let spot = snaps
        .get(UNDERLYING)
        .and_then(|s| s.last())
        .ok_or("no last price for XLU")?;
    let contracts = rt.block_on(alpaca.option_contracts(UNDERLYING).fetch(ctx.clone(), None))?;
    let today = mt_core::exchange::now_exchange().date_naive();
    let expiry = contracts
        .expiries()
        .into_iter()
        .find(|d| is_monthly(*d) && *d >= today + chrono::Duration::days(7))
        .ok_or("no monthly expiry a week out")?;
    let rows = chain_rows(
        UNDERLYING,
        expiry,
        contracts.on(expiry).map(|c| c.symbol.as_str()),
    );
    let call_at = |target: f64| {
        rows.iter()
            .filter(|r| r.call.is_some())
            .min_by(|a, b| {
                (to_f64(a.strike) - target)
                    .abs()
                    .total_cmp(&(to_f64(b.strike) - target).abs())
            })
            .and_then(|r| r.call.clone())
    };
    let otm = call_at(spot * 1.1).ok_or("no call 10% out of the money")?;
    let near = rows
        .iter()
        .position(|r| to_f64(r.strike) >= spot && r.call.is_some())
        .ok_or("no call at the money")?;
    let (low, high) = (
        rows[near].call.clone().ok_or("no call")?,
        rows.get(near + 1)
            .and_then(|r| r.call.clone())
            .ok_or("no call a strike up")?,
    );
    println!("{UNDERLYING} {spot}: expiry {expiry}, single {otm}, spread {low} / {high}");

    let mut placed: Vec<String> = Vec::new();
    let result = run(
        &rt,
        &ctx,
        &desk,
        &trading,
        level,
        (&otm, &low, &high),
        &mut placed,
    );

    // Whatever happened, cancel what was placed.
    for order_id in &placed {
        desk.cancel(order_id);
        let state = settle_action(&desk, &OrderDesk::cancel_key(order_id));
        println!("cleanup: cancel {order_id}: {state:?}");
    }
    let audit = desk.audit().recent().join("\n");
    if audit.contains(&id) || audit.contains(&secret) {
        return Err("a key reached the audit log".into());
    }
    println!(
        "audit log: {} entries, no keys",
        desk.audit().recent().len()
    );
    result
}

fn run(
    rt: &tokio::runtime::Runtime,
    ctx: &FetchCtx,
    desk: &OrderDesk,
    trading: &str,
    level: Option<u8>,
    (otm, low, high): (&str, &str, &str),
    placed: &mut Vec<String>,
) -> Result<(), Error> {
    let get = |url: String| -> Result<Value, Error> {
        let req = Request::get(url)
            .secret_header("APCA-API-KEY-ID", ctx.secret(KEY_ID)?)
            .secret_header("APCA-API-SECRET-KEY", ctx.secret(SECRET_KEY)?);
        let resp = rt.block_on(ctx.send(req))?;
        Ok(serde_json::from_slice(&resp.body)?)
    };
    let by_client_id = |cid: &str| -> Result<Order, Error> {
        let v = get(format!(
            "{trading}/v2/orders:by_client_order_id?client_order_id={cid}"
        ))?;
        parse_order(&v).ok_or_else(|| format!("looking up {cid}: {v}").into())
    };

    // 1. One contract, buy to open.
    let single = request(
        new_client_order_id(),
        otm,
        Vec::new(),
        Some(PositionIntent::BuyToOpen),
    );
    desk.submit(Approved::unreviewed(single.clone()))?;
    let order = match settle(desk, &single.client_order_id) {
        Outcome::Accepted(o) => *o,
        other => return Err(format!("placing one contract: {other:?}").into()),
    };
    placed.push(order.id.clone());
    println!(
        "single: placed {} ({}), intent {:?}",
        order.id,
        order.status.label(),
        order.position_intent
    );
    if !order.is_option() || order.symbol != otm {
        return Err(format!("the answer is not the contract: {order:?}").into());
    }
    let found = by_client_id(&single.client_order_id)?;
    if found.id != order.id {
        return Err(format!("lookup found {} instead of {}", found.id, order.id).into());
    }
    println!("single: found by client id");

    // 2. A spread, as one mleg order.
    if level.is_some_and(|l| l < 3) {
        println!("spread: skipped, the account's options level is below 3");
    } else {
        let spread = request(
            new_client_order_id(),
            "",
            vec![
                Leg {
                    intent: Some(PositionIntent::BuyToOpen),
                    ..Leg::new(low, OrderSide::Buy, 1)
                },
                Leg {
                    intent: Some(PositionIntent::SellToOpen),
                    ..Leg::new(high, OrderSide::Sell, 1)
                },
            ],
            None,
        );
        desk.submit(Approved::unreviewed(spread.clone()))?;
        let order = match settle(desk, &spread.client_order_id) {
            Outcome::Accepted(o) => *o,
            other => return Err(format!("placing the spread: {other:?}").into()),
        };
        placed.push(order.id.clone());
        println!(
            "spread: placed {} ({}), class {:?}, {} legs: {}",
            order.id,
            order.status.label(),
            order.order_class,
            order.legs.len(),
            order
                .strategy_legs()
                .iter()
                .map(Leg::describe)
                .collect::<Vec<_>>()
                .join(" / ")
        );
        if !order.is_multi_leg() || order.legs.len() != 2 {
            return Err(format!("the answer is not a two-leg spread: {order:?}").into());
        }
        let found = by_client_id(&spread.client_order_id)?;
        if found.id != order.id || found.legs.len() != 2 {
            return Err(format!("the spread looked up: {found:?}").into());
        }
        println!("spread: found by client id, with its legs");
        // The order list rolls the legs up under it, as ORD reads it.
        let list = rt.block_on(Alpaca::default().orders().fetch(ctx.clone(), None))?;
        let listed = list
            .iter()
            .find(|o| o.id == order.id)
            .ok_or("the spread is not in the order list")?;
        if listed.legs.len() != 2 {
            return Err(format!("the order list's spread: {listed:?}").into());
        }
        // A leg listed as an order of its own as well would count twice.
        if let Some(dup) = list
            .iter()
            .find(|o| listed.legs.iter().any(|l| l.id == o.id))
        {
            return Err(format!("a leg is also listed on its own: {dup:?}").into());
        }
        println!(
            "spread: in the order list as one order with {} legs",
            listed.legs.len()
        );
    }

    // 3. Cancel them and wait until Alpaca says so.
    let mut open = Vec::new();
    for id in placed.iter() {
        desk.cancel(id);
        match settle_action(desk, &OrderDesk::cancel_key(id)) {
            ActionState::Done(_) => open.push(id.clone()),
            ActionState::Failed(m) => return Err(format!("cancelling {id}: {m}").into()),
            ActionState::Working => unreachable!("settled"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    for id in &open {
        loop {
            let status = parse_order(&get(format!("{trading}/v2/orders/{id}"))?).map(|o| o.status);
            if status.as_ref().is_some_and(|s| s.is_final()) {
                println!("{id}: {}", status.map(|s| s.label()).unwrap_or_default());
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("{id} still {status:?} after 30 s").into());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    placed.clear();
    println!("ok: one contract and one spread placed, found and cancelled");
    Ok(())
}
