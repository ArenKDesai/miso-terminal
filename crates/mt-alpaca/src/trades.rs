//! The account's order events as they happen: `wss://paper-api.alpaca.markets/stream`.
//!
//! Unlike the market-data streams, this one sends single JSON objects tagged
//! by `stream`, in binary frames. After the login (`authorization`), a
//! `listen` message chooses the streams; `trade_updates` then carries every
//! order's progress (`new`, `partial_fill`, `fill`, `canceled`…), with the
//! whole order as it stands after the event. Nothing here places orders (the
//! [`crate::OrderDesk`] does): the events keep the blotter's orders current
//! between re-reads of the order list, and make the app re-read the account,
//! positions, orders and activities at once instead of at the next minute.

use std::collections::BTreeMap;

use mt_core::account::OrderEvent;
use mt_core::order::Order;
use mt_data::{Applied, FetchCtx, FetchError, Frame, Request, Stream};
use serde_json::Value;

use crate::{AccountMode, KEY_ID, SECRET_KEY, account};

/// The one topic: order events.
pub const TRADE_UPDATES: &str = "trade_updates";
/// Order events a stream keeps.
const EVENTS_KEEP: usize = 200;
/// Orders a stream keeps (the newest by last change).
const ORDERS_KEEP: usize = 500;

/// Order events since the stream first connected, newest first.
#[derive(Clone, Debug, Default)]
pub struct LiveTrades {
    pub events: Vec<OrderEvent>,
    /// Logins accepted: more than one means it reconnected.
    pub sessions: u64,
    /// Order events received (fills and everything else).
    pub received: u64,
    /// Whether the server confirmed it is sending order events.
    pub listening: bool,
    pub notice: Option<String>,
    /// Each order as its latest event left it, by order id.
    pub orders: BTreeMap<String, Order>,
}

impl LiveTrades {
    /// Changes whenever the account should be re-read: a new login (after a
    /// reconnect, events may have been missed) or a new order event.
    pub fn sync_token(&self) -> (u64, u64) {
        (self.sessions, self.received)
    }
}

/// `wss://paper-api.alpaca.markets/stream` (live: `api.alpaca.markets`).
#[derive(Clone, Debug)]
pub struct TradeStream {
    url: String,
    mode: AccountMode,
}

impl TradeStream {
    pub(crate) fn new(url: String, mode: AccountMode) -> Self {
        Self { url, mode }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

fn listen(streams: &[&str]) -> String {
    serde_json::json!({"action": "listen", "data": {"streams": streams}}).to_string()
}

impl Stream for TradeStream {
    type State = LiveTrades;

    fn key(&self) -> String {
        format!("alpaca/stream/trade-updates/{}", self.mode.key())
    }

    fn label(&self) -> String {
        format!("Alpaca {} account (order events)", self.mode.name())
    }

    fn request(&self, _ctx: &FetchCtx) -> Result<Request, FetchError> {
        Ok(Request::get(&self.url))
    }

    fn hello(&self, ctx: &FetchCtx) -> Result<Vec<String>, FetchError> {
        if !ctx.is_live() {
            return Ok(Vec::new());
        }
        let (key, secret) = (ctx.secret(KEY_ID)?, ctx.secret(SECRET_KEY)?);
        Ok(vec![
            serde_json::json!({"action": "auth", "key": key.expose(), "secret": secret.expose()})
                .to_string(),
        ])
    }

    fn waits_for_ready(&self) -> bool {
        true
    }

    fn subscribe(&self, topics: &[String]) -> Vec<String> {
        if topics.iter().any(|t| t == TRADE_UPDATES) {
            vec![listen(&[TRADE_UPDATES])]
        } else {
            Vec::new()
        }
    }

    fn unsubscribe(&self, topics: &[String]) -> Vec<String> {
        if topics.iter().any(|t| t == TRADE_UPDATES) {
            vec![listen(&[])]
        } else {
            Vec::new()
        }
    }

    fn apply(&self, state: &mut LiveTrades, frame: &Frame) -> Result<Applied, FetchError> {
        let text = match frame {
            Frame::Text(t) => t.as_str(),
            Frame::Binary(b) => {
                std::str::from_utf8(b).map_err(|e| FetchError::parse("Alpaca order events", e))?
            }
            Frame::Pong => return Ok(Applied::Ignored),
        };
        let v: Value =
            serde_json::from_str(text).map_err(|e| FetchError::parse("Alpaca order events", e))?;
        let data = v.get("data").unwrap_or(&Value::Null);
        match v.get("stream").and_then(Value::as_str) {
            Some("authorization") => match data.get("status").and_then(Value::as_str) {
                Some("authorized") => {
                    state.sessions += 1;
                    state.notice = None;
                    Ok(Applied::Ready)
                }
                other => Err(FetchError::Auth(format!(
                    "Alpaca {} account stream: login {}",
                    self.mode.name(),
                    other.unwrap_or("refused")
                ))),
            },
            Some("listening") => {
                state.listening = data
                    .get("streams")
                    .and_then(Value::as_array)
                    .is_some_and(|s| s.iter().any(|x| x.as_str() == Some(TRADE_UPDATES)));
                Ok(Applied::Changed)
            }
            Some(TRADE_UPDATES) => {
                let Some(event) = account::order_event(data) else {
                    return Err(FetchError::parse(
                        "Alpaca order events",
                        "an order event without an order",
                    ));
                };
                state.received += 1;
                state.events.insert(0, event);
                state.events.truncate(EVENTS_KEEP);
                if let Some(order) = data.get("order").and_then(crate::orders::parse_order) {
                    keep_newer(&mut state.orders, order);
                }
                Ok(Applied::Changed)
            }
            _ => {
                // `{"stream":"error",…}` and anything new: keep what it says.
                if let Some(msg) = data
                    .get("error_message")
                    .or_else(|| data.get("message"))
                    .and_then(Value::as_str)
                {
                    state.notice = Some(format!("Alpaca account stream: {msg}"));
                    return Ok(Applied::Changed);
                }
                Ok(Applied::Ignored)
            }
        }
    }

    fn on_connect(&self, state: &mut LiveTrades) {
        state.listening = false;
    }
}

/// Keep `order` unless a copy changed later is already there; drop the
/// oldest beyond [`ORDERS_KEEP`].
fn keep_newer(orders: &mut BTreeMap<String, Order>, order: Order) {
    match orders.get(&order.id) {
        Some(old) if old.last_change() > order.last_change() => return,
        _ => {}
    }
    orders.insert(order.id.clone(), order);
    while orders.len() > ORDERS_KEEP {
        let Some(oldest) = orders
            .values()
            .min_by_key(|o| o.last_change())
            .map(|o| o.id.clone())
        else {
            break;
        };
        orders.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_listen_and_order_events() {
        let s = TradeStream::new(
            "wss://paper-api.alpaca.markets/stream".into(),
            AccountMode::Paper,
        );
        assert_eq!(s.key(), "alpaca/stream/trade-updates/paper");
        let sub: Value =
            serde_json::from_str(&s.subscribe(&[TRADE_UPDATES.to_owned()])[0]).unwrap();
        assert_eq!(sub["data"]["streams"], serde_json::json!(["trade_updates"]));
        assert!(s.subscribe(&["other".to_owned()]).is_empty());
        let mut st = LiveTrades::default();
        // The paper stream sends binary frames.
        let auth = Frame::Binary(
            r#"{"stream":"authorization","data":{"status":"authorized","action":"authenticate"}}"#
                .as_bytes()
                .to_vec()
                .into(),
        );
        assert_eq!(s.apply(&mut st, &auth), Ok(Applied::Ready));
        assert_eq!(st.sync_token(), (1, 0));
        let listening =
            Frame::Text(r#"{"stream":"listening","data":{"streams":["trade_updates"]}}"#.into());
        assert_eq!(s.apply(&mut st, &listening), Ok(Applied::Changed));
        assert!(st.listening);
        let fill = Frame::Text(
            r#"{"stream":"trade_updates","data":{"event":"fill","timestamp":"2026-10-02T18:31:00Z","price":"70.05",
                "qty":"60","position_qty":"60","order":{"id":"o1","symbol":"CEG","side":"buy","type":"market","qty":"60",
                "filled_qty":"60","filled_avg_price":"70.05","status":"filled"}}}"#
                .into(),
        );
        assert_eq!(s.apply(&mut st, &fill), Ok(Applied::Changed));
        assert_eq!(st.sync_token(), (1, 1));
        assert!(st.events[0].is_fill());
        assert_eq!(
            st.orders["o1"].status,
            mt_core::order::OrderStatus::Filled,
            "the order as the event left it"
        );
        s.on_connect(&mut st);
        assert!(!st.listening);
        assert_eq!(st.events.len(), 1, "events survive a reconnect");
        let refused = Frame::Text(
            r#"{"stream":"authorization","data":{"status":"unauthorized","action":"authenticate"}}"#
                .into(),
        );
        assert!(matches!(
            s.apply(&mut st, &refused),
            Err(FetchError::Auth(_))
        ));
        assert!(matches!(
            s.apply(&mut st, &Frame::Text("nope".into())),
            Err(FetchError::Parse { .. })
        ));
    }
}
