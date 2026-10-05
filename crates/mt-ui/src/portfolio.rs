//! What the account functions share (PORT, ACCT, PNL, ACT, HOME's tile and
//! the status bar's band): the account and its positions marked with the
//! quote stream's newer prices, the order-event stream, the early re-sync
//! after order events and reconnects, and money formats.
//!
//! Alpaca's figures are the truth: balances and positions are re-read every
//! minute (and at once after an order event). Between re-reads a stock
//! position moves with trades newer than the re-read, by the change in price
//! times the quantity, so P&L keeps Alpaca's own reference for the day.

use egui::{Color32, RichText, Ui};
use mt_alpaca::{AccountMode, Alpaca, HistoryPeriod, LiveTrades, TRADE_UPDATES};
use mt_core::account::{Account, Marked, Position, from_f64, to_f64};
use mt_core::money::{Decimal, fmt_qty, fmt_usd};
use mt_data::{DataHub, Snapshot};

use crate::context::PanelCx;
use crate::market::{self, Board};
use crate::skin::Skin;

/// The account, its positions, live prices for the stocks among them, and
/// the order-event stream.
pub struct Book {
    pub account: Snapshot<Account>,
    pub positions: Snapshot<Vec<Position>>,
    pub trades: Snapshot<LiveTrades>,
    /// Live prices for the stock positions (none without any).
    pub board: Option<Board>,
}

/// One position with its marked value.
pub struct Line {
    pub position: Position,
    pub mark: Marked,
}

/// The account's headline figures, marked live.
#[derive(Clone, Debug, PartialEq)]
pub struct Totals {
    pub equity: Decimal,
    pub day_pl: Decimal,
    pub day_pct: Option<f64>,
    pub unrealized: Decimal,
    pub cost: Decimal,
    pub long_value: Decimal,
    pub short_value: Decimal,
    /// Positions whose price is newer than Alpaca's.
    pub live: usize,
}

/// Watch everything a portfolio view needs (call every frame).
pub fn watch(cx: &PanelCx<'_>) -> Book {
    let account = cx.hub.watch(&cx.alpaca.account());
    let positions = cx.hub.watch(&cx.alpaca.positions());
    let trades = cx
        .hub
        .watch_stream(&cx.alpaca.trade_stream(), &[TRADE_UPDATES]);
    let stocks: Vec<String> = positions
        .data()
        .map(|list| {
            list.iter()
                .filter(|p| !p.is_option())
                .map(|p| p.symbol.clone())
                .collect()
        })
        .unwrap_or_default();
    let board = (!stocks.is_empty()).then(|| market::board(cx, &stocks));
    Book {
        account,
        positions,
        trades,
        board,
    }
}

impl Book {
    /// The stream's price for a stock position, when it traded after Alpaca
    /// last valued the positions.
    fn live_price(&self, p: &Position) -> Option<Decimal> {
        if p.is_option() {
            return None;
        }
        let valued = self.positions.updated?;
        let row = self.board.as_ref()?.row(&p.symbol);
        let (last, at) = (row.last?, row.last_time?);
        if at <= valued {
            return None;
        }
        from_f64(last, 4)
    }

    pub fn lines(&self) -> Vec<Line> {
        self.positions
            .data()
            .map(|list| {
                list.iter()
                    .map(|p| Line {
                        mark: p.mark(self.live_price(p)),
                        position: p.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Equity and the day's P&L moved by the live marks.
    pub fn totals(&self, lines: &[Line]) -> Option<Totals> {
        let a = self.account.data()?;
        let delta: Decimal = lines.iter().map(|l| l.mark.delta).sum();
        let (mut long_value, mut short_value) = (Decimal::ZERO, Decimal::ZERO);
        for l in lines {
            if l.mark.market_value.is_sign_negative() {
                short_value += l.mark.market_value;
            } else {
                long_value += l.mark.market_value;
            }
        }
        let day_pl = a.day_pl() + delta;
        Some(Totals {
            equity: a.equity + delta,
            day_pct: (!a.last_equity.is_zero()).then(|| to_f64(day_pl) / to_f64(a.last_equity)),
            day_pl,
            unrealized: lines.iter().filter_map(|l| l.mark.unrealized_pl).sum(),
            cost: lines.iter().map(|l| l.position.cost_basis.abs()).sum(),
            long_value: if lines.is_empty() {
                a.long_market_value
            } else {
                long_value
            },
            short_value: if lines.is_empty() {
                a.short_market_value
            } else {
                short_value
            },
            live: lines.iter().filter(|l| l.mark.live).count(),
        })
    }

    /// The stream's complaint or failure, if any.
    pub fn stream_notice(&self) -> Option<String> {
        self.trades
            .data()
            .and_then(|t| t.notice.clone())
            .or_else(|| self.trades.error.as_ref().map(ToString::to_string))
    }
}

/// Re-read the account at once after an order event or a reconnect, rather
/// than at the next minute. Call every frame (the app does); `seen` is the
/// stream's last token.
pub fn resync(hub: &DataHub, alpaca: &Alpaca, seen: &mut Option<(u64, u64)>) {
    let snap = hub.peek_stream(&alpaca.trade_stream());
    let Some(token) = snap.data().map(LiveTrades::sync_token) else {
        return;
    };
    match *seen {
        Some(s) if s == token => {}
        Some(_) => {
            // Only what something has asked for (refresh_key skips the rest).
            for key in [
                mt_data::Query::key(&alpaca.account()),
                mt_data::Query::key(&alpaca.positions()),
                mt_data::Query::key(&alpaca.activities()),
                mt_data::Query::key(&alpaca.portfolio_history(HistoryPeriod::Day)),
            ] {
                hub.refresh_key(&key);
            }
            *seen = Some(token);
        }
        None => *seen = Some(token),
    }
}

// ------------------------------------------------------------------ formats

/// `$1,234.50`.
pub fn usd(v: Decimal) -> String {
    fmt_usd(v, 2)
}

/// `+$1,234.50`, `-$3.00`, `$0.00`.
pub fn usd_signed(v: Decimal) -> String {
    let s = fmt_usd(v, 2);
    if v.round_dp(2) > Decimal::ZERO {
        format!("+{s}")
    } else {
        s
    }
}

pub fn usd_opt(v: Option<Decimal>) -> String {
    v.map_or_else(|| market::fmt::DASH.to_owned(), usd)
}

/// A fraction as `+1.23%`.
pub fn pct_signed(v: Option<f64>) -> String {
    v.filter(|v| v.is_finite()).map_or_else(
        || market::fmt::DASH.to_owned(),
        |v| format!("{:+.2}%", v * 100.0),
    )
}

/// An exact price: at least two decimals and up to four, as given
/// (`34.70`, `0.88`, `0.1925`).
pub fn price(v: Decimal) -> String {
    let r = v.round_dp(4).normalize();
    if r.scale() < 2 {
        format!("{r:.2}")
    } else {
        r.to_string()
    }
}

/// An option's strike as written on the contract: `35`, `82.5`.
pub fn strike(v: Decimal) -> String {
    v.normalize().to_string()
}

/// `XLU 45 call · Dec 18 '26`, for an OCC symbol.
pub fn contract_name(c: &mt_core::instrument::OptionContract) -> String {
    let right = match c.right {
        mt_core::instrument::OptionRight::Call => "call",
        mt_core::instrument::OptionRight::Put => "put",
    };
    format!(
        "{} {} {right} · {}",
        c.underlying,
        strike(c.strike),
        c.expiry.format("%b %d '%y")
    )
}

pub fn qty(v: Decimal) -> String {
    fmt_qty(v)
}

/// Colour for a signed amount.
pub fn delta_color(skin: &Skin, v: Option<Decimal>) -> Color32 {
    v.map_or(skin.text_muted, |v| skin.delta(to_f64(v)))
}

/// `+$504.30 (+0.49%)`, coloured.
pub fn pl_text(skin: &Skin, v: Decimal, pct: Option<f64>) -> RichText {
    let text = match pct {
        Some(_) => format!("{} ({})", usd_signed(v), pct_signed(pct)),
        None => usd_signed(v),
    };
    RichText::new(text).color(skin.delta(to_f64(v)))
}

// --------------------------------------------------------------------- band

/// Text that reads on `fill`: whichever of `a` and `b` contrasts more.
fn on(fill: Color32, a: Color32, b: Color32) -> Color32 {
    let lum = |c: Color32| {
        let ch = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    };
    let contrast = |x: Color32| {
        let (l1, l2) = (lum(fill), lum(x));
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    };
    if contrast(a) >= contrast(b) { a } else { b }
}

/// The band's fill: calm for paper, unmistakable red for live money.
pub fn band_fill(skin: &Skin, mode: AccountMode) -> Color32 {
    match mode {
        AccountMode::Paper => skin.info,
        AccountMode::Live => skin.negative,
    }
}

/// The strip across the bottom of the window whenever an Alpaca account is
/// connected (draw it in a frame filled with [`band_fill`]): which kind it
/// is, and whether it answers. Never a balance, so a screenshot of the
/// terminal never shows what an account holds. Returns whether it was clicked.
pub fn band(ui: &mut Ui, skin: &Skin, mode: AccountMode, account: &Snapshot<Account>) -> bool {
    let ink = on(band_fill(skin, mode), skin.background, skin.text_strong);
    let mut clicked = false;
    ui.horizontal(|ui| {
        let tag = RichText::new(mode.band()).strong().monospace().color(ink);
        let resp = ui
            .add(egui::Label::new(tag).sense(egui::Sense::click()))
            .on_hover_text("Open ACCT")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        clicked |= resp.clicked();
        let what = match mode {
            AccountMode::Paper => "Alpaca paper account · simulated money",
            AccountMode::Live => "Alpaca LIVE account · real money",
        };
        ui.label(RichText::new(what).small().color(ink));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let state = match (&account.error, account.data()) {
                (Some(mt_data::FetchError::Auth(_)), _) => "keys refused: check SET".to_owned(),
                (Some(_), Some(_)) => "connection lost · retrying".to_owned(),
                (Some(_), None) => "cannot reach the account · retrying".to_owned(),
                (None, Some(a)) if !a.restrictions().is_empty() => {
                    format!("connected · {}", a.restrictions().join(", "))
                }
                (None, Some(_)) => "connected".to_owned(),
                (None, None) => "connecting…".to_owned(),
            };
            ui.label(RichText::new(state).small().color(ink));
        });
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn money_formats() {
        let d = |s: &str| Decimal::from_str(s).unwrap();
        assert_eq!(usd_signed(d("504.3")), "+$504.30");
        assert_eq!(usd_signed(d("-3")), "-$3.00");
        assert_eq!(usd_signed(d("0.001")), "$0.00");
        assert_eq!(pct_signed(Some(0.004_87)), "+0.49%");
        assert_eq!(pct_signed(Some(f64::NAN)), market::fmt::DASH);
        assert_eq!(qty(d("-50")), "-50");
        assert_eq!(price(d("0.88")), "0.88");
        assert_eq!(price(d("34.7")), "34.70");
        assert_eq!(price(d("0.19250")), "0.1925");
        assert_eq!(price(d("2")), "2.00");
        assert_eq!(strike(d("35.000")), "35");
        let c = mt_core::instrument::OptionContract::parse_occ("VST261120P00035000").unwrap();
        assert_eq!(contract_name(&c), "VST 35 put · Nov 20 '26");
    }

    #[test]
    fn band_text_contrasts() {
        let dark = Color32::from_rgb(10, 10, 10);
        let light = Color32::from_rgb(245, 245, 245);
        assert_eq!(on(Color32::from_rgb(90, 160, 230), dark, light), dark);
        assert_eq!(on(Color32::from_rgb(160, 20, 20), dark, light), light);
    }
}
