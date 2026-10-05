//! Checks the order desk against Alpaca's live **paper** API: places one
//! order that cannot fill, confirms Alpaca refuses a second order with the
//! same `client_order_id` (what the desk's duplicate protection rests on),
//! finds it by that id, replaces it, cancels it, and checks the audit log
//! holds no keys. Run by hand in the drift workflow (`paper_orders`) with the
//! CI account's keys; it refuses anything but the paper endpoint.
//!
//! ```powershell
//! $env:APCA_API_KEY_ID = "PK…"; $env:APCA_API_SECRET_KEY = "…"
//! cargo run -p mt-alpaca --example check_paper_orders
//! ```
//!
//! The order buys one share of XLU at half its last price, good for the day,
//! so it never fills; whatever happens, every order it placed is cancelled
//! before it exits.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mt_alpaca::orders::{order_body, parse_order};
use mt_alpaca::{
    ActionState, Alpaca, AuditLog, KEY_ID, OrderDesk, Outcome, Replacement, SECRET_KEY,
    new_client_order_id,
};
use mt_core::account::OrderSide;
use mt_core::money::{Decimal, RoundingStrategy, round_to_tick};
use mt_core::order::{OrderRequest, OrderType, PENNY, TimeInForce};
use mt_data::{
    EventLog, FetchCtx, FetchCtxOptions, HttpTransport, MemorySecrets, Query, Request, Secret,
};
use serde_json::Value;

type Error = Box<dyn std::error::Error>;

const SYMBOL: &str = "XLU";

fn keys() -> Result<(String, String), Error> {
    match (
        std::env::var("APCA_API_KEY_ID"),
        std::env::var("APCA_API_SECRET_KEY"),
    ) {
        (Ok(id), Ok(secret)) if !id.trim().is_empty() && !secret.trim().is_empty() => {
            Ok((id.trim().to_owned(), secret.trim().to_owned()))
        }
        _ => Err("set APCA_API_KEY_ID and APCA_API_SECRET_KEY to a paper account's keys".into()),
    }
}

/// Wait for the desk to settle an order.
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

    let account = rt.block_on(alpaca.account().fetch(ctx.clone(), None))?;
    println!(
        "paper account {}: {}",
        mt_core::account::mask_number(&account.number),
        account.status
    );
    if !account.restrictions().is_empty() {
        return Err(format!("the account cannot trade: {:?}", account.restrictions()).into());
    }
    let snaps = rt.block_on(alpaca.snapshots([SYMBOL]).fetch(ctx.clone(), None))?;
    let last = snaps
        .get(SYMBOL)
        .and_then(|s| s.last())
        .and_then(|p| mt_core::account::from_f64(p, 4))
        .ok_or("no last price for XLU")?;
    let limit = round_to_tick(last / Decimal::TWO, PENNY, RoundingStrategy::ToZero);
    println!("{SYMBOL} last {last}; buying 1 at {limit}, which cannot fill");

    let mut placed: Vec<String> = Vec::new();
    let result = run(&rt, &ctx, &desk, &trading, limit, &mut placed);

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
    limit: Decimal,
    placed: &mut Vec<String>,
) -> Result<(), Error> {
    let authed = |req: Request| -> Result<Request, Error> {
        Ok(req
            .secret_header("APCA-API-KEY-ID", ctx.secret(KEY_ID)?)
            .secret_header("APCA-API-SECRET-KEY", ctx.secret(SECRET_KEY)?))
    };

    // 1. Place it through the desk.
    let req = OrderRequest {
        client_order_id: new_client_order_id(),
        symbol: SYMBOL.into(),
        side: OrderSide::Buy,
        qty: Decimal::ONE,
        order_type: OrderType::Limit,
        limit_price: Some(limit),
        stop_price: None,
        tif: TimeInForce::Day,
        extended_hours: false,
    };
    desk.submit(req.clone())?;
    let order = match settle(desk, &req.client_order_id) {
        Outcome::Accepted(o) => *o,
        other => return Err(format!("placing: {other:?}").into()),
    };
    placed.push(order.id.clone());
    println!("placed {} ({})", order.id, order.status.label());

    // 2. The desk refuses to send the same ticket again…
    if desk.submit(req.clone()).is_ok() {
        return Err("the desk sent a placed ticket again".into());
    }
    // …and so does Alpaca, which is what makes a resend after a lost answer safe.
    let dup = rt.block_on(ctx.send(authed(
        Request::post(format!("{trading}/v2/orders")).json(&order_body(&req))?,
    )?))?;
    let message = mt_alpaca::orders::error_message(&dup.body);
    if dup.status != 422 || !message.contains("client_order_id") {
        if dup.is_success()
            && let Some(o) = serde_json::from_slice::<Value>(&dup.body)
                .ok()
                .as_ref()
                .and_then(parse_order)
        {
            placed.push(o.id);
        }
        return Err(format!(
            "Alpaca accepted or answered a duplicate client_order_id unexpectedly: HTTP {} {message}",
            dup.status
        )
        .into());
    }
    println!("a second order with the same id: HTTP 422 ({message})");

    // 3. Found by its client id, as after a lost answer.
    let found = rt.block_on(ctx.send(authed(Request::get(format!(
        "{trading}/v2/orders:by_client_order_id?client_order_id={}",
        req.client_order_id
    )))?))?;
    let found = serde_json::from_slice::<Value>(&found.body)
        .ok()
        .as_ref()
        .and_then(parse_order)
        .ok_or("looking it up by client id failed")?;
    if found.id != order.id {
        return Err(format!("lookup found {} instead of {}", found.id, order.id).into());
    }
    println!("found by client id");

    // 4. Replace its limit (a new order with its own client id).
    let rep = Replacement {
        client_order_id: new_client_order_id(),
        limit_price: Some(limit + PENNY),
        ..Replacement::default()
    };
    desk.replace(&order.id, rep.clone())?;
    match settle(desk, &rep.client_order_id) {
        Outcome::Accepted(new) => {
            placed.push(new.id.clone());
            if new.replaces.as_deref() != Some(order.id.as_str()) {
                return Err(format!("the replacement does not name {}: {new:?}", order.id).into());
            }
            println!("replaced by {} at {:?}", new.id, new.limit_price);
        }
        // Alpaca may not replace an order that has not reached the market yet
        // (before the open); say so rather than fail.
        Outcome::Rejected { status, message } => {
            println!("replace refused (HTTP {status}: {message}); cancelling the original instead");
        }
        other => return Err(format!("replacing: {other:?}").into()),
    }

    // 5. Cancel what is open, and wait for Alpaca to say so.
    let mut open = Vec::new();
    for id in placed.iter() {
        desk.cancel(id);
        match settle_action(desk, &OrderDesk::cancel_key(id)) {
            ActionState::Done(_) => open.push(id.clone()),
            // A replaced order is no longer cancellable.
            ActionState::Failed(m) if m.contains("not cancelable") => {}
            ActionState::Failed(m) => return Err(format!("cancelling {id}: {m}").into()),
            ActionState::Working => unreachable!("settled"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    for id in &open {
        loop {
            let resp =
                rt.block_on(ctx.send(authed(Request::get(format!("{trading}/v2/orders/{id}")))?))?;
            let status = serde_json::from_slice::<Value>(&resp.body)
                .ok()
                .as_ref()
                .and_then(parse_order)
                .map(|o| o.status);
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
    println!("ok: placed once, duplicate refused, found, replaced, cancelled");
    Ok(())
}
