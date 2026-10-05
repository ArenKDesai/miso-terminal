//! Orders on the paper account: parsing Alpaca's order objects (a
//! multi-leg order, class `mleg`, has an empty symbol and side and its legs
//! nested), the order list behind the ORD blotter (`GET /v2/orders`, legs
//! rolled up under their order), and the JSON bodies the order desk sends
//! ([`crate::desk`]). Placing, cancelling and replacing are
//! the desk's alone: nothing here sends anything but GETs.

use std::sync::Arc;
use std::time::Duration;

use mt_core::account::{AssetClass, OrderSide};
use mt_core::options::PositionIntent;
use mt_core::order::{Order, OrderRequest, OrderStatus, OrderType, TimeInForce};
use mt_data::{FetchCtx, FetchError, Freshness, Query};
use serde_json::{Map, Value, json};

use crate::account::decimal;
use crate::{Alpaca, parse, request};

/// Orders the blotter lists: the newest this many, open or not.
pub const ORDERS_KEEP: usize = 500;

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn time(v: &Value, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(parse::parse_time)
}

/// One order object, from the REST API or a `trade_updates` event.
pub fn parse_order(v: &Value) -> Option<Order> {
    let type_name = text(v, "type")
        .or_else(|| text(v, "order_type"))
        .unwrap_or_default();
    let order_class = text(v, "order_class").unwrap_or_default();
    // The strategy itself, not one of its legs (which may carry the class too).
    let multi = order_class.eq_ignore_ascii_case("mleg") && text(v, "symbol").is_none();
    let legs: Vec<Order> = v
        .get("legs")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(parse_order).collect())
        .unwrap_or_default();
    let limit_price = v.get("limit_price").and_then(decimal);
    // A multi-leg order names neither a symbol nor a side: its legs do. Its
    // side here is the net's: a buy for a debit, a sell for a credit.
    let symbol = match text(v, "symbol") {
        Some(s) => s.to_ascii_uppercase(),
        None if multi => String::new(),
        None => return None,
    };
    let side = match text(v, "side").and_then(|s| OrderSide::parse(&s)) {
        Some(s) => s,
        None if multi => match limit_price {
            Some(p) if p.is_sign_negative() => OrderSide::Sell,
            Some(_) => OrderSide::Buy,
            None => legs.first().map_or(OrderSide::Buy, |l| l.side),
        },
        None => return None,
    };
    Some(Order {
        id: text(v, "id")?,
        client_order_id: text(v, "client_order_id").unwrap_or_default(),
        symbol,
        class: if multi {
            AssetClass::Option
        } else {
            AssetClass::parse(&text(v, "asset_class").unwrap_or_default())
        },
        side,
        order_type: OrderType::parse(&type_name),
        tif: text(v, "time_in_force").and_then(|t| TimeInForce::parse(&t)),
        type_name,
        qty: v.get("qty").and_then(decimal),
        notional: v.get("notional").and_then(decimal),
        filled_qty: v.get("filled_qty").and_then(decimal).unwrap_or_default(),
        filled_avg_price: v.get("filled_avg_price").and_then(decimal),
        limit_price,
        stop_price: v.get("stop_price").and_then(decimal),
        status: OrderStatus::parse(&text(v, "status").unwrap_or_default()),
        extended_hours: v
            .get("extended_hours")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        created_at: time(v, "created_at"),
        submitted_at: time(v, "submitted_at"),
        updated_at: time(v, "updated_at"),
        filled_at: time(v, "filled_at"),
        canceled_at: time(v, "canceled_at"),
        expired_at: time(v, "expired_at"),
        replaced_by: text(v, "replaced_by"),
        replaces: text(v, "replaces"),
        position_intent: text(v, "position_intent").and_then(|i| PositionIntent::parse(&i)),
        ratio_qty: v.get("ratio_qty").and_then(decimal),
        order_class,
        legs,
    })
}

/// `GET /v2/orders`: newest first.
pub fn parse_orders(body: &[u8]) -> Result<Vec<Order>, FetchError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| FetchError::parse("Alpaca orders", e))?;
    let list = v
        .as_array()
        .ok_or_else(|| FetchError::parse("Alpaca orders", "not a list"))?;
    Ok(mt_core::order::merge_orders(
        &list.iter().filter_map(parse_order).collect::<Vec<_>>(),
        [],
    ))
}

/// Alpaca's message in an error body (`{"code":40310000,"message":"insufficient buying power"}`),
/// or the start of the body.
pub fn error_message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| text(&v, "message"))
        .unwrap_or_else(|| {
            let s = String::from_utf8_lossy(body);
            let s = s.trim();
            s.chars().take(200).collect()
        })
}

fn amount(d: mt_core::money::Decimal) -> Value {
    Value::String(d.normalize().to_string())
}

/// `POST /v2/orders`. Amounts go as strings, exactly.
pub fn order_body(req: &OrderRequest) -> Value {
    if req.is_multi_leg() {
        return spread_body(req);
    }
    let mut m = Map::new();
    m.insert(
        "symbol".into(),
        json!(req.symbol.trim().to_ascii_uppercase()),
    );
    m.insert("qty".into(), amount(req.qty));
    m.insert(
        "side".into(),
        json!(match req.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }),
    );
    m.insert("type".into(), json!(req.order_type.as_str()));
    m.insert("time_in_force".into(), json!(req.tif.as_str()));
    if req.order_type.needs_limit()
        && let Some(p) = req.limit_price
    {
        m.insert("limit_price".into(), amount(p));
    }
    if req.order_type.needs_stop()
        && let Some(p) = req.stop_price
    {
        m.insert("stop_price".into(), amount(p));
    }
    if req.extended_hours {
        m.insert("extended_hours".into(), json!(true));
    }
    if let Some(i) = req.position_intent {
        m.insert("position_intent".into(), json!(i.as_str()));
    }
    m.insert("client_order_id".into(), json!(req.client_order_id));
    Value::Object(m)
}

/// `POST /v2/orders` for a multi-leg order (`order_class: mleg`): the units,
/// the net limit (positive a debit, negative a credit), and each leg's
/// contract, ratio, side and intent. No symbol or side of its own.
fn spread_body(req: &OrderRequest) -> Value {
    let legs: Vec<Value> = req
        .legs
        .iter()
        .map(|l| {
            let mut leg = Map::new();
            leg.insert("symbol".into(), json!(l.symbol.trim().to_ascii_uppercase()));
            leg.insert("ratio_qty".into(), json!(l.ratio.to_string()));
            leg.insert(
                "side".into(),
                json!(match l.side {
                    OrderSide::Buy => "buy",
                    OrderSide::Sell => "sell",
                }),
            );
            if let Some(i) = l.intent {
                leg.insert("position_intent".into(), json!(i.as_str()));
            }
            Value::Object(leg)
        })
        .collect();
    let mut m = Map::new();
    m.insert("order_class".into(), json!("mleg"));
    m.insert("qty".into(), amount(req.qty));
    m.insert("type".into(), json!(req.order_type.as_str()));
    m.insert("time_in_force".into(), json!(req.tif.as_str()));
    if req.order_type == OrderType::Limit
        && let Some(p) = req.limit_price
    {
        m.insert("limit_price".into(), amount(p));
    }
    m.insert("legs".into(), Value::Array(legs));
    m.insert("client_order_id".into(), json!(req.client_order_id));
    Value::Object(m)
}

/// What a replace changes (`PATCH /v2/orders/{id}`): any of the quantity,
/// limit, stop and time in force; the rest stays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Replacement {
    /// The replacement order's own id, made before it is sent, as for a new order.
    pub client_order_id: String,
    pub qty: Option<mt_core::money::Decimal>,
    pub limit_price: Option<mt_core::money::Decimal>,
    pub stop_price: Option<mt_core::money::Decimal>,
    pub tif: Option<TimeInForce>,
}

pub fn replace_body(r: &Replacement) -> Value {
    let mut m = Map::new();
    if let Some(q) = r.qty {
        m.insert("qty".into(), amount(q));
    }
    if let Some(p) = r.limit_price {
        m.insert("limit_price".into(), amount(p));
    }
    if let Some(p) = r.stop_price {
        m.insert("stop_price".into(), amount(p));
    }
    if let Some(t) = r.tif {
        m.insert("time_in_force".into(), json!(t.as_str()));
    }
    m.insert("client_order_id".into(), json!(r.client_order_id));
    Value::Object(m)
}

/// The account's orders, newest first: every open one and the latest
/// finished ones, up to [`ORDERS_KEEP`]. Re-read every minute, and at once
/// after an order event or a reconnect, so the blotter is reconciled with
/// the broker whenever the stream may have missed something.
#[derive(Clone, Debug)]
pub struct OrdersQuery {
    alpaca: Alpaca,
}

impl OrdersQuery {
    pub(crate) fn new(alpaca: Alpaca) -> Self {
        Self { alpaca }
    }

    pub fn url(&self) -> String {
        format!(
            "{}/v2/orders?status=all&limit={ORDERS_KEEP}&direction=desc&nested=true",
            self.alpaca.endpoints.trading
        )
    }
}

impl Query for OrdersQuery {
    type Output = Vec<Order>;

    fn key(&self) -> String {
        format!("alpaca/{}/orders", self.alpaca.mode().key())
    }

    fn label(&self) -> String {
        format!("Alpaca {} orders", self.alpaca.mode().name())
    }

    fn freshness(&self, _: &Vec<Order>) -> Freshness {
        let trading = mt_core::exchange::market_status(mt_core::time::now_utc(), &[]).session
            != mt_core::exchange::Session::Closed;
        Freshness::Every(Duration::from_secs(if trading { 60 } else { 300 }))
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _: Option<Arc<Vec<Order>>>,
    ) -> Result<Vec<Order>, FetchError> {
        parse_orders(&ctx.get(request(&ctx, self.url())?).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use mt_core::money::Decimal;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn orders_parse_exactly() {
        let list = parse_orders(
            br#"[{"id":"o1","client_order_id":"mt-1","created_at":"2026-10-02T18:30:58.12Z","updated_at":"2026-10-02T18:31:02Z",
                   "submitted_at":"2026-10-02T18:30:58Z","filled_at":"2026-10-02T18:31:02Z","symbol":"ceg","asset_class":"us_equity",
                   "qty":"60","filled_qty":"60","filled_avg_price":"71.48","order_type":"limit","type":"limit","side":"buy",
                   "time_in_force":"day","limit_price":"71.53","stop_price":null,"status":"filled","extended_hours":false,
                   "replaced_by":null,"replaces":null},
                  {"id":"o2","client_order_id":"mt-2","created_at":"2026-10-02T19:00:00Z","symbol":"XLU","qty":"100",
                   "filled_qty":"0","type":"stop_limit","side":"sell","time_in_force":"gtc","limit_price":"40","stop_price":"40.5",
                   "status":"new"},
                  {"id":"o3","symbol":"XEL","side":"sideways"}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "o2", "newest first");
        assert_eq!(list[0].order_type, Some(OrderType::StopLimit));
        assert_eq!(list[0].tif, Some(TimeInForce::Gtc));
        assert_eq!(list[0].remaining(), d("100"));
        let ceg = &list[1];
        assert_eq!(ceg.symbol, "CEG");
        assert_eq!(ceg.status, OrderStatus::Filled);
        assert_eq!(ceg.value(None), d("4288.80"));
        assert!(parse_orders(b"{}").is_err());
    }

    #[test]
    fn bodies_carry_exact_amounts_and_only_what_applies() {
        let req = OrderRequest {
            client_order_id: "mt-abc".into(),
            symbol: "xlu".into(),
            side: OrderSide::Sell,
            qty: d("10.50"),
            order_type: OrderType::Limit,
            limit_price: Some(d("82.50")),
            stop_price: Some(d("80")),
            tif: TimeInForce::Day,
            extended_hours: true,
            position_intent: None,
            legs: Vec::new(),
        };
        let b = order_body(&req);
        assert_eq!(b["symbol"], "XLU");
        assert_eq!(b["qty"], "10.5");
        assert_eq!(b["limit_price"], "82.5");
        assert!(b.get("stop_price").is_none(), "a limit order sends no stop");
        assert_eq!(b["extended_hours"], true);
        assert_eq!(b["client_order_id"], "mt-abc");
        let market = order_body(&OrderRequest {
            order_type: OrderType::Market,
            extended_hours: false,
            ..req.clone()
        });
        assert!(market.get("limit_price").is_none() && market.get("extended_hours").is_none());
        assert!(market.get("position_intent").is_none());
        // An option order names its contract and what it does to the position.
        let option = order_body(&OrderRequest {
            symbol: "XLU261218C00045000".into(),
            qty: d("2"),
            extended_hours: false,
            position_intent: Some(PositionIntent::SellToClose),
            ..req
        });
        assert_eq!(option["symbol"], "XLU261218C00045000");
        assert_eq!(option["qty"], "2");
        assert_eq!(option["position_intent"], "sell_to_close");
        let parsed = parse_order(&json!({"id": "o", "symbol": "XLU261218C00045000", "side": "sell",
            "asset_class": "us_option", "qty": "2", "position_intent": "sell_to_close", "status": "new"}))
        .unwrap();
        assert!(parsed.is_option());
        assert_eq!(parsed.position_intent, Some(PositionIntent::SellToClose));
    }

    #[test]
    fn spreads_go_as_mleg_orders_and_come_back_with_their_legs() {
        use mt_core::options::Leg;
        let req = OrderRequest {
            client_order_id: "mt-s".into(),
            symbol: String::new(),
            side: OrderSide::Sell,
            qty: d("2"),
            order_type: OrderType::Limit,
            limit_price: Some(d("-0.70")),
            stop_price: None,
            tif: TimeInForce::Day,
            extended_hours: false,
            position_intent: None,
            legs: vec![
                Leg {
                    intent: Some(PositionIntent::SellToOpen),
                    ..Leg::new("XLU261218C00045000", OrderSide::Sell, 1)
                },
                Leg {
                    intent: Some(PositionIntent::BuyToOpen),
                    ..Leg::new("XLU261218C00047000", OrderSide::Buy, 1)
                },
            ],
        };
        let b = order_body(&req);
        assert_eq!(
            b,
            json!({
                "order_class": "mleg", "qty": "2", "type": "limit", "time_in_force": "day",
                "limit_price": "-0.7", "client_order_id": "mt-s",
                "legs": [
                    {"symbol": "XLU261218C00045000", "ratio_qty": "1", "side": "sell", "position_intent": "sell_to_open"},
                    {"symbol": "XLU261218C00047000", "ratio_qty": "1", "side": "buy", "position_intent": "buy_to_open"},
                ],
            })
        );
        // Alpaca's answer: no symbol, side or asset class on the order itself.
        let o = parse_order(&json!({
            "id": "m1", "client_order_id": "mt-s", "symbol": "", "side": "", "asset_class": "",
            "order_class": "mleg", "qty": "2", "type": "limit", "limit_price": "-0.7",
            "time_in_force": "day", "status": "new", "filled_qty": "0",
            "legs": [
                {"id": "l1", "symbol": "XLU261218C00045000", "side": "sell", "ratio_qty": "1",
                 "position_intent": "sell_to_open", "asset_class": "us_option", "status": "new", "filled_qty": "0"},
                {"id": "l2", "symbol": "XLU261218C00047000", "side": "buy", "ratio_qty": "1",
                 "position_intent": "buy_to_open", "asset_class": "us_option", "status": "new", "filled_qty": "0"}
            ]
        }))
        .unwrap();
        assert!(o.is_multi_leg() && o.is_option());
        assert_eq!(o.side, OrderSide::Sell, "a credit");
        assert_eq!(o.legs.len(), 2);
        assert_eq!(o.strategy_legs()[0].ratio, 1);
        assert_eq!(o.title(), "Bear call spread on XLU");
        assert_eq!(
            o.value(None),
            d("-140.0"),
            "a credit's value is negative; day_value counts it whole"
        );
        let r = replace_body(&Replacement {
            client_order_id: "mt-r".into(),
            limit_price: Some(d("82.55")),
            ..Replacement::default()
        });
        assert_eq!(
            r,
            json!({"limit_price": "82.55", "client_order_id": "mt-r"})
        );
        assert_eq!(
            error_message(br#"{"code":40310000,"message":"insufficient buying power"}"#),
            "insufficient buying power"
        );
        assert_eq!(error_message(b"  bad gateway "), "bad gateway");
    }

    #[test]
    fn the_list_url_picks_up_open_and_finished_orders() {
        let q = Alpaca::default().orders();
        assert!(q.url().contains("/v2/orders?status=all&limit=500"));
        assert_eq!(q.key(), "alpaca/paper/orders");
    }
}
