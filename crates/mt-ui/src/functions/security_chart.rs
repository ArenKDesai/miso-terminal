//! GP for a security (`GP XLU US`, `XLU US GP 365`): the latest session at
//! one-minute resolution against the previous close, a few days at fifteen
//! minutes, or daily closes over months and years, with volume underneath.
//! Studies (`GP XLU US 365 SMA50 RSI14`) draw over the price, or in panes
//! between the price and the volume. Times are New York time.

use std::ops::Range;

use egui::containers::menu::{MenuButton, MenuConfig};
use egui::{Color32, PopupCloseBehavior, RichText, Ui};
use egui_plot::{
    Bar as PlotBar, BarChart, Corner, HLine, Legend, Line, LineStyle, PlotPoints, PlotUi, Span,
};
use mt_alpaca::Timeframe;
use mt_core::equity::Bar;
use mt_core::exchange;
use mt_core::instrument::Security;
use mt_core::studies::{self, Study, Values};

use crate::context::{AppCommand, PanelCx};
use crate::function::{Panel, Route};
use crate::market::{self, SecurityPicker, fmt};
use crate::skin::Skin;
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
/// The route word for a chart whose studies were all turned off, so it
/// reopens without them rather than with `[markets] studies`.
const NO_STUDIES: &str = "NOSTUDIES";
/// The theme's series colours studies take in turn. Series 2 is the volume's,
/// and series 3 sits too close to a falling price.
const STUDY_SERIES: [usize; 6] = [0, 1, 4, 5, 6, 7];

pub(crate) fn open(security: Security, args: &[String]) -> Result<Box<dyn Panel>, String> {
    let mut days = None;
    let mut studies: Option<Vec<Study>> = None;
    for arg in args.iter().skip(1) {
        if let Ok(d) = arg.parse::<u32>() {
            if days.replace(d.min(MAX_DAYS)).is_some() {
                return Err(format!("one number of days only, got {arg} as well"));
            }
        } else if arg.eq_ignore_ascii_case(NO_STUDIES) {
            studies.get_or_insert_with(Vec::new);
        } else if let Some(study) = Study::parse(arg) {
            let list = studies.get_or_insert_with(Vec::new);
            if !list.contains(&study) {
                list.push(study);
            }
        } else {
            return Err(format!(
                "{arg:?} is neither a number of days nor a study \
                 (SMA50, EMA20, BB20, MOM10, ROC10, RSI14, MACD12,26,9)"
            ));
        }
    }
    Ok(Box::new(SecurityChart {
        security,
        days: days.unwrap_or(0),
        picker: SecurityPicker::default(),
        own_studies: studies.is_some(),
        studies: studies.unwrap_or_default(),
    }))
}

struct SecurityChart {
    security: Security,
    /// 0: the latest session at one minute.
    days: u32,
    picker: SecurityPicker,
    /// Drawn in this order: averages and bands over the price, the rest in
    /// panes below it.
    studies: Vec<Study>,
    /// The studies were chosen for this chart (in its route or the Studies
    /// menu). Otherwise it follows `[markets] studies`.
    own_studies: bool,
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

/// Calendar days to fetch before a window, so studies that need `lookback`
/// earlier bars have them. The lookback is rounded up to a power of two, so
/// editing a period fetches again only now and then. Intraday counts assume
/// half a regular session of bars a day: a thin name has a bar only in a
/// minute it trades.
fn warm_up_days(timeframe: Timeframe, lookback: usize) -> i64 {
    if lookback == 0 {
        return 0;
    }
    let bars = lookback.next_power_of_two().max(32);
    let per_day = match timeframe {
        Timeframe::Day1 => 1,
        Timeframe::Hour1 => 4,
        Timeframe::Min15 => 13,
        Timeframe::Min5 => 39,
        Timeframe::Min1 => 195,
    };
    let trading_days = bars.div_ceil(per_day);
    // Five trading days a week, about ten holidays a year, and a margin.
    (trading_days * 7 / 5 + trading_days / 25 + 4) as i64
}

/// Heights of the price chart and of each study pane, out of `height`, with
/// `gap` between plots; the volume takes what is left. Panes share at most
/// about two fifths, so the price stays the largest.
fn heights(height: f32, panes: usize, gap: f32) -> (f32, f32) {
    if panes == 0 {
        return ((height * 0.74).max(120.0), 0.0);
    }
    let n = panes as f32;
    let pane = (height * 0.42 / n - gap).clamp(56.0, 150.0);
    let volume = (height * 0.1).max(48.0);
    let price = (height - n * (pane + gap) - volume - gap).max(120.0);
    (price, pane)
}

/// A study over the fetched bars, with its colour.
struct Drawn {
    study: Study,
    values: Values,
    color: Color32,
}

impl Drawn {
    /// The study's points at the shown bars' `xs` (bars `range` of the
    /// fetched ones), skipping bars without a value.
    fn line(
        &self,
        xs: &[f64],
        range: &Range<usize>,
        value: impl Fn(usize) -> Option<f64>,
    ) -> Line<'static> {
        let points: Vec<[f64; 2]> = xs
            .iter()
            .zip(range.clone())
            .filter_map(|(x, i)| Some([*x, value(i)?]))
            .filter(|p| p[1].is_finite())
            .collect();
        Line::new(self.study.label(), PlotPoints::from(points)).color(self.color)
    }

    /// The study's value at bar `i`, written for the key above the chart.
    fn reading(&self, i: usize) -> Option<String> {
        Some(match (&self.values, self.study) {
            (Values::Line(v), Study::Sma { .. } | Study::Ema { .. }) => fmt::price((*v.get(i)?)?),
            (Values::Line(v), Study::Momentum { .. }) => format!("{:+.2}", (*v.get(i)?)?),
            (Values::Line(v), Study::RateOfChange { .. }) => format!("{:+.2}%", (*v.get(i)?)?),
            (Values::Line(v), _) => format!("{:.1}", (*v.get(i)?)?),
            (Values::Bands(v), _) => {
                let b = (*v.get(i)?)?;
                format!("{}–{}", fmt::price(b.lower), fmt::price(b.upper))
            }
            (Values::Macd(v), _) => {
                let m = (*v.get(i)?)?;
                match m.signal {
                    Some(s) => format!("{:+.2} · signal {:+.2}", m.macd, s),
                    None => format!("{:+.2}", m.macd),
                }
            }
        })
    }
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
        if self.own_studies {
            if self.studies.is_empty() {
                args.push(NO_STUDIES.into());
            } else {
                args.extend(self.studies.iter().map(Study::token));
            }
        }
        Route::new("GP", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if !self.own_studies {
            self.studies = cx.config.markets.studies();
        }
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
            ui.separator();
            self.studies_menu(ui, cx);
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
        // Studies need bars from before the window to have a value at its start.
        let lookback = self.studies.iter().map(Study::lookback).max().unwrap_or(0);
        let from = start - chrono::Duration::days(warm_up_days(timeframe, lookback));
        let snap = cx
            .hub
            .watch(&cx.alpaca.bars([sym.as_str()], timeframe, from, None));
        let Some(set) = snap.data() else {
            widgets::placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string));
            return;
        };
        let all = set.get(&sym);
        let date_of = |b: &Bar| exchange::to_exchange(b.time).date_naive();
        // The latest session view shows the latest session with any bars;
        // the others, the bars from the window's first day on.
        let (range, session): (Range<usize>, Option<chrono::NaiveDate>) = if self.days == 0 {
            let shown = all
                .iter()
                .rev()
                .map(date_of)
                .find(|d| *d == latest)
                .or_else(|| all.last().map(date_of));
            let first = all.iter().position(|b| Some(date_of(b)) == shown);
            let last = all.iter().rposition(|b| Some(date_of(b)) == shown);
            match (first, last) {
                (Some(f), Some(l)) => (f..l + 1, shown),
                _ => (0..0, shown),
            }
        } else {
            let first = all
                .iter()
                .position(|b| date_of(b) >= start)
                .unwrap_or(all.len());
            (first..all.len(), None)
        };
        let bars = &all[range.clone()];
        let closes: Vec<f64> = all.iter().map(|b| b.close).collect();
        let drawn: Vec<Drawn> = self
            .studies
            .iter()
            .enumerate()
            .map(|(i, study)| Drawn {
                study: *study,
                values: study.values(&closes),
                color: skin.series(STUDY_SERIES[i % STUDY_SERIES.len()]),
            })
            .collect();

        ui.horizontal_wrapped(|ui| {
            let mut note = format!("{} bars · {}", timeframe.label(), set.source.label());
            if let Some(d) = session {
                note += &format!(" · session of {}", d.format("%a %b %d"));
            }
            if set.truncated {
                note += " · more bars than one request returns; the start is cut";
            }
            if !drawn.is_empty() && timeframe != Timeframe::Day1 {
                note += " · studies count bars with trades, not minutes";
            }
            ui.label(RichText::new(note).small().color(skin.text_muted));
            csv::copy_button(ui, skin, || {
                let mut header: Vec<String> =
                    ["time_utc", "open", "high", "low", "close", "volume", "vwap"]
                        .map(String::from)
                        .to_vec();
                for d in &drawn {
                    header.extend(csv_columns(d.study));
                }
                let header: Vec<&str> = header.iter().map(String::as_str).collect();
                csv::to_csv(
                    &header,
                    bars.iter().zip(range.clone()).map(|(b, i)| {
                        let mut row = vec![
                            b.time.to_rfc3339(),
                            b.open.to_string(),
                            b.high.to_string(),
                            b.low.to_string(),
                            b.close.to_string(),
                            b.volume.to_string(),
                            b.vwap.map_or_else(String::new, |v| v.to_string()),
                        ];
                        for d in &drawn {
                            row.extend(csv_values(&d.values, i));
                        }
                        row
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
            && let Some(w) = window(bars)
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
                        &volatility(bars).map_or_else(|| fmt::DASH.into(), |v| format!("{v:.1}%")),
                        Some(RichText::new("annualised, daily").color(skin.text_muted)),
                    );
                }
            });
        }
        if !drawn.is_empty() {
            study_key(ui, skin, &drawn, &range);
        }

        let up = match (bars.first(), bars.last(), self.days) {
            (_, Some(l), 0) => row.prev_close.is_none_or(|p| l.close >= p),
            (Some(f), Some(l), _) => l.close >= f.open,
            _ => true,
        };
        let color = if up { skin.positive } else { skin.negative };
        let secs = timeframe.seconds() as f64;
        // Intraday points sit at the bar's end, daily ones at the day.
        let xs: Vec<f64> = bars
            .iter()
            .map(|b| utc_x(b.time) + if secs < 86_400.0 { secs } else { 0.0 })
            .collect();
        let points: Vec<[f64; 2]> = xs.iter().zip(bars).map(|(x, b)| [*x, b.close]).collect();
        // Bars (volume, MACD's histogram) sit at the middle of the bar.
        let centres: Vec<f64> = bars
            .iter()
            .map(|b| utc_x(b.time) + if secs < 86_400.0 { secs / 2.0 } else { 0.0 })
            .collect();
        let bar_width = secs.min(86_400.0) * 0.8;
        let id = format!("gp-sec-{sym}");
        let link = egui::Id::new(("gp-sec-link", &sym));
        let panes: Vec<&Drawn> = drawn.iter().filter(|d| !d.study.is_overlay()).collect();
        let height = ui.available_height();
        let gap = ui.spacing().item_spacing.y;
        let (price_h, pane_h) = heights(height, panes.len(), gap);
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
                for d in drawn.iter().filter(|d| d.study.is_overlay()) {
                    overlay(plot, d, &xs, &range);
                }
                plot.line(
                    Line::new(sec.as_str(), PlotPoints::from(points))
                        .color(color)
                        .width(1.6),
                );
            });
        for d in &panes {
            let mut plot = exchange_plot(&format!("{id}-{}", d.study.token()), skin)
                .height(pane_h)
                .link_axis(link, [true, false])
                .link_cursor(link, [true, false])
                .show_x(false)
                .show_axes([false, true])
                .legend(
                    Legend::default()
                        .position(Corner::LeftTop)
                        .background_alpha(0.8),
                );
            if matches!(d.study, Study::Rsi { .. }) {
                plot = plot.include_y(0.0).include_y(100.0);
            }
            plot.show(ui, |plot| {
                pane(plot, skin, d, &xs, &centres, bar_width, &range)
            });
        }
        let volume: Vec<PlotBar> = bars
            .iter()
            .zip(&centres)
            .map(|(b, x)| {
                PlotBar::new(*x, b.volume)
                    .width(bar_width)
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

impl SecurityChart {
    /// The Studies menu: the chart's studies with their periods, the presets
    /// to add, and the default for new charts.
    fn studies_menu(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let label = match self.studies.len() {
            0 => "Studies".to_owned(),
            n => format!("Studies · {n}"),
        };
        // Stay open while periods are edited and studies added.
        let config = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
        let (button, _) = MenuButton::new(label).config(config).ui(ui, |ui| {
            let before = self.studies.clone();
            let mut follow = false;
            if self.studies.is_empty() {
                ui.label(RichText::new("No studies on this chart.").color(skin.text_muted));
            }
            let mut remove = None;
            egui::Grid::new("gp-sec-studies")
                .num_columns(3)
                .show(ui, |ui| {
                    for (i, study) in self.studies.iter_mut().enumerate() {
                        ui.label(study.name());
                        ui.horizontal(|ui| edit_periods(ui, study));
                        if ui.small_button("✕").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = remove {
                self.studies.remove(i);
            }
            for (heading, overlays) in [("Over the price", true), ("Below the price", false)] {
                ui.add_space(4.0);
                widgets::label(ui, skin, heading);
                ui.horizontal_wrapped(|ui| {
                    for preset in Study::PRESETS.iter().filter(|p| p.is_overlay() == overlays) {
                        let fresh = !self.studies.contains(preset);
                        if ui
                            .add_enabled(fresh, egui::Button::new(format!("+ {}", preset.label())))
                            .clicked()
                        {
                            self.studies.push(*preset);
                        }
                    }
                });
            }
            ui.separator();
            let defaults = cx.config.markets.studies();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.studies != defaults,
                        egui::Button::new("Use for new charts"),
                    )
                    .on_hover_text(
                        "Save these studies as [markets] studies in config.toml: new GP \
                         charts of securities start with them",
                    )
                    .clicked()
                {
                    let mut config = cx.config.clone();
                    config.markets.studies = self.studies.iter().map(Study::token).collect();
                    cx.send(AppCommand::ReplaceConfig(Box::new(config)));
                }
                if ui
                    .add_enabled(self.own_studies, egui::Button::new("Default"))
                    .on_hover_text("Show the studies new charts start with ([markets] studies)")
                    .clicked()
                {
                    follow = true;
                }
                if ui
                    .add_enabled(!self.studies.is_empty(), egui::Button::new("Clear"))
                    .clicked()
                {
                    self.studies.clear();
                }
            });
            ui.label(
                RichText::new(
                    "Periods count bars: days on daily charts; on intraday charts, \
                     bars with trades, not minutes.",
                )
                .small()
                .color(skin.text_muted),
            );
            if follow {
                self.own_studies = false;
                self.studies = defaults;
            } else if self.studies != before {
                self.own_studies = true;
            }
        });
        button.on_hover_text("Moving averages, Bollinger bands, momentum, RSI and MACD");
    }
}

/// Editors for a study's periods (and a band's width).
fn edit_periods(ui: &mut Ui, study: &mut Study) {
    let period = |ui: &mut Ui, p: &mut usize, hover: &str| {
        ui.add(
            egui::DragValue::new(p)
                .range(1..=studies::MAX_PERIOD)
                .speed(0.2),
        )
        .on_hover_text(hover);
    };
    match study {
        Study::Sma { period: p }
        | Study::Ema { period: p }
        | Study::Momentum { period: p }
        | Study::RateOfChange { period: p }
        | Study::Rsi { period: p } => period(ui, p, "Bars"),
        Study::Bollinger { period: p, width } => {
            period(ui, p, "Bars");
            ui.add(
                egui::DragValue::new(width)
                    .range(0.1..=studies::MAX_WIDTH)
                    .speed(0.05)
                    .fixed_decimals(1)
                    .suffix(" sd"),
            )
            .on_hover_text("Standard deviations above and below the average");
        }
        Study::Macd { fast, slow, signal } => {
            period(ui, fast, "Fast average, bars");
            period(ui, slow, "Slow average, bars");
            period(ui, signal, "Signal line, bars");
        }
    }
}

/// The studies' colours and their values at the last bar shown, and a note
/// on any that start inside the window.
fn study_key(ui: &mut Ui, skin: &Skin, drawn: &[Drawn], range: &Range<usize>) {
    let last = range.end.saturating_sub(1);
    ui.horizontal_wrapped(|ui| {
        for d in drawn {
            ui.label(RichText::new("■").color(d.color));
            ui.label(
                RichText::new(d.study.label())
                    .small()
                    .color(skin.text_muted),
            );
            ui.label(
                RichText::new(d.reading(last).unwrap_or_else(|| fmt::DASH.into()))
                    .small()
                    .color(skin.text),
            );
            ui.add_space(8.0);
        }
    });
    let late: Vec<String> = drawn
        .iter()
        .filter(|d| !d.values.is_defined(range.start))
        .map(|d| d.study.label())
        .collect();
    if !late.is_empty() {
        ui.label(
            RichText::new(format!(
                "{} start{} inside the window: too few earlier bars.",
                late.join(", "),
                if late.len() == 1 { "s" } else { "" }
            ))
            .small()
            .color(skin.warning),
        );
    }
}

/// An average or Bollinger bands over the price.
fn overlay(plot: &mut PlotUi<'_>, d: &Drawn, xs: &[f64], range: &Range<usize>) {
    match &d.values {
        Values::Line(v) => plot.line(d.line(xs, range, |i| v[i]).width(1.2)),
        Values::Bands(v) => {
            plot.line(d.line(xs, range, |i| v[i].map(|b| b.upper)).width(1.0));
            plot.line(
                d.line(xs, range, |i| v[i].map(|b| b.middle))
                    .width(1.0)
                    .style(LineStyle::dashed_loose()),
            );
            plot.line(d.line(xs, range, |i| v[i].map(|b| b.lower)).width(1.0));
        }
        Values::Macd(_) => {}
    }
}

/// A pane below the price: momentum, rate of change, RSI or MACD.
fn pane(
    plot: &mut PlotUi<'_>,
    skin: &Skin,
    d: &Drawn,
    xs: &[f64],
    centres: &[f64],
    bar_width: f64,
    range: &Range<usize>,
) {
    // Guides and the histogram go unnamed, so the legend lists the lines.
    let guide = |plot: &mut PlotUi<'_>, y: f64| {
        plot.hline(
            HLine::new("", y)
                .color(skin.border_strong)
                .style(LineStyle::dashed_dense())
                .width(1.0),
        );
    };
    match &d.values {
        Values::Line(v) => {
            if let Study::Rsi { .. } = d.study {
                guide(plot, 70.0);
                guide(plot, 30.0);
            } else {
                guide(plot, 0.0);
            }
            plot.line(d.line(xs, range, |i| v[i]).width(1.4));
        }
        Values::Macd(v) => {
            guide(plot, 0.0);
            let bars: Vec<PlotBar> = centres
                .iter()
                .zip(range.clone())
                .filter_map(|(x, i)| {
                    let h = v[i]?.histogram()?;
                    let fill = if h >= 0.0 {
                        skin.positive
                    } else {
                        skin.negative
                    };
                    Some(
                        PlotBar::new(*x, h)
                            .width(bar_width)
                            .fill(fill.gamma_multiply(0.6)),
                    )
                })
                .collect();
            plot.bar_chart(BarChart::new("", bars).color(skin.text_muted));
            plot.line(d.line(xs, range, |i| v[i].map(|m| m.macd)).width(1.4));
            let signal: Vec<[f64; 2]> = xs
                .iter()
                .zip(range.clone())
                .filter_map(|(x, i)| Some([*x, v[i]?.signal?]))
                .collect();
            // Dashed, so it stands apart from MACD whatever the theme's colours.
            plot.line(
                Line::new("Signal", PlotPoints::from(signal))
                    .color(skin.text)
                    .style(LineStyle::dashed_loose())
                    .width(1.0),
            );
        }
        Values::Bands(_) => {}
    }
}

/// A study's columns in Copy CSV.
fn csv_columns(study: Study) -> Vec<String> {
    let name = study.token().to_ascii_lowercase().replace(',', "_");
    match study {
        Study::Bollinger { .. } => ["lower", "middle", "upper"]
            .map(|part| format!("{name}_{part}"))
            .to_vec(),
        Study::Macd { .. } => vec![name.clone(), format!("{name}_signal")],
        _ => vec![name],
    }
}

/// A study's values at bar `i`, for [`csv_columns`].
fn csv_values(values: &Values, i: usize) -> Vec<String> {
    let num = |v: Option<f64>| v.map_or_else(String::new, |v| v.to_string());
    match values {
        Values::Line(v) => vec![num(v[i])],
        Values::Bands(v) => {
            let b = v[i];
            vec![
                num(b.map(|b| b.lower)),
                num(b.map(|b| b.middle)),
                num(b.map(|b| b.upper)),
            ]
        }
        Values::Macd(v) => vec![num(v[i].map(|m| m.macd)), num(v[i].and_then(|m| m.signal))],
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

    #[test]
    fn studies_ride_in_the_route() {
        let args = |a: &[&str]| -> Vec<String> { a.iter().map(|s| (*s).to_owned()).collect() };
        let p = open(
            Security::us("XLU"),
            &args(&["XLU US", "rsi", "182", "sma50", "SMA 50", "bb20,2.5"]),
        )
        .unwrap();
        assert_eq!(p.route().to_string(), "GP XLU US 182 RSI14 SMA50 BB20,2.5");
        // A chart whose studies were turned off stays that way.
        let p = open(Security::us("XLU"), &args(&["XLU US", "nostudies"])).unwrap();
        assert_eq!(p.route().to_string(), "GP XLU US NOSTUDIES");
        assert!(open(Security::us("XLU"), &args(&["XLU US", "30", "60"])).is_err());
        assert!(open(Security::us("XLU"), &args(&["XLU US", "SMA0"])).is_err());
    }

    #[test]
    fn warm_up_covers_the_lookback() {
        assert_eq!(warm_up_days(Timeframe::Day1, 0), 0);
        // SMA 200 needs 199 earlier days: 256 trading days is about a year.
        let days = warm_up_days(Timeframe::Day1, 199);
        assert!((365..=380).contains(&days), "{days}");
        // Small edits to a period fetch the same bars.
        assert_eq!(
            warm_up_days(Timeframe::Day1, 140),
            warm_up_days(Timeframe::Day1, 199)
        );
        // MACD 12 26 9 on fifteen-minute bars: 128 bars, ten sessions of 13.
        assert_eq!(warm_up_days(Timeframe::Min15, 114), 18);
        assert_eq!(warm_up_days(Timeframe::Min1, 19), 5);
    }

    #[test]
    fn the_price_stays_the_largest_plot() {
        assert_eq!(heights(600.0, 0, 4.0), (444.0, 0.0));
        for panes in 1..=7 {
            let (price, pane) = heights(600.0, panes, 4.0);
            assert!(
                price > 2.0 * pane && price >= 120.0,
                "{panes}: {price} {pane}"
            );
            assert!((56.0..=150.0).contains(&pane));
        }
        // Too short for everything: the price keeps its minimum.
        assert_eq!(heights(300.0, 4, 4.0).0, 120.0);
    }

    #[test]
    fn csv_columns_match_values() {
        let closes: Vec<f64> = (0..40).map(f64::from).collect();
        for study in Study::PRESETS {
            let values = study.values(&closes);
            assert_eq!(
                csv_columns(study).len(),
                csv_values(&values, 39).len(),
                "{}",
                study.label()
            );
        }
        assert_eq!(
            csv_columns(Study::PRESETS[4]),
            ["bb20_2_lower", "bb20_2_middle", "bb20_2_upper"]
        );
    }
}
