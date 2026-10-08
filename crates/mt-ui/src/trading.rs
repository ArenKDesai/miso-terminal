//! What the trading functions share (the BUY and SELL tickets, ORD): the
//! account's orders, merged from the order list and the order stream; what
//! an option order is checked against ([`OptionMarket`]); the guardrails'
//! verdicts and the order desk's outcomes, drawn the same way everywhere;
//! and today's date in New York.
//!
//! Orders are sent only by `mt_alpaca::OrderDesk`, and only when a button in
//! a ticket or ORD is clicked. Commands (typed, `--run`, forwarded by another
//! launch, hotkeys) open tickets; none of them sends anything.

use chrono::{DateTime, NaiveDate, Utc};
use egui::{RichText, Ui};
use mt_alpaca::{LiveTrades, OptionChain, Outcome, TRADE_UPDATES};
use mt_core::account::{Account, Position, from_f64};
use mt_core::equity::OptionSnapshot;
use mt_core::exchange::Session;
use mt_core::guard::{Check, Level, Loaded, OptionContext};
use mt_core::instrument::OptionContract;
use mt_core::money::Decimal;
use mt_core::options::{ContractInfo, ContractList};
use mt_core::order::{Order, merge_orders};
use mt_data::Snapshot;

use crate::context::PanelCx;
use crate::market::{self, Board};
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

    /// Whether the order list has loaded (the stream alone is not the day's
    /// orders, so the daily cap cannot be counted from it).
    pub fn loaded(&self) -> bool {
        self.list.data().is_some()
    }
}

/// What the guardrails need loaded, given the position list.
pub fn loaded<T>(positions: &Snapshot<T>, book: &OrderBook) -> Loaded {
    Loaded {
        positions: positions.data().is_some(),
        orders: book.loaded(),
    }
}

/// What option orders are checked against: the account, its positions, the
/// contracts listed on the underlying, the chains of the expiries involved
/// and the underlying's price.
pub struct OptionMarket {
    pub underlying: String,
    pub account: Snapshot<Account>,
    pub positions: Snapshot<Vec<Position>>,
    pub contracts: Snapshot<ContractList>,
    /// One chain per expiry the order's contracts expire on.
    pub chains: Vec<Snapshot<OptionChain>>,
    pub stock: Board,
}

/// Watch what option orders on `contracts` (one underlying) need (call
/// every frame). `None` without any contract.
pub fn watch_option(cx: &PanelCx<'_>, contracts: &[OptionContract]) -> Option<OptionMarket> {
    let underlying = contracts.first()?.underlying.clone();
    let mut expiries: Vec<NaiveDate> = contracts.iter().map(|c| c.expiry).collect();
    expiries.sort();
    expiries.dedup();
    Some(OptionMarket {
        account: cx.hub.watch(&cx.alpaca.account()),
        positions: cx.hub.watch(&cx.alpaca.positions()),
        contracts: cx.hub.watch(&cx.alpaca.option_contracts(&underlying)),
        chains: expiries
            .into_iter()
            .map(|d| cx.hub.watch(&cx.alpaca.option_chain(&underlying, d)))
            .collect(),
        stock: market::board(cx, std::slice::from_ref(&underlying)),
        underlying,
    })
}

impl OptionMarket {
    pub fn snapshot(&self, symbol: &str) -> Option<&OptionSnapshot> {
        self.chains
            .iter()
            .find_map(|c| c.data().and_then(|c| c.get(symbol)))
    }

    pub fn info(&self, symbol: &str) -> Option<&ContractInfo> {
        self.contracts.data().and_then(|l| l.get(symbol))
    }

    /// When `symbol`'s quote is from.
    pub fn quoted_at(&self, symbol: &str) -> Option<DateTime<Utc>> {
        self.snapshot(symbol)
            .and_then(|s| s.latest_quote.as_ref())
            .map(|q| q.time)
    }

    /// Bid, ask and last as exact prices.
    pub fn quote(&self, symbol: &str) -> (Option<Decimal>, Option<Decimal>, Option<Decimal>) {
        let s = self.snapshot(symbol);
        let exact = |v: Option<f64>| v.and_then(|v| from_f64(v, 2));
        (
            exact(s.and_then(OptionSnapshot::bid)),
            exact(s.and_then(OptionSnapshot::ask)),
            exact(s.and_then(OptionSnapshot::last)),
        )
    }

    /// The mid of a two-sided quote, else the last trade.
    pub fn mark(&self, symbol: &str) -> Option<Decimal> {
        self.snapshot(symbol)
            .and_then(OptionSnapshot::mark)
            .and_then(|v| from_f64(v, 4))
    }

    /// The underlying's last price.
    pub fn spot(&self) -> Option<f64> {
        self.stock.row(&self.underlying).last
    }

    pub fn positions(&self) -> &[Position] {
        self.positions.data().map_or(&[][..], Vec::as_slice)
    }

    /// Contracts held of `symbol`, signed.
    pub fn held(&self, symbol: &str) -> Decimal {
        self.positions()
            .iter()
            .filter(|p| p.symbol == symbol)
            .map(|p| p.qty)
            .sum()
    }

    /// The guardrails' context for an order on `symbol`.
    pub fn context(
        &self,
        symbol: &str,
        book: &OrderBook,
        session: Session,
        today_value: Decimal,
        day_trade: bool,
    ) -> OptionContext<'_> {
        let (bid, ask, last) = self.quote(symbol);
        OptionContext {
            account: self.account.data(),
            loaded: loaded(&self.positions, book),
            positions: self.positions(),
            info: self.info(symbol),
            bid,
            ask,
            last,
            priced_at: self.snapshot(symbol).and_then(OptionSnapshot::priced_at),
            session,
            now: mt_core::time::now_utc(),
            today_value,
            day_trade,
        }
    }

    /// The first failure among the feeds, for a placeholder.
    pub fn error(&self) -> Option<String> {
        self.contracts
            .error
            .as_ref()
            .or_else(|| self.chains.iter().find_map(|c| c.error.as_ref()))
            .map(ToString::to_string)
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
