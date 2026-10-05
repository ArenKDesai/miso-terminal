//! Shared pieces for the securities functions (Q, DES, CN, and GP and WL for
//! securities): the market's status, live rows for a set of securities,
//! number and time formats (New York time), a security picker, and the prompt
//! shown until Alpaca keys are stored.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use egui::{Color32, RichText, Ui};
use mt_alpaca::board::Row;
use mt_alpaca::{Alpaca, LiveMarket, Snapshots};
use mt_core::equity::{Asset, AssetList};
use mt_core::exchange::{self, MarketStatus, Session, TradingDay};
use mt_core::instrument::{Instrument, Security};
use mt_data::{DataHub, Snapshot};

use crate::context::PanelCx;
use crate::function::Route;
use crate::skin::Skin;
use crate::widgets;

/// Without keys, a securities panel says how to add them and draws nothing
/// else. Returns whether it did.
pub fn needs_keys(ui: &mut Ui, cx: &mut PanelCx<'_>) -> bool {
    if cx.alpaca.is_ready() {
        return false;
    }
    let skin = cx.skin;
    ui.add_space(10.0);
    ui.label(
        RichText::new(
            "Stock and ETF prices and the paper account come from Alpaca, which needs a free \
             account's API keys.",
        )
        .color(skin.text_strong),
    );
    ui.label(
        RichText::new(
            "Sign up at alpaca.markets (no deposit is needed for paper trading or market data), \
             create paper-trading API keys in its dashboard, and paste both into SET. They are \
             kept in Windows Credential Manager, never in files.",
        )
        .color(skin.text_muted),
    );
    ui.add_space(6.0);
    if ui.button("Open SET").clicked() {
        cx.open(Route::code("SET"));
    }
    true
}

/// The exchange calendar around today (empty until it loads).
pub fn calendar(hub: &DataHub, alpaca: &Alpaca) -> Arc<Vec<TradingDay>> {
    let today = exchange::now_exchange().date_naive();
    hub.watch(&alpaca.calendar(today)).data.unwrap_or_default()
}

/// Where the market is now: the calendar's hours, overruled by Alpaca's
/// clock when they disagree.
pub fn status(hub: &DataHub, alpaca: &Alpaca) -> MarketStatus {
    let now = mt_core::time::now_utc();
    let mut s = exchange::market_status(now, &calendar(hub, alpaca));
    if let Some(clock) = hub.watch(&alpaca.clock()).data() {
        s = s.with_clock(clock, now);
    }
    s
}

/// `Open · closes 16:00 EDT`, `Closed · opens Mon 09:30 EDT`.
pub fn status_text(s: &MarketStatus) -> String {
    let label = match s.session {
        Session::Closed => "Closed",
        Session::PreMarket => "Pre-market",
        Session::Regular => "Open",
        Session::AfterHours => "After hours",
    };
    match s.next {
        Some((at, what)) => format!("{label} · {what} {}", when(at)),
        None => label.to_owned(),
    }
}

/// A time in New York: `16:00 EDT` today, `Mon 09:30 EDT` this week, else with the date.
pub fn when(at: DateTime<Utc>) -> String {
    let local = exchange::to_exchange(at);
    let now = exchange::now_exchange();
    let days = (local.date_naive() - now.date_naive()).num_days();
    let tz = exchange::tz_label(&local);
    match days {
        0 => format!("{} {tz}", local.format("%H:%M")),
        1..=6 => format!("{} {tz}", local.format("%a %H:%M")),
        _ => format!("{} {tz}", local.format("%b %d %H:%M")),
    }
}

pub fn status_color(skin: &Skin, s: &MarketStatus) -> Color32 {
    match s.session {
        Session::Regular => skin.live,
        Session::PreMarket | Session::AfterHours => skin.warning,
        Session::Closed => skin.text_muted,
    }
}

/// `US market · Open · closes 16:00 EDT` with a lamp, for title bars.
pub fn status_label(ui: &mut Ui, cx: &PanelCx<'_>) {
    let s = status(cx.hub, cx.alpaca);
    ui.label(
        RichText::new(format!("US market · {}", status_text(&s)))
            .small()
            .color(cx.skin.text_muted),
    );
    widgets::lamp(ui, status_color(cx.skin, &s));
}

/// The latest session that has begun (pre-market counts) and the one before
/// it, by the calendar (or standard hours where it does not reach).
pub fn sessions(calendar: &[TradingDay], now: DateTime<Utc>) -> (NaiveDate, NaiveDate) {
    let today = exchange::to_exchange(now).date_naive();
    let mut found = (0..30)
        .map(|i| today - chrono::Duration::days(i))
        .filter_map(|d| exchange::trading_day(d, calendar))
        .filter(|d| {
            exchange::exchange_to_utc(d.date.and_time(d.session_open)).is_some_and(|t| t <= now)
        })
        .map(|d| d.date);
    let latest = found.next().unwrap_or(today);
    let previous = found.next().unwrap_or(latest - chrono::Duration::days(1));
    (latest, previous)
}

/// Snapshots and live prices for a set of securities, merged per row.
pub struct Board {
    pub snapshots: Snapshot<Snapshots>,
    pub live: Snapshot<LiveMarket>,
}

/// Watch `symbols` (call every frame): minute snapshots plus the stream's
/// trades, quotes and minute bars.
pub fn board(cx: &PanelCx<'_>, symbols: &[String]) -> Board {
    let snapshots = cx.hub.watch(&cx.alpaca.snapshots(symbols));
    let topics = mt_alpaca::market_topics(symbols);
    let topics: Vec<&str> = topics.iter().map(String::as_str).collect();
    let live = cx.hub.watch_stream(&cx.alpaca.market_stream(), &topics);
    Board { snapshots, live }
}

impl Board {
    pub fn row(&self, symbol: &str) -> Row {
        mt_alpaca::board::row(
            symbol,
            self.snapshots.data().and_then(|s| s.get(symbol)),
            self.live.data(),
        )
    }

    /// The stream's complaint, if any (a subscription limit, a busy connection).
    pub fn notice(&self) -> Option<String> {
        self.live
            .data()
            .and_then(|l| l.notice.clone())
            .or_else(|| self.live.error.as_ref().map(ToString::to_string))
    }
}

/// The asset list (empty until loaded), for names and completion.
pub fn assets(cx: &PanelCx<'_>) -> Arc<AssetList> {
    if !cx.alpaca.is_ready() {
        return Arc::default();
    }
    cx.hub.watch(&cx.alpaca.assets()).data.unwrap_or_default()
}

/// A security's name from the asset list, if known.
pub fn name_of(assets: &AssetList, symbol: &str) -> Option<String> {
    assets
        .get(symbol)
        .map(|a| a.short_name().to_owned())
        .filter(|n| !n.is_empty())
}

/// The security a route argument names, if any.
pub fn security_of(arg: &str) -> Option<Security> {
    match Instrument::parse_security(arg)? {
        Instrument::Security(s) => Some(s),
        _ => None,
    }
}

/// Number formats for securities.
pub mod fmt {
    use chrono::{DateTime, Utc};
    use mt_core::exchange;

    pub use crate::widgets::fmt::DASH;

    /// Dollars per share: two decimals, four below a dollar.
    pub fn price(v: f64) -> String {
        if v.abs() < 1.0 {
            format!("{v:.4}")
        } else {
            format!("{v:.2}")
        }
    }

    pub fn price_opt(v: Option<f64>) -> String {
        v.map_or_else(|| DASH.to_owned(), price)
    }

    pub fn change_opt(v: Option<f64>) -> String {
        v.map_or_else(|| DASH.to_owned(), |v| format!("{v:+.2}"))
    }

    pub fn pct_opt(v: Option<f64>) -> String {
        v.map_or_else(|| DASH.to_owned(), |v| format!("{v:+.2}%"))
    }

    /// `8,250`, `123.4K`, `12.35M`, `1.20B`.
    pub fn volume(v: f64) -> String {
        let a = v.abs();
        if a >= 1e9 {
            format!("{:.2}B", v / 1e9)
        } else if a >= 1e6 {
            format!("{:.2}M", v / 1e6)
        } else if a >= 1e5 {
            format!("{:.1}K", v / 1e3)
        } else {
            crate::widgets::fmt::mw(v)
        }
    }

    pub fn volume_opt(v: Option<f64>) -> String {
        v.map_or_else(|| DASH.to_owned(), volume)
    }

    /// `15:59:59` in New York, or `Oct 01` before today.
    pub fn time(t: DateTime<Utc>) -> String {
        let local = exchange::to_exchange(t);
        if local.date_naive() == exchange::now_exchange().date_naive() {
            local.format("%H:%M:%S").to_string()
        } else {
            local.format("%b %d").to_string()
        }
    }

    /// `Oct 02 15:59:59 EDT`.
    pub fn full_time(t: DateTime<Utc>) -> String {
        let local = exchange::to_exchange(t);
        format!(
            "{} {}",
            local.format("%b %d %H:%M:%S"),
            exchange::tz_label(&local)
        )
    }
}

/// A security search field: suggestions from the asset list as you type.
#[derive(Default)]
pub struct SecurityPicker {
    search: String,
}

impl SecurityPicker {
    /// Draw it; returns a security when one is chosen (a suggestion, or Enter
    /// on a ticker the asset list knows, or any ticker when it has not loaded).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        cx: &PanelCx<'_>,
        id: &str,
        hint: &str,
    ) -> Option<Security> {
        let assets = assets(cx);
        let mut chosen = None;
        ui.vertical(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id_salt(("security-picker", id))
                    .hint_text(hint)
                    .desired_width(180.0),
            );
            let typed = self.search.trim().to_owned();
            if resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !typed.is_empty()
            {
                chosen = match security_of(&typed) {
                    Some(s) => Some(s),
                    None => {
                        let best = assets.search(&typed, 1).first().map(|a| a.security());
                        best.or_else(|| {
                            (assets.assets.is_empty() && mt_core::instrument::is_ticker(&typed))
                                .then(|| Security::us(&typed))
                        })
                    }
                };
            }
            if !typed.is_empty() && chosen.is_none() {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(460.0);
                    for a in assets.search(&typed, 8) {
                        if widgets::link(ui, cx.skin, &a.symbol)
                            .on_hover_text(&a.name)
                            .clicked()
                        {
                            chosen = Some(a.security());
                        }
                    }
                });
            }
        });
        if chosen.is_some() {
            self.search.clear();
        }
        chosen
    }
}

/// `XLU US` and its name, for the command line's completions.
pub fn completions(assets: &AssetList, text: &str, limit: usize) -> Vec<(String, String)> {
    assets
        .search(text, limit)
        .into_iter()
        .map(|a: &Asset| (a.security().to_string(), a.short_name().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn sessions_start_with_pre_market() {
        let cal: Vec<TradingDay> = ["2026-10-01", "2026-10-02", "2026-10-05"]
            .iter()
            .filter_map(|d| TradingDay::standard(d.parse().unwrap()))
            .collect();
        let d = |s: &str| s.parse::<NaiveDate>().unwrap();
        // Saturday: Friday's session, and Thursday's before it.
        assert_eq!(
            sessions(&cal, utc("2026-10-03T15:00:00Z")),
            (d("2026-10-02"), d("2026-10-01"))
        );
        // Monday 03:00 ET, before pre-market: still Friday's.
        assert_eq!(
            sessions(&cal, utc("2026-10-05T07:00:00Z")).0,
            d("2026-10-02")
        );
        // Monday 05:00 ET: Monday's.
        assert_eq!(
            sessions(&cal, utc("2026-10-05T09:00:00Z")),
            (d("2026-10-05"), d("2026-10-02"))
        );
    }

    #[test]
    fn formats() {
        assert_eq!(fmt::price(82.5), "82.50");
        assert_eq!(fmt::price(0.4567), "0.4567");
        assert_eq!(fmt::volume(8_250.0), "8,250");
        assert_eq!(fmt::volume(123_456.0), "123.5K");
        assert_eq!(fmt::volume(12_345_678.0), "12.35M");
        assert_eq!(fmt::pct_opt(Some(1.5)), "+1.50%");
        let s = MarketStatus {
            session: Session::Regular,
            next: None,
        };
        assert_eq!(status_text(&s), "Open");
        assert_eq!(security_of("XLU US").map(|s| s.ticker), Some("XLU".into()));
        assert_eq!(security_of("MINN.HUB"), None);
    }
}
