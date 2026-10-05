//! What the trading functions share (the BUY and SELL tickets, ORD): the
//! account's orders, merged from the order list and the order stream; the
//! guardrails' verdicts and the order desk's outcomes, drawn the same way
//! everywhere; and today's date in New York.
//!
//! Orders are sent only by `mt_alpaca::OrderDesk`, and only when a button in
//! a ticket or ORD is clicked. Commands (typed, `--run`, forwarded by another
//! launch, hotkeys) open tickets; none of them sends anything.

use chrono::NaiveDate;
use egui::{RichText, Ui};
use mt_alpaca::{LiveTrades, Outcome, TRADE_UPDATES};
use mt_core::guard::{Check, Level};
use mt_core::order::{Order, merge_orders};
use mt_data::Snapshot;

use crate::context::PanelCx;
use crate::skin::Skin;

/// The order list and the stream's newer copies of its orders.
pub struct OrderBook {
    pub list: Snapshot<Vec<Order>>,
    pub trades: Snapshot<LiveTrades>,
    /// Newest first, each order as it last changed.
    pub orders: Vec<Order>,
}

/// Watch the account's orders (call every frame).
pub fn watch_orders(cx: &PanelCx<'_>) -> OrderBook {
    let list = cx.hub.watch(&cx.alpaca.orders());
    let trades = cx
        .hub
        .watch_stream(&cx.alpaca.trade_stream(), &[TRADE_UPDATES]);
    let listed = list.data().map_or(&[][..], Vec::as_slice);
    let orders = merge_orders(
        listed,
        trades.data().into_iter().flat_map(|t| t.orders.values()),
    );
    OrderBook {
        list,
        trades,
        orders,
    }
}

impl OrderBook {
    pub fn get(&self, id: &str) -> Option<&Order> {
        self.orders.iter().find(|o| o.id == id)
    }

    pub fn open(&self) -> impl Iterator<Item = &Order> {
        self.orders.iter().filter(|o| o.status.is_open())
    }
}

/// Today in New York, the exchange's day.
pub fn today() -> NaiveDate {
    mt_core::exchange::now_exchange().date_naive()
}

/// `✓`, `⚠` or `✗` in the matching colour.
pub fn level_mark(skin: &Skin, level: Level) -> RichText {
    match level {
        Level::Pass => RichText::new("✓").color(skin.positive),
        Level::Warn => RichText::new("⚠").color(skin.warning),
        Level::Block => RichText::new("✕").color(skin.negative),
    }
    .strong()
}

/// One guardrail's verdict on a line: blocks and warnings stand out, passes
/// are quiet.
pub fn check_line(ui: &mut Ui, skin: &Skin, c: &Check) {
    ui.horizontal_wrapped(|ui| {
        ui.label(level_mark(skin, c.level));
        let color = match c.level {
            Level::Pass => skin.text,
            Level::Warn => skin.warning,
            Level::Block => skin.negative,
        };
        ui.label(RichText::new(&c.message).color(color));
    });
}

/// Blocks first, then warnings, then passes, each in rule order.
pub fn ordered(checks: &[Check]) -> Vec<&Check> {
    let mut out: Vec<&Check> = checks.iter().collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.level));
    out
}

/// What became of an order the desk was asked to send, in words.
pub fn outcome_line(ui: &mut Ui, skin: &Skin, outcome: &Outcome) {
    ui.horizontal_wrapped(|ui| match outcome {
        Outcome::Sending => {
            ui.spinner();
            ui.label(RichText::new("Sending to Alpaca…").color(skin.text_strong));
        }
        Outcome::Checking { reason } => {
            ui.spinner();
            ui.label(
                RichText::new(format!("{reason}. Asking Alpaca whether it arrived…"))
                    .color(skin.warning),
            );
        }
        Outcome::Accepted(o) => {
            ui.label(RichText::new("✓").strong().color(skin.positive));
            ui.label(
                RichText::new(format!("Placed: {}", o.status.label())).color(skin.text_strong),
            );
        }
        Outcome::Rejected { message, .. } => {
            ui.label(RichText::new("✕").strong().color(skin.negative));
            ui.label(
                RichText::new(format!("Alpaca refused it: {message}. Nothing was placed."))
                    .color(skin.negative),
            );
        }
        Outcome::NotPlaced { reason } => {
            ui.label(RichText::new("⚠").strong().color(skin.warning));
            ui.label(RichText::new(reason).color(skin.warning));
        }
        Outcome::Unknown { reason } => {
            ui.label(RichText::new("⚠").strong().color(skin.negative));
            ui.label(RichText::new(reason).color(skin.negative));
        }
        Outcome::NotSent(why) => {
            ui.label(RichText::new("✕").strong().color(skin.warning));
            ui.label(RichText::new(why).color(skin.warning));
        }
    });
}

/// `XEL, MGEE` (or `XEL US`) as tickers, upper case, without repeats.
pub fn parse_tickers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split([',', ';', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // `XEL US` is one security; `XEL WEC` two tickers.
        let tickers: Vec<String> = match mt_alpaca::normalize_symbol(part) {
            Some(t)
                if part.split_whitespace().count() <= 3
                    && crate::market::security_of(part).is_some() =>
            {
                vec![t]
            }
            _ => part
                .split_whitespace()
                .filter_map(mt_alpaca::normalize_symbol)
                .collect(),
        };
        for t in tickers {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickers_from_a_text_box() {
        assert_eq!(
            parse_tickers("xel, MGEE US;wec  aee\n"),
            ["XEL", "MGEE", "WEC", "AEE"]
        );
        assert_eq!(parse_tickers("XEL, "), ["XEL"]);
        assert!(parse_tickers(" , 1abc").is_empty());
    }
}
