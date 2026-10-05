//! PNL: the paper account's equity curve from Alpaca's portfolio history,
//! over the latest session (five-minute points), a week (hourly), or one,
//! three or twelve months (daily), with the change, range and deepest
//! drawdown over the period. Times are New York time.

use egui::{RichText, Ui};
use egui_plot::{HLine, Line, LineStyle, PlotPoints};
use mt_alpaca::HistoryPeriod;
use mt_core::account::PortfolioHistory;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market;
use crate::portfolio;
use crate::widgets::chart::{exchange_plot, utc_x};
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "PNL",
    aliases: &["EQUITY", "PERF"],
    name: "Profit and loss",
    category: Category::Account,
    usage: "PNL [1D | 1W | 1M | 3M | 1Y]",
    description: "The Alpaca paper account's equity curve: today at five minutes, a week hourly, or one, three or twelve months daily, with the change, high, low and deepest drawdown (PNL 1M).",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let period = match args.first() {
        Some(a) => HistoryPeriod::parse(a)
            .ok_or_else(|| format!("unknown period {a:?}: use 1D, 1W, 1M, 3M or 1Y"))?,
        None => HistoryPeriod::Day,
    };
    Ok(Box::new(Pnl { period }))
}

struct Pnl {
    period: HistoryPeriod,
}

impl Panel for Pnl {
    fn title(&self) -> String {
        match self.period {
            HistoryPeriod::Day => "PNL".into(),
            p => format!("PNL {}", p.label()),
        }
    }

    fn route(&self) -> Route {
        match self.period {
            HistoryPeriod::Day => Route::code("PNL"),
            p => Route::new("PNL", [p.label()]),
        }
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        if let Some(p) = args.first().and_then(|a| HistoryPeriod::parse(a)) {
            self.period = p;
        }
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let history = if cx.alpaca.is_ready() {
            Some(cx.hub.watch(&cx.alpaca.portfolio_history(self.period)))
        } else {
            None
        };
        widgets::title_bar(
            ui,
            skin,
            &format!("Equity · Alpaca {} account", cx.alpaca.mode().name()),
            |ui| {
                if let Some(h) = &history {
                    widgets::freshness(ui, skin, h);
                }
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let Some(snap) = history else { return };
        ui.horizontal_wrapped(|ui| {
            for p in HistoryPeriod::ALL {
                if ui.selectable_label(self.period == p, p.label()).clicked() {
                    self.period = p;
                }
            }
            if let Some(h) = snap.data() {
                ui.add_space(8.0);
                csv::copy_button(ui, skin, || to_csv(h));
            }
        });
        widgets::with_data(ui, skin, &snap, |ui, h| {
            if h.points.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("No equity history for this period yet.").color(skin.text_muted),
                );
                return;
            }
            tiles(ui, cx, h, self.period);
            chart(ui, cx, h, self.period);
            ui.label(
                RichText::new(match self.period {
                    HistoryPeriod::Day => {
                        "Regular hours, every five minutes, from the previous close. Alpaca's figures, refreshed each minute."
                    }
                    HistoryPeriod::Week => "Regular hours, hourly, from the close before the week.",
                    _ => "Daily closes, from the close before the period. Deposits and withdrawals count as P&L.",
                })
                .small()
                .color(skin.text_muted),
            );
        });
    }
}

fn tiles(ui: &mut Ui, cx: &PanelCx<'_>, h: &PortfolioHistory, period: HistoryPeriod) {
    let skin = cx.skin;
    let Some(last) = h.last() else { return };
    // Daily points are stamped at midnight: show the date alone.
    let as_of = if period.is_intraday() {
        market::fmt::full_time(last.time)
    } else {
        mt_core::exchange::to_exchange(last.time)
            .format("%a %b %d %Y")
            .to_string()
    };
    ui.horizontal_wrapped(|ui| {
        widgets::stat_tile(
            ui,
            skin,
            "Equity",
            &usd(last.equity),
            Some(RichText::new(as_of).color(skin.text_muted)),
        );
        if let Some((chg, pct)) = h.change() {
            widgets::stat_tile(
                ui,
                skin,
                "Change",
                &signed_usd(chg),
                Some(
                    RichText::new(format!(
                        "{} from {}",
                        portfolio::pct_signed(pct),
                        usd(h.start_value().unwrap_or_default())
                    ))
                    .color(skin.delta(chg)),
                ),
            );
        }
        if let Some((lo, hi)) = h.range() {
            widgets::stat_tile(ui, skin, "High", &usd(hi), None);
            widgets::stat_tile(ui, skin, "Low", &usd(lo), None);
        }
        if let Some(dd) = h.max_drawdown() {
            widgets::stat_tile(
                ui,
                skin,
                "Max drawdown",
                &format!("{:.2}%", dd * 100.0),
                Some(RichText::new("from a running high").color(skin.text_muted)),
            )
            .on_hover_text("The deepest fall from the highest equity before it, in this period.");
        }
    });
}

fn chart(ui: &mut Ui, cx: &PanelCx<'_>, h: &PortfolioHistory, period: HistoryPeriod) {
    let skin = cx.skin;
    let pts: Vec<[f64; 2]> = h.points.iter().map(|p| [utc_x(p.time), p.equity]).collect();
    let base = h.start_value();
    let color = h.change().map_or(skin.series(0), |(c, _)| skin.delta(c));
    let height = (ui.available_height() - 24.0).max(160.0);
    exchange_plot(&format!("pnl-{}", period.label()), skin)
        .height(height)
        .y_axis_formatter(|m, _| usd_short(m.value))
        .show(ui, |plot| {
            if let Some(b) = base {
                plot.hline(
                    HLine::new("Start", b)
                        .color(skin.text_muted)
                        .style(LineStyle::dashed_dense())
                        .width(1.0),
                );
            }
            plot.line(
                Line::new("Equity", PlotPoints::from(pts))
                    .color(color)
                    .width(1.6),
            );
        });
}

fn usd(v: f64) -> String {
    mt_core::account::from_f64(v, 2).map_or_else(|| market::fmt::DASH.to_owned(), portfolio::usd)
}

fn signed_usd(v: f64) -> String {
    mt_core::account::from_f64(v, 2)
        .map_or_else(|| market::fmt::DASH.to_owned(), portfolio::usd_signed)
}

/// `$103.9K`, `$1.25M`, for the axis.
fn usd_short(v: f64) -> String {
    let a = v.abs();
    if a >= 1e6 {
        format!("${:.2}M", v / 1e6)
    } else if a >= 1e4 {
        format!("${:.1}K", v / 1e3)
    } else {
        format!("${v:.0}")
    }
}

fn to_csv(h: &PortfolioHistory) -> String {
    csv::to_csv(
        &["time_utc", "equity", "profit_loss", "profit_loss_pct"],
        h.points.iter().map(|p| {
            vec![
                p.time.to_rfc3339(),
                format!("{}", p.equity),
                format!("{}", p.profit_loss),
                p.profit_loss_pct
                    .map_or_else(String::new, |v| format!("{v}")),
            ]
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_labels() {
        assert_eq!(usd_short(103_957.5), "$104.0K");
        assert_eq!(usd_short(1_250_000.0), "$1.25M");
        assert_eq!(usd_short(950.0), "$950");
        assert_eq!(signed_usd(504.3), "+$504.30");
    }
}
