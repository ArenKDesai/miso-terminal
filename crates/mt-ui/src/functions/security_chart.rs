//! GP for a security (`GP XLU US`, `XLU US GP 365`): the latest session at
//! one-minute resolution against the previous close, a few days at fifteen
//! minutes, or daily closes over months and years, with volume underneath.
//! Times are New York time.

use egui::{RichText, Ui};
use egui_plot::{Bar as PlotBar, BarChart, HLine, Line, LineStyle, PlotPoints, Span};
use mt_alpaca::Timeframe;
use mt_core::equity::Bar;
use mt_core::exchange;
use mt_core::instrument::Security;

use crate::context::{AppCommand, PanelCx};
use crate::function::{Panel, Route};
use crate::market::{self, SecurityPicker, fmt};
use crate::widgets::chart::{exchange_plot, utc_x};
use crate::widgets::{self, csv};

/// The longest window offered: ten years of daily bars.
const MAX_DAYS: u32 = 3650;
/// Up to this many days are drawn from fifteen-minute bars.
const INTRADAY_DAYS: u32 = 10;
/// The window buttons: label and days (0 is the latest session).
const WINDOWS: [(&str, u32); 8] = [
    ("1D", 0),
    ("5D", 5),
    ("1M", 30),
    ("3M", 91),
    ("6M", 182),
    ("1Y", 365),
    ("5Y", 1826),
    ("10Y", 3650),
];

pub(crate) fn open(security: Security, args: &[String]) -> Result<Box<dyn Panel>, String> {
    let days = match args.get(1) {
        Some(d) => d
            .parse::<u32>()
            .map_err(|_| format!("days must be a number, got {d:?}"))?
            .min(MAX_DAYS),
        None => 0,
    };
    Ok(Box::new(SecurityChart {
        security,
        days,
        picker: SecurityPicker::default(),
    }))
}

struct SecurityChart {
    security: Security,
    /// 0: the latest session at one minute.
    days: u32,
    picker: SecurityPicker,
}

/// Summary of a window of bars.
struct Window {
    first: f64,
    last: f64,
    high: f64,
    low: f64,
    volume: f64,
}

fn window(bars: &[Bar]) -> Option<Window> {
    Some(Window {
        first: bars.first()?.open,
        last: bars.last()?.close,
        high: bars.iter().map(|b| b.high).fold(f64::MIN, f64::max),
        low: bars.iter().map(|b| b.low).fold(f64::MAX, f64::min),
        volume: bars.iter().map(|b| b.volume).sum(),
    })
}

/// Annualised volatility of daily log returns, in percent.
fn volatility(bars: &[Bar]) -> Option<f64> {
    let r: Vec<f64> = bars
        .windows(2)
        .filter(|w| w[0].close > 0.0 && w[1].close > 0.0)
        .map(|w| (w[1].close / w[0].close).ln())
        .collect();
    if r.len() < 5 {
        return None;
    }
    let mean = r.iter().sum::<f64>() / r.len() as f64;
    let var = r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (r.len() - 1) as f64;
    Some(var.sqrt() * 252f64.sqrt() * 100.0)
}

impl Panel for SecurityChart {
    fn title(&self) -> String {
        format!("GP {}", self.security)
    }

    fn route(&self) -> Route {
        let mut args = vec![self.security.to_string()];
        if self.days > 0 {
            args.push(self.days.to_string());
        }
        Route::new("GP", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let sec = self.security.to_string();
        let sym = self.security.ticker.clone();
        let assets = market::assets(cx);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&sec).heading().color(skin.text_strong));
            if let Some(name) = market::name_of(&assets, &sym) {
                ui.label(RichText::new(name).color(skin.text_muted));
            }
            let watched = cx.config.ui.favorite_securities.contains(&sec);
            let (label, cmd) = if watched {
                ("★ Watching", AppCommand::RemoveFavorite(sec.clone()))
            } else {
                ("☆ Watch", AppCommand::AddFavorite(sec.clone()))
            };
            if ui
                .small_button(label)
                .on_hover_text("Add to or remove from WL")
                .clicked()
            {
                cx.send(cmd);
            }
            for code in ["DES", "CN"] {
                if widgets::link(ui, skin, code).clicked() {
                    cx.open(Route::new(code, [sec.clone()]));
                }
            }
            if let Some(s) = self.picker.show(ui, cx, "gp-sec", "another security…") {
                self.security = s;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if cx.alpaca.is_ready() {
                    market::status_label(ui, cx);
                }
            });
        });
        if market::needs_keys(ui, cx) {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for (label, days) in WINDOWS {
                ui.selectable_value(&mut self.days, days, label);
            }
        });

        let board = market::board(cx, std::slice::from_ref(&sym));
        let row = board.row(&sym);
        ui.horizontal_wrapped(|ui| {
            let when = row
                .last_time
                .map(|t| format!("{} · {}", fmt::time(t), cx.alpaca.feed().label()));
            widgets::stat_tile(
                ui,
                skin,
                "Last",
                &fmt::price_opt(row.last),
                when.map(|w| RichText::new(w).color(skin.text_muted)),
            );
            widgets::stat_tile(
                ui,
                skin,
                "Change",
                &fmt::change_opt(row.change),
                Some(
                    RichText::new(fmt::pct_opt(row.change_pct))
                        .color(row.change.map_or(skin.text_muted, |c| skin.delta(c))),
                ),
            );
            let q = row.quote.as_ref().filter(|q| q.is_two_sided());
            for (label, side) in [
                ("Bid", q.map(|q| (q.bid, q.bid_size))),
                ("Ask", q.map(|q| (q.ask, q.ask_size))),
            ] {
                widgets::stat_tile(
                    ui,
                    skin,
                    label,
                    &fmt::price_opt(side.map(|s| s.0)),
                    side.map(|s| RichText::new(format!("size {}", s.1)).color(skin.text_muted)),
                );
            }
            widgets::stat_tile(
                ui,
                skin,
                "Day range",
                &match (row.low, row.high) {
                    (Some(l), Some(h)) => format!("{}–{}", fmt::price(l), fmt::price(h)),
                    _ => fmt::DASH.into(),
                },
                row.prev_close.map(|p| {
                    RichText::new(format!("prev close {}", fmt::price(p))).color(skin.text_muted)
                }),
            );
            widgets::stat_tile(
                ui,
                skin,
                "Volume",
                &fmt::volume_opt(row.volume),
                row.vwap.map(|v| {
                    RichText::new(format!("VWAP {}", fmt::price(v))).color(skin.text_muted)
                }),
            );
        });

        let cal = market::calendar(cx.hub, cx.alpaca);
        let now = mt_core::time::now_utc();
        let (latest, previous) = market::sessions(&cal, now);
        let today = exchange::to_exchange(now).date_naive();
        let (timeframe, start) = match self.days {
            0 => (Timeframe::Min1, previous),
            d if d <= INTRADAY_DAYS => (
                Timeframe::Min15,
                today - chrono::Duration::days(i64::from(d)),
            ),
            d => (
                Timeframe::Day1,
                today - chrono::Duration::days(i64::from(d)),
            ),
        };
        let snap = cx
            .hub
            .watch(&cx.alpaca.bars([sym.as_str()], timeframe, start, None));
        let Some(set) = snap.data() else {
            widgets::placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string));
            return;
        };
        let all = set.get(&sym);
        // The latest session view shows the latest session with any bars.
        let (bars, session): (Vec<Bar>, Option<chrono::NaiveDate>) = if self.days == 0 {
            let date_of = |b: &Bar| exchange::to_exchange(b.time).date_naive();
            let shown = all
                .iter()
                .rev()
                .map(date_of)
                .find(|d| *d == latest)
                .or_else(|| all.last().map(date_of));
            (
                all.iter()
                    .filter(|b| Some(date_of(b)) == shown)
                    .cloned()
                    .collect(),
                shown,
            )
        } else {
            (all.to_vec(), None)
        };

        ui.horizontal_wrapped(|ui| {
            let mut note = format!("{} bars · {}", timeframe.label(), set.source.label());
            if let Some(d) = session {
                note += &format!(" · session of {}", d.format("%a %b %d"));
            }
            if set.truncated {
                note += " · more bars than one request returns; the start is cut";
            }
            ui.label(RichText::new(note).small().color(skin.text_muted));
            csv::copy_button(ui, skin, || {
                csv::to_csv(
                    &["time_utc", "open", "high", "low", "close", "volume", "vwap"],
                    bars.iter().map(|b| {
                        vec![
                            b.time.to_rfc3339(),
                            b.open.to_string(),
                            b.high.to_string(),
                            b.low.to_string(),
                            b.close.to_string(),
                            b.volume.to_string(),
                            b.vwap.map_or_else(String::new, |v| v.to_string()),
                        ]
                    }),
                )
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::freshness(ui, skin, &snap);
            });
        });
        if bars.is_empty() {
            ui.label(
                RichText::new(format!(
                    "No {} bars for {sec} in this window.",
                    timeframe.label()
                ))
                .color(skin.text_muted),
            );
            return;
        }
        if self.days > 0
            && let Some(w) = window(&bars)
        {
            ui.horizontal_wrapped(|ui| {
                let ret = (w.first > 0.0).then(|| (w.last / w.first - 1.0) * 100.0);
                widgets::stat_tile(ui, skin, "Return", &fmt::pct_opt(ret), None);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Range",
                    &format!("{}–{}", fmt::price(w.low), fmt::price(w.high)),
                    None,
                );
                if timeframe == Timeframe::Day1 {
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Avg daily volume",
                        &fmt::volume(w.volume / bars.len() as f64),
                        None,
                    );
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Volatility",
                        &volatility(&bars).map_or_else(|| fmt::DASH.into(), |v| format!("{v:.1}%")),
                        Some(RichText::new("annualised, daily").color(skin.text_muted)),
                    );
                }
            });
        }

        let up = match (bars.first(), bars.last(), self.days) {
            (_, Some(l), 0) => row.prev_close.is_none_or(|p| l.close >= p),
            (Some(f), Some(l), _) => l.close >= f.open,
            _ => true,
        };
        let color = if up { skin.positive } else { skin.negative };
        let secs = timeframe.seconds() as f64;
        let points: Vec<[f64; 2]> = bars
            .iter()
            .map(|b| {
                [
                    utc_x(b.time) + if secs < 86_400.0 { secs } else { 0.0 },
                    b.close,
                ]
            })
            .collect();
        let id = format!("gp-sec-{sym}");
        let link = egui::Id::new(("gp-sec-link", &sym));
        let height = ui.available_height();
        let price_h = (height * 0.74).max(120.0);
        exchange_plot(&format!("{id}-price"), skin)
            .height(price_h)
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .show_x(false)
            .show(ui, |plot| {
                if self.days == 0
                    && let Some(d) = session.and_then(|d| exchange::trading_day(d, &cal))
                    && let Some((open, close)) = d.regular_utc()
                {
                    plot.span(
                        Span::new("Regular session", utc_x(open)..=utc_x(close))
                            .fill(skin.grid.gamma_multiply(0.35))
                            .border_width(0.0),
                    );
                }
                if self.days == 0
                    && let Some(p) = row.prev_close
                {
                    plot.hline(
                        HLine::new("Previous close", p)
                            .color(skin.text_muted)
                            .style(LineStyle::dashed_dense())
                            .width(1.0),
                    );
                }
                plot.line(
                    Line::new(sec.as_str(), PlotPoints::from(points))
                        .color(color)
                        .width(1.6),
                );
            });
        let volume: Vec<PlotBar> = bars
            .iter()
            .map(|b| {
                let x = utc_x(b.time) + if secs < 86_400.0 { secs / 2.0 } else { 0.0 };
                PlotBar::new(x, b.volume)
                    .width(secs.min(86_400.0) * 0.8)
                    .fill(skin.series(2).gamma_multiply(0.7))
            })
            .collect();
        exchange_plot(&format!("{id}-volume"), skin)
            .height((ui.available_height() - 4.0).max(48.0))
            .link_axis(link, [true, false])
            .link_cursor(link, [true, false])
            .y_axis_formatter(|m, _| fmt::volume(m.value))
            .include_y(0.0)
            .show(ui, |plot| {
                plot.bar_chart(BarChart::new("Volume", volume).color(skin.series(2)));
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_volatility() {
        let p = open(Security::us("XLU"), &["XLU US".into(), "365".into()]).unwrap();
        assert_eq!(p.route(), Route::new("GP", ["XLU US", "365"]));
        let p = open(Security::us("XLU"), &["XLU US".into()]).unwrap();
        assert_eq!(p.route().to_string(), "GP XLU US");
        assert!(open(Security::us("XLU"), &["XLU US".into(), "x".into()]).is_err());
        let flat: Vec<Bar> = (0..10)
            .map(|i| Bar {
                time: chrono::DateTime::from_timestamp(i * 86_400, 0).unwrap(),
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
                trades: None,
                vwap: None,
            })
            .collect();
        assert_eq!(volatility(&flat), Some(0.0));
        assert_eq!(volatility(&flat[..3]), None);
    }
}
