//! The order desk: the only code that places, replaces or cancels orders.
//!
//! It is not the data hub. Nothing is cached, nothing refreshes on a timer
//! and nothing is retried blindly. Every order carries a `client_order_id`
//! made before it is sent ([`new_client_order_id`]). When an answer does not
//! come (a timeout, a dropped connection, a server error), the desk asks
//! Alpaca for the order by that id before anything else happens: found, it
//! was placed; not found, it was not, and confirming the ticket again sends
//! it with the same id, which Alpaca refuses to accept twice. So an order can
//! never go in twice.
//!
//! The desk only acts when asked by a ticket's Confirm, the blotter's Cancel
//! and Replace, or the kill switch; nothing from outside the window reaches
//! it. Every request and answer goes to the [`AuditLog`].

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mt_core::order::{Order, OrderRequest};
use mt_data::{FetchCtx, FetchError, Method, Request, Response};
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::audit::AuditLog;
use crate::orders::{self, Replacement};
use crate::{AccountMode, Alpaca, authed};

/// How long an order request may take before the desk asks whether it arrived.
pub const ORDER_TIMEOUT: Duration = Duration::from_secs(10);
/// When to ask, after a lost answer: Alpaca may still be taking the order in.
pub const LOOKUP_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(6),
];
/// The kill switch's key in [`OrderDesk::action`].
pub const KILL: &str = "kill";

/// A fresh id for an order, unique across launches: `mt-<time>-<process>-<n>`.
/// Made from the real clock, never the terminal's frozen one.
pub fn new_client_order_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("mt-{nanos:x}-{:x}-{n}", std::process::id())
}

/// Ids the desk puts in URLs: letters, digits, `-` and `_` only.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What became of an order the desk was asked to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// On its way; no answer yet.
    Sending,
    /// No answer (or a server error): asking Alpaca whether it arrived.
    Checking { reason: String },
    /// Alpaca has it.
    Accepted(Box<Order>),
    /// Alpaca refused it (buying power, an unknown symbol…); nothing was placed.
    Rejected { status: u16, message: String },
    /// Alpaca has no order with this id: it was never placed. Sending it
    /// again, with the same id, is safe.
    NotPlaced { reason: String },
    /// Whether it arrived is not known (Alpaca could not be asked). Check
    /// again before anything else.
    Unknown { reason: String },
    /// Not sent at all: an offline replay, missing keys, the rate limit.
    NotSent(String),
}

impl Outcome {
    /// Still being worked out.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Sending | Self::Checking { .. })
    }

    /// Whether the same ticket may be sent (again): only when it is certain
    /// nothing was placed.
    pub fn may_send(&self) -> bool {
        matches!(
            self,
            Self::Rejected { .. } | Self::NotPlaced { .. } | Self::NotSent(_)
        )
    }
}

/// A cancel or the kill switch.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionState {
    Working,
    Done(String),
    Failed(String),
}

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub struct OrderDesk {
    inner: Arc<Inner>,
}

type Notify = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    ctx: FetchCtx,
    runtime: tokio::runtime::Handle,
    trading: String,
    mode: AccountMode,
    audit: AuditLog,
    timeout: Duration,
    lookup_delays: Vec<Duration>,
    state: Mutex<State>,
    notify: Mutex<Option<Notify>>,
}

#[derive(Default)]
struct State {
    /// By client order id.
    submissions: HashMap<String, Outcome>,
    /// `cancel:<order id>` and [`KILL`].
    actions: HashMap<String, ActionState>,
    generation: u64,
}

impl OrderDesk {
    /// A desk for `alpaca`'s account, sending through `ctx` (its request
    /// budget and event log, never its caches) on `runtime`.
    pub fn new(
        alpaca: &Alpaca,
        ctx: FetchCtx,
        runtime: tokio::runtime::Handle,
        audit: AuditLog,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                ctx,
                runtime,
                trading: alpaca.endpoints().trading.clone(),
                mode: alpaca.mode(),
                audit,
                timeout: ORDER_TIMEOUT,
                lookup_delays: LOOKUP_DELAYS.to_vec(),
                state: Mutex::default(),
                notify: Mutex::default(),
            }),
        }
    }

    /// Shorter waits, for tests.
    pub fn with_timing(self, timeout: Duration, lookup_delays: Vec<Duration>) -> Self {
        let inner = Arc::try_unwrap(self.inner)
            .unwrap_or_else(|_| panic!("with_timing must be called before the desk is shared"));
        Self {
            inner: Arc::new(Inner {
                timeout,
                lookup_delays,
                ..inner
            }),
        }
    }

    /// Called (from a background thread) whenever something changes.
    pub fn set_notify(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.notify.lock() = Some(Arc::new(f));
    }

    pub fn mode(&self) -> AccountMode {
        self.inner.mode
    }

    pub fn audit(&self) -> &AuditLog {
        &self.inner.audit
    }

    /// Whether orders can be sent at all: not from an offline replay.
    pub fn can_send(&self) -> Result<(), String> {
        if self.inner.ctx.is_live() {
            Ok(())
        } else {
            Err("Offline replay: orders are not sent to Alpaca.".into())
        }
    }

    /// Goes up whenever an order, cancel or kill finishes, so the app can
    /// re-read the account and orders.
    pub fn generation(&self) -> u64 {
        self.inner.state.lock().generation
    }

    pub fn outcome(&self, client_order_id: &str) -> Option<Outcome> {
        self.inner
            .state
            .lock()
            .submissions
            .get(client_order_id)
            .cloned()
    }

    pub fn action(&self, key: &str) -> Option<ActionState> {
        self.inner.state.lock().actions.get(key).cloned()
    }

    /// The key [`OrderDesk::action`] reports a cancel under.
    pub fn cancel_key(order_id: &str) -> String {
        format!("cancel:{order_id}")
    }

    /// Claim `id` for a send, unless it is already on its way or placed.
    fn claim(&self, id: &str) -> Result<(), String> {
        if !is_safe_id(id) {
            return Err(format!("Not a usable order id: {id:?}."));
        }
        self.can_send()?;
        let mut st = self.inner.state.lock();
        match st.submissions.get(id) {
            Some(o) if !o.may_send() => Err(match o {
                Outcome::Accepted(_) => "This order has been placed already.".to_owned(),
                Outcome::Unknown { .. } => {
                    "Whether this order arrived is not known yet: check again first.".to_owned()
                }
                _ => "This order is on its way already.".to_owned(),
            }),
            _ => {
                st.submissions.insert(id.to_owned(), Outcome::Sending);
                Ok(())
            }
        }
    }

    /// Send a new order. The outcome is reported under its
    /// `client_order_id`; sending the same id again is refused unless it is
    /// certain the first was not placed.
    pub fn submit(&self, req: OrderRequest) -> Result<(), String> {
        if let Some(p) = req.problems().into_iter().next() {
            return Err(p);
        }
        self.claim(&req.client_order_id)?;
        self.inner.notify();
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            let id = req.client_order_id.clone();
            let outcome = inner.place(&req).await;
            inner.finish(&id, outcome);
        });
        Ok(())
    }

    /// Replace an open order's quantity, prices or time in force. The
    /// replacement is a new order (Alpaca's rule) and is reported under the
    /// replacement's own `client_order_id`.
    pub fn replace(&self, order_id: &str, r: Replacement) -> Result<(), String> {
        if !is_safe_id(order_id) {
            return Err(format!("Not a usable order id: {order_id:?}."));
        }
        if r.qty.is_none() && r.limit_price.is_none() && r.stop_price.is_none() && r.tif.is_none() {
            return Err("Nothing to change.".into());
        }
        self.claim(&r.client_order_id)?;
        self.inner.notify();
        let (inner, order_id) = (self.inner.clone(), order_id.to_owned());
        self.inner.runtime.spawn(async move {
            let id = r.client_order_id.clone();
            let outcome = inner.replace(&order_id, &r).await;
            inner.finish(&id, outcome);
        });
        Ok(())
    }

    /// Ask Alpaca again whether an order whose fate is unknown arrived.
    pub fn check_again(&self, client_order_id: &str) {
        let id = client_order_id.to_owned();
        {
            let mut st = self.inner.state.lock();
            match st.submissions.get(&id) {
                Some(Outcome::Unknown { .. } | Outcome::NotPlaced { .. }) => {
                    st.submissions.insert(
                        id.clone(),
                        Outcome::Checking {
                            reason: "Checking again".into(),
                        },
                    );
                }
                _ => return,
            }
        }
        self.inner.notify();
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            let outcome = inner.lookup(&id, "Checked again".into()).await;
            inner.finish(&id, outcome);
        });
    }

    /// Cancel an open order. Asking twice is harmless.
    pub fn cancel(&self, order_id: &str) {
        let key = Self::cancel_key(order_id);
        match self.start_action(&key, is_safe_id(order_id)) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                self.inner.set_action(&key, ActionState::Failed(e));
                return;
            }
        }
        let (inner, order_id) = (self.inner.clone(), order_id.to_owned());
        self.inner.runtime.spawn(async move {
            let state = inner.cancel(&order_id).await;
            inner.finish_action(&key, state);
        });
    }

    /// The kill switch: cancel every open order and, if `flatten`, close
    /// every position at the market (outside the regular session the closing
    /// orders wait for the open). Reported under [`KILL`].
    pub fn kill(&self, flatten: bool) {
        match self.start_action(KILL, true) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                self.inner.set_action(KILL, ActionState::Failed(e));
                return;
            }
        }
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            let state = inner.kill(flatten).await;
            inner.finish_action(KILL, state);
        });
    }

    /// Mark an action as working. `Ok(false)`: it already is.
    fn start_action(&self, key: &str, valid: bool) -> Result<bool, String> {
        if !valid {
            return Err("Not a usable order id.".into());
        }
        self.can_send()?;
        let mut st = self.inner.state.lock();
        if st.actions.get(key) == Some(&ActionState::Working) {
            return Ok(false);
        }
        st.actions.insert(key.to_owned(), ActionState::Working);
        drop(st);
        self.inner.notify();
        Ok(true)
    }
}

impl Inner {
    fn notify(&self) {
        let f = self.notify.lock().clone();
        if let Some(f) = f {
            f();
        }
    }

    fn set(&self, id: &str, outcome: Outcome) {
        self.state.lock().submissions.insert(id.to_owned(), outcome);
        self.notify();
    }

    fn finish(&self, id: &str, outcome: Outcome) {
        {
            let mut st = self.state.lock();
            st.submissions.insert(id.to_owned(), outcome);
            st.generation += 1;
        }
        self.notify();
    }

    fn set_action(&self, key: &str, state: ActionState) {
        self.state.lock().actions.insert(key.to_owned(), state);
        self.notify();
    }

    fn finish_action(&self, key: &str, state: ActionState) {
        {
            let mut st = self.state.lock();
            st.actions.insert(key.to_owned(), state);
            st.generation += 1;
        }
        self.notify();
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.trading)
    }

    /// One request and its answer, both written to the audit log. `Err` is
    /// for no answer at all (a timeout or a network failure).
    async fn exchange(
        &self,
        action: &str,
        client_order_id: Option<&str>,
        req: Request,
        body: Option<&Value>,
    ) -> Result<Response, String> {
        let entry = |extra: (&str, Value)| {
            let mut e = json!({
                "account": self.mode.key(),
                "action": action,
                "method": req.method.as_str(),
                "url": req.url,
            });
            if let Some(id) = client_order_id {
                e["client_order_id"] = json!(id);
            }
            e[extra.0] = extra.1;
            e
        };
        self.audit
            .record(entry(("request", body.cloned().unwrap_or(Value::Null))));
        let answer = tokio::time::timeout(self.timeout, self.ctx.send(req.clone())).await;
        match answer {
            Err(_) => {
                let reason = format!("No answer from Alpaca in {} s", self.timeout.as_secs_f32());
                self.audit.record(entry(("error", json!(reason))));
                Err(reason)
            }
            Ok(Err(e)) => {
                let reason = no_answer(&e);
                self.audit.record(entry(("error", json!(reason))));
                Err(reason)
            }
            Ok(Ok(resp)) => {
                let shown = serde_json::from_slice::<Value>(&resp.body)
                    .unwrap_or_else(|_| json!(resp.excerpt(500)));
                self.audit.record(entry((
                    "response",
                    json!({"status": resp.status, "body": shown}),
                )));
                Ok(resp)
            }
        }
    }

    async fn place(&self, req: &OrderRequest) -> Outcome {
        let body = orders::order_body(req);
        let http = match Request::post(self.url("/v2/orders"))
            .json(&body)
            .map_err(|e| e.to_string())
            .and_then(|r| authed(&self.ctx, r).map_err(|e| keys_problem(&e)))
        {
            Ok(r) => r,
            Err(e) => return Outcome::NotSent(e),
        };
        let id = &req.client_order_id;
        match self.exchange("place", Some(id), http, Some(&body)).await {
            Err(reason) => self.lookup(id, reason).await,
            Ok(resp) => self.judge(id, resp).await,
        }
    }

    async fn replace(&self, order_id: &str, r: &Replacement) -> Outcome {
        let body = orders::replace_body(r);
        let http = match Request::new(Method::Patch, self.url(&format!("/v2/orders/{order_id}")))
            .json(&body)
            .map_err(|e| e.to_string())
            .and_then(|r| authed(&self.ctx, r).map_err(|e| keys_problem(&e)))
        {
            Ok(r) => r,
            Err(e) => return Outcome::NotSent(e),
        };
        let id = &r.client_order_id;
        match self.exchange("replace", Some(id), http, Some(&body)).await {
            Err(reason) => self.lookup(id, reason).await,
            Ok(resp) => self.judge(id, resp).await,
        }
    }

    /// What an answer to a place or replace means.
    async fn judge(&self, id: &str, resp: Response) -> Outcome {
        let message = orders::error_message(&resp.body);
        match resp.status {
            200..=299 => match serde_json::from_slice::<Value>(&resp.body)
                .ok()
                .as_ref()
                .and_then(orders::parse_order)
            {
                Some(o) => Outcome::Accepted(Box::new(o)),
                None => {
                    self.lookup(id, "Alpaca took the order, but its answer could not be read".into())
                        .await
                }
            },
            // A resend of an order that did arrive: find it.
            422 if message.contains("client_order_id") => {
                self.lookup(id, format!("Alpaca already has an order with this id ({message})"))
                    .await
            }
            429 => Outcome::NotSent(
                "Alpaca's rate limit refused the request; nothing was placed. Try again in a minute."
                    .into(),
            ),
            status @ 400..=499 => Outcome::Rejected { status, message },
            status => {
                self.lookup(id, format!("HTTP {status} from Alpaca ({message})"))
                    .await
            }
        }
    }

    /// Ask for the order by its client id, a few times: Alpaca may still be
    /// taking it in.
    async fn lookup(&self, id: &str, reason: String) -> Outcome {
        self.set(
            id,
            Outcome::Checking {
                reason: reason.clone(),
            },
        );
        let url = self.url(&format!(
            "/v2/orders:by_client_order_id?client_order_id={id}"
        ));
        let mut missing = false;
        let mut problem = String::new();
        for delay in &self.lookup_delays {
            tokio::time::sleep(*delay).await;
            let http = match authed(&self.ctx, Request::get(&url)) {
                Ok(r) => r,
                Err(e) => {
                    return Outcome::Unknown {
                        reason: format!("{reason}. {}", keys_problem(&e)),
                    };
                }
            };
            match self.exchange("lookup", Some(id), http, None).await {
                Ok(resp) if resp.is_success() => {
                    if let Some(o) = serde_json::from_slice::<Value>(&resp.body)
                        .ok()
                        .as_ref()
                        .and_then(orders::parse_order)
                    {
                        return Outcome::Accepted(Box::new(o));
                    }
                    missing = false;
                    problem = "its answer could not be read".into();
                }
                Ok(resp) if resp.status == 404 => missing = true,
                Ok(resp) => {
                    missing = false;
                    problem = format!(
                        "HTTP {}: {}",
                        resp.status,
                        orders::error_message(&resp.body)
                    );
                }
                Err(e) => {
                    missing = false;
                    problem = e;
                }
            }
        }
        if missing {
            Outcome::NotPlaced {
                reason: format!(
                    "{reason}. Alpaca has no order with this ticket's id, so it was not placed. \
                     Confirming again sends it with the same id, so it cannot go in twice."
                ),
            }
        } else {
            Outcome::Unknown {
                reason: format!(
                    "{reason}. Could not find out whether it arrived ({problem}). Check again before sending anything else."
                ),
            }
        }
    }

    async fn cancel(&self, order_id: &str) -> ActionState {
        let http = match authed(
            &self.ctx,
            Request::delete(self.url(&format!("/v2/orders/{order_id}"))),
        ) {
            Ok(r) => r,
            Err(e) => return ActionState::Failed(keys_problem(&e)),
        };
        match self.exchange("cancel", None, http, None).await {
            Ok(r) if r.is_success() => ActionState::Done("Cancel requested.".into()),
            Ok(r) => ActionState::Failed(format!(
                "Alpaca did not cancel it: {}",
                orders::error_message(&r.body)
            )),
            Err(reason) => ActionState::Failed(format!(
                "{reason}: the cancel may not have arrived. Cancelling again is safe."
            )),
        }
    }

    async fn kill(&self, flatten: bool) -> ActionState {
        let mut said = Vec::new();
        type Say = fn(usize) -> String;
        let cancel: (&str, &str, Say) = ("cancel_all", "/v2/orders", |n| {
            format!("Cancelled {}", plural(n, "open order", "open orders"))
        });
        let close: (&str, &str, Say) = ("close_all", "/v2/positions?cancel_orders=true", |n| {
            format!(
                "closing {} at the market",
                plural(n, "position", "positions")
            )
        });
        let steps = if flatten {
            vec![cancel, close]
        } else {
            vec![cancel]
        };
        for (action, path, what) in steps {
            let http = match authed(&self.ctx, Request::delete(self.url(path))) {
                Ok(r) => r,
                Err(e) => return ActionState::Failed(keys_problem(&e)),
            };
            match self.exchange(action, None, http, None).await {
                Ok(r) if r.is_success() => {
                    let items: Vec<Value> = serde_json::from_slice::<Value>(&r.body)
                        .ok()
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default();
                    let failed = items
                        .iter()
                        .filter(|i| i.get("status").and_then(Value::as_u64).unwrap_or(200) >= 300)
                        .count();
                    let mut s = what(items.len() - failed);
                    if failed > 0 {
                        s.push_str(&format!(" ({failed} refused)"));
                    }
                    said.push(s);
                }
                Ok(r) => {
                    return ActionState::Failed(format!(
                        "{}Alpaca refused: {}",
                        prefix(&said),
                        orders::error_message(&r.body)
                    ));
                }
                Err(reason) => {
                    return ActionState::Failed(format!(
                        "{}{reason}. Check ORD; using the kill switch again is safe.",
                        prefix(&said)
                    ));
                }
            }
        }
        ActionState::Done(format!("{}.", said.join("; ")))
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn prefix(done: &[String]) -> String {
    if done.is_empty() {
        String::new()
    } else {
        format!("{}; then ", done.join("; "))
    }
}

fn no_answer(e: &FetchError) -> String {
    format!("No answer from Alpaca ({e})")
}

fn keys_problem(e: &FetchError) -> String {
    match e {
        FetchError::Auth(_) => "No Alpaca keys: add them in SET.".into(),
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::str::FromStr;
    use std::time::Instant;

    use mt_core::account::OrderSide;
    use mt_core::money::Decimal;
    use mt_core::order::{OrderStatus, OrderType, TimeInForce};
    use mt_data::{
        BoxFuture, EventLog, FetchCtxOptions, MemorySecrets, Secret, SecretStore, Transport,
    };

    use crate::{KEY_ID, SECRET_KEY};

    /// What the fake broker does with the next order request.
    #[derive(Clone, Copy, Debug)]
    enum Post {
        Accept,
        /// Takes the order, but the answer never comes back.
        Lost,
        /// The connection drops before the order arrives.
        Vanish,
        /// Refuses it.
        Reject(u16, &'static str),
        /// A 502 from a proxy: the order never arrived.
        BadGateway,
    }

    /// A paper broker in memory: orders, duplicate client ids refused,
    /// cancels, replaces, cancel-all and close-all.
    #[derive(Default)]
    struct Broker {
        orders: Mutex<Vec<Value>>,
        posts: Mutex<VecDeque<Post>>,
        /// Fail this many lookups with a dropped connection.
        failing_lookups: Mutex<usize>,
        calls: Mutex<Vec<String>>,
        keys_seen: Mutex<Vec<String>>,
    }

    fn resp(status: u16, body: Value) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: if body.is_null() {
                Vec::new().into()
            } else {
                body.to_string().into_bytes().into()
            },
        }
    }

    impl Broker {
        fn order_count(&self) -> usize {
            self.orders.lock().len()
        }

        fn posts_made(&self) -> usize {
            self.calls
                .lock()
                .iter()
                .filter(|c| c.starts_with("POST /v2/orders"))
                .count()
        }

        fn store(&self, body: &Value) -> Value {
            let mut orders = self.orders.lock();
            let o = json!({
                "id": format!("ord-{}", orders.len() + 1),
                "client_order_id": body["client_order_id"],
                "symbol": body["symbol"], "qty": body["qty"], "side": body["side"],
                "type": body["type"], "time_in_force": body["time_in_force"],
                "limit_price": body.get("limit_price").cloned().unwrap_or(Value::Null),
                "status": "new", "filled_qty": "0", "asset_class": "us_equity",
                "order_class": body.get("order_class").cloned().unwrap_or(Value::Null),
                // Each leg comes back as an order of its own.
                "legs": body.get("legs").and_then(Value::as_array).map(|legs| {
                    legs.iter()
                        .enumerate()
                        .map(|(i, l)| {
                            let mut l = l.clone();
                            l["id"] = json!(format!("ord-{}-leg-{i}", orders.len() + 1));
                            l["status"] = json!("new");
                            l["filled_qty"] = json!("0");
                            l
                        })
                        .collect::<Vec<_>>()
                }),
                "created_at": "2026-10-02T18:00:00Z", "updated_at": "2026-10-02T18:00:00Z",
            });
            orders.push(o.clone());
            o
        }

        fn answer(&self, req: &Request) -> Result<Response, FetchError> {
            let path = req
                .url
                .split_once("alpaca.markets")
                .map_or(req.url.as_str(), |(_, p)| p);
            self.calls.lock().push(format!("{} {path}", req.method));
            if let Some(k) = req.header_value("APCA-API-KEY-ID") {
                self.keys_seen.lock().push(k.expose().to_owned());
            }
            let body: Value = req
                .body
                .as_ref()
                .and_then(|b| serde_json::from_slice(b).ok())
                .unwrap_or(Value::Null);
            let not_found = || resp(404, json!({"code": 40410000, "message": "order not found"}));
            match (req.method, path) {
                (Method::Post, "/v2/orders") => {
                    let dup = self
                        .orders
                        .lock()
                        .iter()
                        .any(|o| o["client_order_id"] == body["client_order_id"]);
                    if dup {
                        return Ok(resp(
                            422,
                            json!({"code": 40010001, "message": "client_order_id must be unique"}),
                        ));
                    }
                    match self.posts.lock().pop_front().unwrap_or(Post::Accept) {
                        Post::Accept => Ok(resp(200, self.store(&body))),
                        Post::Lost => {
                            self.store(&body);
                            Err(FetchError::Other("hang".into()))
                        }
                        Post::Vanish => Err(FetchError::Network("connection reset".into())),
                        Post::Reject(status, msg) => {
                            Ok(resp(status, json!({"code": 40310000, "message": msg})))
                        }
                        Post::BadGateway => Ok(resp(502, json!("<html>bad gateway</html>"))),
                    }
                }
                (Method::Get, p) if p.starts_with("/v2/orders:by_client_order_id") => {
                    {
                        let mut f = self.failing_lookups.lock();
                        if *f > 0 {
                            *f -= 1;
                            return Err(FetchError::Network("connection reset".into()));
                        }
                    }
                    let id = p.rsplit('=').next().unwrap_or_default();
                    Ok(self
                        .orders
                        .lock()
                        .iter()
                        .find(|o| o["client_order_id"] == id)
                        .map_or_else(not_found, |o| resp(200, o.clone())))
                }
                (Method::Delete, "/v2/orders") => {
                    let mut out = Vec::new();
                    for o in self.orders.lock().iter_mut() {
                        if o["status"] == "new" {
                            o["status"] = json!("canceled");
                            out.push(json!({"id": o["id"], "status": 200}));
                        }
                    }
                    Ok(resp(207, Value::Array(out)))
                }
                (Method::Delete, "/v2/positions?cancel_orders=true") => Ok(resp(
                    207,
                    json!([{"symbol": "XLU", "status": 200}, {"symbol": "UNG", "status": 200},
                           {"symbol": "HALT", "status": 403}]),
                )),
                (Method::Delete, p) if p.starts_with("/v2/orders/") => {
                    let id = &p["/v2/orders/".len()..];
                    let mut orders = self.orders.lock();
                    match orders.iter_mut().find(|o| o["id"] == id) {
                        Some(o) if o["status"] == "new" => {
                            o["status"] = json!("canceled");
                            Ok(resp(204, Value::Null))
                        }
                        Some(_) => Ok(resp(
                            422,
                            json!({"code": 42210000, "message": "order is not cancelable"}),
                        )),
                        None => Ok(not_found()),
                    }
                }
                (Method::Patch, p) if p.starts_with("/v2/orders/") => {
                    let id = p["/v2/orders/".len()..].to_owned();
                    let mut orders = self.orders.lock();
                    let Some(old) = orders.iter_mut().find(|o| o["id"] == id.as_str()) else {
                        return Ok(not_found());
                    };
                    old["status"] = json!("replaced");
                    let mut new = old.clone();
                    for k in ["qty", "limit_price", "client_order_id"] {
                        if let Some(v) = body.get(k) {
                            new[k] = v.clone();
                        }
                    }
                    new["status"] = json!("new");
                    new["replaces"] = json!(id);
                    new["id"] = json!(format!("ord-{}", orders.len() + 1));
                    orders.push(new.clone());
                    Ok(resp(200, new))
                }
                _ => Ok(not_found()),
            }
        }
    }

    impl Transport for Broker {
        fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
            let answer = self.answer(req);
            Box::pin(async move {
                match answer {
                    // The order went in; the answer is stuck somewhere.
                    Err(FetchError::Other(m)) if m == "hang" => {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        Err(FetchError::Network("gave up".into()))
                    }
                    other => other,
                }
            })
        }

        fn describe(&self) -> String {
            "fake broker".into()
        }
    }

    struct Setup {
        _rt: tokio::runtime::Runtime,
        broker: Arc<Broker>,
        desk: OrderDesk,
    }

    fn setup() -> Setup {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let broker = Arc::new(Broker::default());
        let secrets = Arc::new(MemorySecrets::default());
        secrets.set(KEY_ID, &Secret::new("PKSECRETID")).unwrap();
        secrets
            .set(SECRET_KEY, &Secret::new("SKVERYSECRET"))
            .unwrap();
        let ctx = FetchCtx::new(
            broker.clone(),
            None,
            FetchCtxOptions {
                secrets,
                polite_interval: Duration::ZERO,
                ..FetchCtxOptions::default()
            },
            EventLog::default(),
        );
        let desk = OrderDesk::new(
            &Alpaca::default(),
            ctx,
            rt.handle().clone(),
            AuditLog::in_memory(),
        )
        .with_timing(
            Duration::from_millis(100),
            vec![Duration::from_millis(5); 3],
        );
        Setup {
            _rt: rt,
            broker,
            desk,
        }
    }

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn ticket(id: &str) -> OrderRequest {
        OrderRequest {
            client_order_id: id.into(),
            symbol: "XLU".into(),
            side: OrderSide::Buy,
            qty: d("10"),
            order_type: OrderType::Limit,
            limit_price: Some(d("82.50")),
            stop_price: None,
            tif: TimeInForce::Day,
            extended_hours: false,
            position_intent: None,
            legs: Vec::new(),
        }
    }

    fn settle(desk: &OrderDesk, id: &str) -> Outcome {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match desk.outcome(id) {
                Some(o) if !o.is_pending() => return o,
                _ => {}
            }
            assert!(Instant::now() < deadline, "{id} never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn settle_action(desk: &OrderDesk, key: &str) -> ActionState {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match desk.action(key) {
                Some(ActionState::Working) | None => {}
                Some(s) => return s,
            }
            assert!(Instant::now() < deadline, "{key} never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn an_order_goes_in_once_and_is_audited_without_keys() {
        let s = setup();
        let generation = s.desk.generation();
        s.desk.submit(ticket("mt-a")).unwrap();
        let Outcome::Accepted(o) = settle(&s.desk, "mt-a") else {
            panic!("{:?}", s.desk.outcome("mt-a"));
        };
        assert_eq!(
            (o.symbol.as_str(), o.status.clone()),
            ("XLU", OrderStatus::New)
        );
        assert!(s.desk.generation() > generation);
        // Sending the same ticket again is refused here, before Alpaca.
        assert!(
            s.desk
                .submit(ticket("mt-a"))
                .unwrap_err()
                .contains("placed already")
        );
        assert_eq!((s.broker.order_count(), s.broker.posts_made()), (1, 1));
        assert_eq!(
            s.broker.keys_seen.lock()[0],
            "PKSECRETID",
            "keys go in headers"
        );
        let audit = s.desk.audit().recent().join("\n");
        assert!(audit.contains("\"action\":\"place\"") && audit.contains("\"status\":200"));
        assert!(audit.contains("\"limit_price\":\"82.5\""));
        assert!(!audit.contains("PKSECRETID") && !audit.contains("SKVERYSECRET"));
    }

    #[test]
    fn a_spread_goes_in_as_one_order() {
        use mt_core::options::Leg;
        let s = setup();
        let spread = OrderRequest {
            symbol: String::new(),
            qty: d("2"),
            limit_price: Some(d("0.85")),
            legs: vec![
                Leg::new("XLU261218C00045000", OrderSide::Buy, 1),
                Leg::new("XLU261218C00047000", OrderSide::Sell, 1),
            ],
            ..ticket("mt-spread")
        };
        s.desk.submit(spread).unwrap();
        let Outcome::Accepted(o) = settle(&s.desk, "mt-spread") else {
            panic!("{:?}", s.desk.outcome("mt-spread"));
        };
        assert!(o.is_multi_leg() && o.legs.len() == 2, "{o:?}");
        assert_eq!((s.broker.order_count(), s.broker.posts_made()), (1, 1));
        let audit = s.desk.audit().recent().join("\n");
        assert!(audit.contains("\"order_class\":\"mleg\""), "{audit}");
        // A spread Alpaca would refuse never leaves the desk.
        let lopsided = OrderRequest {
            symbol: String::new(),
            legs: vec![Leg::new("XLU261218C00045000", OrderSide::Buy, 1)],
            ..ticket("mt-spread-2")
        };
        assert!(s.desk.submit(lopsided).unwrap_err().contains("two to"));
        assert_eq!(s.broker.posts_made(), 1);
    }

    #[test]
    fn a_lost_answer_is_looked_up_not_resent() {
        let s = setup();
        s.broker.posts.lock().push_back(Post::Lost);
        s.desk.submit(ticket("mt-lost")).unwrap();
        let Outcome::Accepted(o) = settle(&s.desk, "mt-lost") else {
            panic!("{:?}", s.desk.outcome("mt-lost"));
        };
        assert_eq!(o.client_order_id, "mt-lost");
        assert_eq!((s.broker.order_count(), s.broker.posts_made()), (1, 1));
        let audit = s.desk.audit().recent().join("\n");
        assert!(audit.contains("No answer from Alpaca") && audit.contains("\"action\":\"lookup\""));
    }

    #[test]
    fn an_order_that_never_arrived_can_be_sent_again_with_the_same_id() {
        let s = setup();
        s.broker.posts.lock().push_back(Post::Vanish);
        s.desk.submit(ticket("mt-v")).unwrap();
        assert!(matches!(settle(&s.desk, "mt-v"), Outcome::NotPlaced { .. }));
        assert_eq!(s.broker.order_count(), 0);
        s.desk.submit(ticket("mt-v")).unwrap();
        assert!(matches!(settle(&s.desk, "mt-v"), Outcome::Accepted(_)));
        assert_eq!(s.broker.order_count(), 1);

        // The first send turns up late, after the lookups said "not placed":
        // the resend is refused by Alpaca as a duplicate and found instead.
        s.broker.posts.lock().push_back(Post::BadGateway);
        s.desk.submit(ticket("mt-late")).unwrap();
        assert!(matches!(
            settle(&s.desk, "mt-late"),
            Outcome::NotPlaced { .. }
        ));
        s.broker
            .store(&crate::orders::order_body(&ticket("mt-late")));
        s.desk.submit(ticket("mt-late")).unwrap();
        assert!(matches!(settle(&s.desk, "mt-late"), Outcome::Accepted(_)));
        assert_eq!(s.broker.order_count(), 2, "mt-v and mt-late, once each");
    }

    #[test]
    fn rejections_rate_limits_and_unknowns() {
        let s = setup();
        s.broker.posts.lock().extend([
            Post::Reject(403, "insufficient buying power"),
            Post::Reject(429, "too many"),
        ]);
        s.desk.submit(ticket("mt-r")).unwrap();
        assert_eq!(
            settle(&s.desk, "mt-r"),
            Outcome::Rejected {
                status: 403,
                message: "insufficient buying power".into()
            }
        );
        s.desk.submit(ticket("mt-429")).unwrap();
        assert!(matches!(settle(&s.desk, "mt-429"), Outcome::NotSent(_)));
        assert_eq!(s.broker.order_count(), 0);

        // The order arrived but nobody can say so: unknown, and blocked until checked.
        s.broker.posts.lock().push_back(Post::Lost);
        *s.broker.failing_lookups.lock() = 3;
        s.desk.submit(ticket("mt-u")).unwrap();
        assert!(matches!(settle(&s.desk, "mt-u"), Outcome::Unknown { .. }));
        assert!(
            s.desk
                .submit(ticket("mt-u"))
                .unwrap_err()
                .contains("check again")
        );
        s.desk.check_again("mt-u");
        assert!(matches!(settle(&s.desk, "mt-u"), Outcome::Accepted(_)));
        assert_eq!((s.broker.order_count(), s.broker.posts_made()), (1, 3));

        // Bad tickets and ids never leave.
        let mut bad = ticket("mt-bad");
        bad.limit_price = None;
        assert!(s.desk.submit(bad).is_err());
        assert!(s.desk.submit(ticket("mt bad/../x")).is_err());
        assert_eq!(s.broker.posts_made(), 3);
    }

    #[test]
    fn cancels_replaces_and_the_kill_switch() {
        let s = setup();
        s.desk.submit(ticket("mt-1")).unwrap();
        let Outcome::Accepted(first) = settle(&s.desk, "mt-1") else {
            panic!()
        };
        s.desk
            .replace(
                &first.id,
                Replacement {
                    client_order_id: "mt-1r".into(),
                    limit_price: Some(d("82.40")),
                    ..Replacement::default()
                },
            )
            .unwrap();
        let Outcome::Accepted(second) = settle(&s.desk, "mt-1r") else {
            panic!("{:?}", s.desk.outcome("mt-1r"))
        };
        assert_eq!(second.replaces.as_deref(), Some(first.id.as_str()));
        assert_eq!(second.limit_price, Some(d("82.40")));
        assert!(
            s.desk
                .replace(
                    &second.id,
                    Replacement {
                        client_order_id: "mt-x".into(),
                        ..Replacement::default()
                    }
                )
                .is_err()
        );

        s.desk.cancel(&second.id);
        assert_eq!(
            settle_action(&s.desk, &OrderDesk::cancel_key(&second.id)),
            ActionState::Done("Cancel requested.".into())
        );
        s.desk.cancel(&second.id);
        assert!(matches!(
            settle_action(&s.desk, &OrderDesk::cancel_key(&second.id)),
            ActionState::Failed(m) if m.contains("not cancelable")
        ));

        s.desk.submit(ticket("mt-2")).unwrap();
        settle(&s.desk, "mt-2");
        s.desk.kill(true);
        let ActionState::Done(said) = settle_action(&s.desk, KILL) else {
            panic!("{:?}", s.desk.action(KILL));
        };
        assert_eq!(
            said,
            "Cancelled 1 open order; closing 2 positions at the market (1 refused)."
        );
        assert!(
            s.broker
                .calls
                .lock()
                .iter()
                .any(|c| c == "DELETE /v2/positions?cancel_orders=true")
        );
    }

    #[test]
    fn a_replay_sends_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let ctx = FetchCtx::new(
            Arc::new(mt_data::FixtureTransport::new("unused")),
            None,
            FetchCtxOptions::default(),
            EventLog::default(),
        );
        let desk = OrderDesk::new(
            &Alpaca::default(),
            ctx,
            rt.handle().clone(),
            AuditLog::in_memory(),
        );
        assert!(
            desk.submit(ticket("mt-off"))
                .unwrap_err()
                .contains("Offline replay")
        );
        desk.kill(false);
        assert!(matches!(desk.action(KILL), Some(ActionState::Failed(_))));
        let a = new_client_order_id();
        assert!(a.starts_with("mt-") && is_safe_id(&a) && a != new_client_order_id());
    }
}
