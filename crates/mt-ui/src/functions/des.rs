//! DES: a security's description. What it is and where it lists (Alpaca's
//! asset record: exchange, shortable, marginable, fractional), today's trading
//! and its range and returns over the past year (from every exchange's daily
//! bars).

use egui::{Grid, RichText, ScrollArea, Ui};
use mt_alpaca::Timeframe;
use mt_core::equity::{Asset, Bar};
use mt_core::instrument::Security;

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker, fmt};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "DES",
    aliases: &["DESC", "DESCRIBE"],
    name: "Security description",
    category: Category::Markets,
    usage: "DES <security>",
    description: "What a stock or ETF is and where it lists, how it trades with Alpaca (shortable, marginable, fractional), today's prices and its range and returns over the past year.",
    takes_node: false,
    takes_security: true,
    takes_option: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Des {
        security: args.iter().find_map(|a| market::security_of(a)),
        picker: SecurityPicker::default(),
    }))
}

struct Des {
    security: Option<Security>,
    picker: SecurityPicker,
}

/// Days of daily bars behind the 52-week figures (a little over a year).
const YEAR_DAYS: i64 = 372;

/// Return from the last close on or before `days` ago to the latest close.
fn change_over(bars: &[Bar], days: i64) -> Option<f64> {
    let last = bars.last()?;
    let cutoff = last.time - chrono::Duration::days(days);
    let base = bars.iter().rev().find(|b| b.time <= cutoff)?;
    (base.close > 0.0).then(|| (last.close / base.close - 1.0) * 100.0)
}

/// Return since the last close of the previous year.
fn year_to_date(bars: &[Bar]) -> Option<f64> {
    use chrono::Datelike;
    let last = bars.last()?;
    let year = mt_core::exchange::to_exchange(last.time).year();
    let base = bars
        .iter()
        .rev()
        .find(|b| mt_core::exchange::to_exchange(b.time).year() < year)?;
    (base.close > 0.0).then(|| (last.close / base.close - 1.0) * 100.0)
}

fn flags(a: &Asset) -> Vec<(&'static str, String)> {
    let yes = |b: bool| if b { "Yes" } else { "No" }.to_owned();
    let mut out = vec![
        ("Listed on", a.exchange_name().to_owned()),
        (
            "Status",
            if a.active { "Active" } else { "Inactive" }.to_owned(),
        ),
        ("Tradable with Alpaca", yes(a.tradable)),
        ("Fractional shares", yes(a.fractionable)),
        (
            "Shortable",
            match (a.shortable, a.easy_to_borrow) {
                (true, true) => "Yes, easy to borrow".to_owned(),
                (true, false) => "Yes, hard to borrow".to_owned(),
                (false, _) => "No".to_owned(),
            },
        ),
        (
            "Marginable",
            match (a.marginable, a.maintenance_margin) {
                (true, Some(m)) => format!("Yes, {m:.0}% maintenance margin"),
                (m, _) => yes(m),
            },
        ),
        ("Options listed", yes(a.has_attribute("has_options"))),
    ];
    if a.attributes.iter().any(|x| x.starts_with("ptp")) {
        out.push((
            "Publicly traded partnership",
            "Yes: foreign accounts may face withholding on sales".to_owned(),
        ));
    }
    out
}

impl Panel for Des {
    fn title(&self) -> String {
        match &self.security {
            Some(s) => format!("DES {s}"),
            None => "DES".into(),
        }
    }

    fn route(&self) -> Route {
        Route::new("DES", self.security.iter().map(ToString::to_string))
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        ui.horizontal_wrapped(|ui| {
            if let Some(s) = &self.security {
                ui.label(
                    RichText::new(s.to_string())
                        .heading()
                        .color(skin.text_strong),
                );
            }
            if let Some(s) = self.picker.show(ui, cx, "des", "another security…") {
                self.security = Some(s);
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
        let Some(sec) = self.security.clone() else {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Pick a security above, or type DES XLU US on the command line.")
                    .color(skin.text_muted),
            );
            return;
        };
        let sym = sec.ticker.clone();
        let assets = cx.hub.watch(&cx.alpaca.assets());
        let asset = assets.data().and_then(|l| l.get(&sym)).cloned();
        let board = market::board(cx, std::slice::from_ref(&sym));
        let row = board.row(&sym);
        let today = mt_core::exchange::now_exchange().date_naive();
        let daily = cx.hub.watch(&cx.alpaca.bars(
            [sym.as_str()],
            Timeframe::Day1,
            today - chrono::Duration::days(YEAR_DAYS),
            None,
        ));
        let bars: Vec<Bar> = daily
            .data()
            .map(|d| d.get(&sym).to_vec())
            .unwrap_or_default();

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            match &asset {
                Some(a) => {
                    ui.label(RichText::new(&a.name).size(18.0).color(skin.text_strong));
                }
                None if assets.data.is_some() => {
                    ui.label(
                        RichText::new(format!(
                            "{sym} is not in Alpaca's list of active US stocks and ETFs. Check the ticker."
                        ))
                        .color(skin.warning),
                    );
                }
                None => widgets::placeholder(ui, skin, assets.error.as_ref().map(ToString::to_string)),
            }
            ui.horizontal_wrapped(|ui| {
                let options = asset.as_ref().is_some_and(|a| a.has_attribute("has_options"));
                for (code, what) in [
                    ("GP", "Chart"),
                    ("CN", "Company news"),
                    ("Q", "Quote"),
                    ("OMON", "Options"),
                ] {
                    if code == "OMON" && !options {
                        continue;
                    }
                    if ui.button(format!("{code} · {what}")).clicked() {
                        cx.open(Route::new(code, [sec.to_string()]));
                    }
                }
                let watched = cx.config.ui.favorite_securities.contains(&sec.to_string());
                let (label, cmd) = if watched {
                    ("★ Watching", AppCommand::RemoveFavorite(sec.to_string()))
                } else {
                    ("☆ Watch", AppCommand::AddFavorite(sec.to_string()))
                };
                if ui.button(label).on_hover_text("Add to or remove from WL").clicked() {
                    cx.send(cmd);
                }
            });

            widgets::section(ui, skin, &format!("Trading · {}", cx.alpaca.feed().label()));
            ui.horizontal_wrapped(|ui| {
                let sub = row
                    .last_time
                    .map(|t| RichText::new(fmt::full_time(t)).color(skin.text_muted));
                widgets::stat_tile(ui, skin, "Last", &fmt::price_opt(row.last), sub);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Change",
                    &fmt::change_opt(row.change),
                    Some(RichText::new(fmt::pct_opt(row.change_pct)).color(row.change.map_or(skin.text_muted, |c| skin.delta(c)))),
                );
                widgets::stat_tile(ui, skin, "Previous close", &fmt::price_opt(row.prev_close), None);
                widgets::stat_tile(
                    ui,
                    skin,
                    "Day range",
                    &match (row.low, row.high) {
                        (Some(l), Some(h)) => format!("{}–{}", fmt::price(l), fmt::price(h)),
                        _ => fmt::DASH.into(),
                    },
                    row.open.map(|o| RichText::new(format!("open {}", fmt::price(o))).color(skin.text_muted)),
                );
                widgets::stat_tile(
                    ui,
                    skin,
                    "Volume",
                    &fmt::volume_opt(row.volume),
                    row.vwap.map(|v| RichText::new(format!("VWAP {}", fmt::price(v))).color(skin.text_muted)),
                );
            });

            widgets::section(ui, skin, "Past year · every exchange, daily");
            if bars.is_empty() {
                widgets::placeholder(ui, skin, daily.error.as_ref().map(ToString::to_string));
            } else {
                let year: Vec<&Bar> = bars
                    .iter()
                    .filter(|b| bars.last().is_some_and(|l| l.time - b.time <= chrono::Duration::days(365)))
                    .collect();
                let hi = year.iter().map(|b| b.high).fold(f64::MIN, f64::max);
                let lo = year.iter().map(|b| b.low).fold(f64::MAX, f64::min);
                let recent: Vec<f64> = bars.iter().rev().take(63).map(|b| b.volume).collect();
                let avg_vol = recent.iter().sum::<f64>() / recent.len().max(1) as f64;
                ui.horizontal_wrapped(|ui| {
                    widgets::stat_tile(ui, skin, "52-week range", &format!("{}–{}", fmt::price(lo), fmt::price(hi)), None);
                    widgets::stat_tile(ui, skin, "Avg volume (3 months)", &fmt::volume(avg_vol), None);
                    for (label, v) in [
                        ("1 month", change_over(&bars, 30)),
                        ("3 months", change_over(&bars, 91)),
                        ("Year to date", year_to_date(&bars)),
                        ("1 year", change_over(&bars, 365)),
                    ] {
                        let resp = widgets::stat_tile(ui, skin, label, &fmt::pct_opt(v), None);
                        if v.is_none() {
                            resp.on_hover_text("Not enough history");
                        }
                    }
                });
            }

            if let Some(a) = &asset {
                widgets::section(ui, skin, "Listing");
                Grid::new("des-flags")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (k, v) in flags(a) {
                            ui.label(RichText::new(k).color(skin.text_muted));
                            ui.label(v);
                            ui.end_row();
                        }
                    });
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Prices and the asset record come from Alpaca. Information only, not advice.",
                )
                .small()
                .color(skin.text_muted),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(day: &str, close: f64) -> Bar {
        Bar {
            time: format!("{day}T04:00:00Z").parse().unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
            trades: None,
            vwap: None,
        }
    }

    #[test]
    fn returns_over_windows() {
        let bars = vec![
            bar("2025-12-31", 100.0),
            bar("2026-09-01", 110.0),
            bar("2026-10-02", 121.0),
        ];
        assert!((change_over(&bars, 30).unwrap() - 10.0).abs() < 1e-9);
        assert!((year_to_date(&bars).unwrap() - 21.0).abs() < 1e-9);
        assert_eq!(change_over(&bars, 3650), None, "not enough history");
    }
}
