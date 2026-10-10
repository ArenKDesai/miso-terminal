//! FCST: forecasts. For a node, tomorrow's DA or RT price (or RT − DA) as a
//! fan chart, the median and the 50% and 80% bands of the model that did
//! best on recent days, an hourly table, and a Skill tab with every model's
//! record against repeating a recent day. For a security, where its close
//! may be over the next twenty trading days: a range, not a call.
//!
//! The data and the background training are in `crate::forecast`; the
//! models in `mt-forecast`.

use chrono::{Duration, NaiveDate, NaiveDateTime};
use egui::{Grid, RichText, ScrollArea, Ui};
use egui_plot::{Corner, Legend, Line, LineStyle, PlotPoints, PlotUi, Polygon};
use mt_core::instrument::Security;
use mt_core::time::chart_x;
use mt_forecast::evaluate::Bands;
use mt_forecast::features::{NodeInputs, Target};
use mt_forecast::hourly::{self, hour_start};
use mt_forecast::node::{ModelForecast, NodeForecast};
use mt_forecast::securities::CloseForecast;

use crate::context::PanelCx;
use crate::forecast::{self, NodeForecaster, NodeState, SecurityState};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker};
use crate::skin::Skin;
use crate::widgets::chart::{self, exchange_plot, utc_x};
use crate::widgets::node_picker::NodePicker;
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "FCST",
    aliases: &["FORECAST"],
    name: "Forecast",
    category: Category::Prices,
    usage: "FCST <node|security> [DA|RT|DART] [SKILL]",
    description: "Tomorrow's DA or RT price at a node, or RT − DA, as a fan chart: the median and the 50% and 80% bands of the model that did best on recent days, an hourly table, and a Skill tab with every model's record against repeating a recent day. For a security (FCST XLU US), the range its close may take over the next twenty trading days.",
    takes_node: true,
    takes_security: true,
    takes_option: false,
    open,
};

/// What a node's forecast shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Da,
    Rt,
    /// RT less tomorrow's DA, from the RT forecast once the DA is posted.
    Dart,
}

impl View {
    fn target(self) -> Target {
        match self {
            Self::Da => Target::DayAhead,
            Self::Rt | Self::Dart => Target::RealTime,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Da => "DA",
            Self::Rt => "RT",
            Self::Dart => "DART",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Da => "DA tomorrow",
            Self::Rt => "RT tomorrow",
            Self::Dart => "RT − DA tomorrow",
        }
    }
}

enum Subject {
    Node(String),
    Security(Security),
}

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let mut subject = None;
    let mut view = View::Da;
    let mut skill = false;
    for (i, arg) in args.iter().enumerate() {
        let word = arg.trim().to_ascii_uppercase();
        match word.as_str() {
            "DA" => view = View::Da,
            "RT" => view = View::Rt,
            "DART" => view = View::Dart,
            "SKILL" => skill = true,
            _ if i == 0 => {
                subject = Some(match market::security_of(arg) {
                    Some(s) => Subject::Security(s),
                    None => Subject::Node(word),
                });
            }
            _ => return Err(format!("{arg:?} is not DA, RT, DART or SKILL")),
        }
    }
    Ok(Box::new(Fcst {
        subject,
        view,
        skill,
        model: None,
        nodes: NodePicker::default(),
        securities: SecurityPicker::default(),
        forecaster: NodeForecaster::default(),
    }))
}

struct Fcst {
    subject: Option<Subject>,
    view: View,
    /// The Skill tab rather than the forecast.
    skill: bool,
    /// The model shown; the best one when unset.
    model: Option<String>,
    nodes: NodePicker,
    securities: SecurityPicker,
    forecaster: NodeForecaster,
}

impl Panel for Fcst {
    fn title(&self) -> String {
        match &self.subject {
            Some(Subject::Node(n)) => format!("FCST {n} {}", self.view.word()),
            Some(Subject::Security(s)) => format!("FCST {s}"),
            None => "FCST".into(),
        }
    }

    fn route(&self) -> Route {
        let mut args = Vec::new();
        match &self.subject {
            Some(Subject::Node(n)) => {
                args.push(n.clone());
                if self.view != View::Da {
                    args.push(self.view.word().to_owned());
                }
            }
            Some(Subject::Security(s)) => args.push(s.to_string()),
            None => {}
        }
        if self.skill && self.subject.is_some() {
            args.push("SKILL".into());
        }
        Route::new("FCST", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        ui.horizontal_wrapped(|ui| {
            match &self.subject {
                Some(Subject::Node(n)) => {
                    ui.label(RichText::new(n).heading().color(skin.text_strong));
                    if widgets::link(ui, skin, "GP").clicked() {
                        cx.open(Route::new("GP", [n.clone()]));
                    }
                }
                Some(Subject::Security(s)) => {
                    ui.label(
                        RichText::new(s.to_string())
                            .heading()
                            .color(skin.text_strong),
                    );
                    for code in ["GP", "DES"] {
                        if widgets::link(ui, skin, code).clicked() {
                            cx.open(Route::new(code, [s.to_string()]));
                        }
                    }
                }
                None => {}
            }
            if let Some(n) = self.nodes.show(ui, cx, "fcst-node", "a node…") {
                self.subject = Some(Subject::Node(n));
                self.model = None;
            }
            if let Some(s) = self.securities.show(ui, cx, "fcst-sec", "or a security…") {
                self.subject = Some(Subject::Security(s));
                self.model = None;
            }
        });
        let Some(subject) = &self.subject else {
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Pick a node or a security above, or type FCST MINN.HUB, FCST MINN.HUB RT or \
                     FCST XLU US.",
                )
                .color(skin.text_muted),
            );
            return;
        };
        ui.horizontal_wrapped(|ui| {
            if let Subject::Node(_) = subject {
                for v in [View::Da, View::Rt, View::Dart] {
                    if ui.selectable_label(self.view == v, v.label()).clicked() && self.view != v {
                        self.view = v;
                        self.model = None;
                    }
                }
                ui.separator();
            }
            ui.selectable_value(&mut self.skill, false, "Forecast");
            ui.selectable_value(&mut self.skill, true, "Skill");
        });
        match subject {
            Subject::Node(node) => {
                let node = node.clone();
                let state = self.forecaster.watch(cx, &node, self.view.target());
                node_view(ui, cx, &node, self.view, self.skill, &mut self.model, state);
            }
            Subject::Security(sec) => {
                let state = forecast::security(cx, &sec.ticker);
                if market::needs_keys(ui, cx) {
                    return;
                }
                security_view(ui, cx, sec, self.skill, &mut self.model, state);
            }
        }
    }
}

/// A spinner and a line while the forecast is not ready; `true` when it is.
fn waiting(ui: &mut Ui, skin: &Skin, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).color(skin.text_muted));
    });
}

fn node_view(
    ui: &mut Ui,
    cx: &mut PanelCx<'_>,
    node: &str,
    view: View,
    skill: bool,
    model: &mut Option<String>,
    state: NodeState,
) {
    let skin = cx.skin;
    let (f, inputs) = match state {
        NodeState::Gathering(n) => {
            waiting(
                ui,
                skin,
                &format!(
                    "Gathering prices, gas and forecasts kept as issued ({n} still loading). The \
                     models train on the price history: SET's Price history fills it further \
                     back."
                ),
            );
            return;
        }
        NodeState::Training => {
            waiting(
                ui,
                skin,
                "Training and backtesting the models on a background thread (a few seconds).",
            );
            return;
        }
        NodeState::Failed(e) => {
            ui.label(RichText::new(format!("The forecast failed: {e}")).color(skin.warning));
            return;
        }
        NodeState::Ready(f, inputs) => (f, inputs),
    };
    let available: Vec<&ModelForecast> = f.models.iter().filter(|m| m.hours.is_some()).collect();
    if available.is_empty() {
        ui.label(
            RichText::new(format!(
                "Not enough history at {node} to forecast {}: the models need at least a few \
                 weeks of complete days.",
                view.target().label()
            ))
            .color(skin.warning),
        );
        return;
    }
    if skill {
        skill_table(ui, cx, &f);
        return;
    }
    let chosen = model
        .as_ref()
        .and_then(|name| available.iter().find(|m| &m.name == name).copied())
        .or_else(|| f.chosen())
        .unwrap_or(available[0]);
    ui.horizontal_wrapped(|ui| {
        widgets::label(ui, skin, "Model");
        egui::ComboBox::from_id_salt(("fcst-model", node))
            .selected_text(chosen.name.as_str())
            .show_ui(ui, |ui| {
                for m in &available {
                    let best = f.chosen().is_some_and(|b| b.name == m.name);
                    let label = if best {
                        format!("{} (best)", m.name)
                    } else {
                        m.name.clone()
                    };
                    if ui.selectable_label(chosen.name == m.name, label).clicked() {
                        *model = (!best).then(|| m.name.clone());
                    }
                }
            });
        match f.beats_naive() {
            Some(false) => {
                ui.label(
                    RichText::new(
                        "No model beat repeating a recent day on the backtest, so the best of \
                         those is shown. More stored history helps the learned models.",
                    )
                    .small()
                    .color(skin.warning),
                );
            }
            Some(true) => {
                ui.label(
                    RichText::new(
                        "The best model beats repeating a recent day (Skill has the record).",
                    )
                    .small()
                    .color(skin.text_muted),
                );
            }
            None => {}
        }
    });

    let day = f.day;
    let Some(median) = chosen.hours else {
        return;
    };
    // The DA the spread is measured from, once MISO posts it.
    let da_day = hourly::full_day(&inputs.da, day);
    if view == View::Dart && da_day.is_none() {
        ui.label(
            RichText::new(
                "RT − DA needs tomorrow's DA, which MISO posts in the afternoon. The RT forecast \
                 itself uses it too, so until then it leans on today's DA.",
            )
            .color(skin.text_muted),
        );
        return;
    }
    let shift = |h: usize| match (view, da_day) {
        (View::Dart, Some(da)) => da[h],
        _ => 0.0,
    };
    let point: [f64; 24] = std::array::from_fn(|h| median[h] - shift(h));
    let quantiles: Option<[[f64; 5]; 24]> = chosen
        .bands
        .map(|b: Bands| std::array::from_fn(|h| b.around(median[h]).map(|q| q - shift(h))));
    let actual: Option<[f64; 24]> = match view {
        View::Da => da_day,
        View::Rt | View::Dart => None,
    };

    tiles(ui, skin, &f, chosen, &point, quantiles.as_ref(), view);
    let height = (ui.available_height() * 0.55).max(180.0);
    fan_chart(
        ui,
        skin,
        node,
        view,
        &inputs,
        day,
        &point,
        quantiles.as_ref(),
        actual.as_ref(),
        height,
    );
    hourly_table(
        ui,
        skin,
        &point,
        quantiles.as_ref(),
        actual.as_ref(),
        da_day.as_ref(),
        view,
    );
}

fn tiles(
    ui: &mut Ui,
    skin: &Skin,
    f: &NodeForecast,
    chosen: &ModelForecast,
    point: &[f64; 24],
    quantiles: Option<&[[f64; 5]; 24]>,
    view: View,
) {
    let avg = point.iter().sum::<f64>() / 24.0;
    // On-peak: hours ending 8 to 23 on weekdays (here, every day's 07:00 to 22:00).
    let on_peak = point[7..23].iter().sum::<f64>() / 16.0;
    let (peak_h, peak) = point
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or((0, 0.0), |(h, v)| (h, *v));
    let naive_mae = f.models[..2]
        .iter()
        .filter_map(|m| m.accuracy.map(|a| a.mae))
        .fold(f64::INFINITY, f64::min);
    ui.horizontal_wrapped(|ui| {
        let value = |v: f64| match view {
            View::Dart => fmt::signed(v),
            _ => fmt::price(v),
        };
        widgets::stat_tile(
            ui,
            skin,
            &format!("{} {}", view.label(), f.day.format("%a %b %d")),
            &value(avg),
            Some(
                RichText::new(format!("average; on-peak {}", value(on_peak)))
                    .color(skin.text_muted),
            ),
        );
        widgets::stat_tile(
            ui,
            skin,
            "Peak hour",
            &value(peak),
            Some(RichText::new(format!("hour ending {}", peak_h + 1)).color(skin.text_muted)),
        );
        if let Some(q) = quantiles {
            let lo = q.iter().map(|x| x[0]).sum::<f64>() / 24.0;
            let hi = q.iter().map(|x| x[4]).sum::<f64>() / 24.0;
            widgets::stat_tile(
                ui,
                skin,
                "80% band, average",
                &format!("{}–{}", value(lo), value(hi)),
                chosen.accuracy.and_then(|a| a.coverage80).map(|c| {
                    RichText::new(format!("held {:.0}% in the backtest", c * 100.0))
                        .color(skin.text_muted)
                }),
            );
        }
        if let Some(a) = chosen.accuracy {
            widgets::stat_tile(
                ui,
                skin,
                "Backtest error",
                &format!("{} $/MWh", fmt::price(a.mae)),
                Some(
                    RichText::new(if naive_mae.is_finite() {
                        format!(
                            "mean absolute, {} days; naive {}",
                            a.days,
                            fmt::price(naive_mae)
                        )
                    } else {
                        format!("mean absolute, {} days", a.days)
                    })
                    .color(skin.text_muted),
                ),
            );
        }
    });
}

/// The last three days as they happened, then the forecast day's median with its
/// 50% and 80% bands, hour by hour.
#[allow(clippy::too_many_arguments)]
fn fan_chart(
    ui: &mut Ui,
    skin: &Skin,
    node: &str,
    view: View,
    inputs: &NodeInputs,
    day: NaiveDate,
    point: &[f64; 24],
    quantiles: Option<&[[f64; 5]; 24]>,
    actual: Option<&[f64; 24]>,
    height: f32,
) {
    let from = hour_start(day - Duration::days(3), 0);
    let to = hour_start(day, 0);
    let window = |s: &hourly::Hourly| -> Vec<(NaiveDateTime, f64)> {
        s.range(from..to).map(|(t, v)| (*t, *v)).collect()
    };
    let history: Vec<(NaiveDateTime, f64)> = match view {
        View::Da => window(&inputs.da),
        View::Rt => window(&inputs.rt),
        View::Dart => window(&inputs.rt)
            .into_iter()
            .filter_map(|(t, rt)| Some((t, rt - inputs.da.get(&t)?)))
            .collect(),
    };
    let hours = |values: &[f64; 24]| -> Vec<(NaiveDateTime, f64)> {
        (0..24).map(|h| (hour_start(day, h), values[h])).collect()
    };
    let color = skin.series(if view == View::Da { 1 } else { 0 });
    chart::time_plot(&format!("fcst-{node}-{}", view.word()), skin)
        .height(height)
        .legend(
            Legend::default()
                .position(Corner::LeftTop)
                .background_alpha(0.8)
                .follow_insertion_order(true),
        )
        .show(ui, |plot| {
            chart::hourly_steps(plot, "Last three days", &history, skin.text_muted);
            if let Some(q) = quantiles {
                band(plot, "80% band", day, q, (0, 4), color.gamma_multiply(0.18));
                band(plot, "50% band", day, q, (1, 3), color.gamma_multiply(0.35));
            }
            chart::hourly_steps(plot, "Forecast (median)", &hours(point), color);
            if let Some(a) = actual {
                chart::forecast_steps(plot, "DA as posted", &hours(a), skin.text_strong);
            }
        });
}

/// Rectangles between two quantiles, one per hour.
fn band(
    plot: &mut PlotUi<'_>,
    name: &str,
    day: NaiveDate,
    q: &[[f64; 5]; 24],
    (lo, hi): (usize, usize),
    fill: egui::Color32,
) {
    for (h, row) in q.iter().enumerate() {
        let (x0, x1) = (chart_x(hour_start(day, h)), chart_x(hour_start(day, h + 1)));
        let (y0, y1) = (row[lo], row[hi]);
        if ![x0, x1, y0, y1].iter().all(|v| v.is_finite()) {
            continue;
        }
        plot.polygon(
            Polygon::new(
                name,
                PlotPoints::from(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]),
            )
            .fill_color(fill)
            .stroke(egui::Stroke::NONE),
        );
    }
}

fn hourly_table(
    ui: &mut Ui,
    skin: &Skin,
    point: &[f64; 24],
    quantiles: Option<&[[f64; 5]; 24]>,
    actual: Option<&[f64; 24]>,
    da: Option<&[f64; 24]>,
    view: View,
) {
    let value = |v: f64| match view {
        View::Dart => fmt::signed(v),
        _ => fmt::price(v),
    };
    ScrollArea::vertical()
        .id_salt("fcst-hours")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Grid::new("fcst-hourly")
                .striped(true)
                .spacing([20.0, 2.0])
                .show(ui, |ui| {
                    let head = |ui: &mut Ui, t: &str| {
                        ui.label(RichText::new(t).small().strong().color(skin.text_muted));
                    };
                    head(ui, "HE");
                    head(ui, "Median");
                    head(ui, "50% band");
                    head(ui, "80% band");
                    if actual.is_some() {
                        head(ui, "DA as posted");
                    }
                    if view == View::Rt && da.is_some() {
                        head(ui, "DA tomorrow");
                    }
                    ui.end_row();
                    for h in 0..24 {
                        ui.label((h + 1).to_string());
                        ui.label(RichText::new(value(point[h])).monospace());
                        match quantiles {
                            Some(q) => {
                                ui.label(
                                    RichText::new(format!(
                                        "{} – {}",
                                        value(q[h][1]),
                                        value(q[h][3])
                                    ))
                                    .monospace(),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} – {}",
                                        value(q[h][0]),
                                        value(q[h][4])
                                    ))
                                    .monospace(),
                                );
                            }
                            None => {
                                ui.label(fmt::DASH);
                                ui.label(fmt::DASH);
                            }
                        }
                        if let Some(a) = actual {
                            ui.label(RichText::new(value(a[h])).monospace());
                        }
                        if view == View::Rt
                            && let Some(d) = da
                        {
                            ui.label(RichText::new(fmt::price(d[h])).monospace());
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Every model's record on the backtest, the chosen one marked.
fn skill_table(ui: &mut Ui, cx: &PanelCx<'_>, f: &NodeForecast) {
    let skin = cx.skin;
    let days = f
        .models
        .iter()
        .map(|m| m.record.days.len())
        .max()
        .unwrap_or(0);
    ui.label(
        RichText::new(format!(
            "Each model forecast each of the last {days} days from what was known at the time \
             (the learned ones refitted weekly). Errors are $/MWh; the bands' coverage is out of \
             sample, so 80% should hold about 80%. The first two models repeat a recent day: \
             the bar every other model must clear."
        ))
        .small()
        .color(skin.text_muted),
    );
    let naive = f.models[..2]
        .iter()
        .filter_map(|m| m.accuracy.map(|a| a.mae))
        .fold(f64::INFINITY, f64::min);
    Grid::new("fcst-skill")
        .striped(true)
        .spacing([20.0, 4.0])
        .show(ui, |ui| {
            for t in [
                "Model", "MAE", "RMSE", "Pinball", "50% held", "80% held", "Days", "vs naive",
            ] {
                ui.label(RichText::new(t).small().strong().color(skin.text_muted));
            }
            ui.end_row();
            let pct = |v: Option<f64>| {
                v.map_or_else(|| fmt::DASH.to_owned(), |v| format!("{:.0}%", v * 100.0))
            };
            for (i, m) in f.models.iter().enumerate() {
                let best = f.chosen().is_some_and(|b| b.name == m.name);
                let name = RichText::new(if best {
                    format!("★ {}", m.name)
                } else {
                    m.name.clone()
                });
                ui.label(if best {
                    name.color(skin.text_strong)
                } else {
                    name
                });
                match m.accuracy {
                    Some(a) => {
                        ui.label(RichText::new(fmt::price(a.mae)).monospace());
                        ui.label(RichText::new(fmt::price(a.rmse)).monospace());
                        ui.label(
                            RichText::new(
                                a.pinball.map_or_else(|| fmt::DASH.to_owned(), fmt::price),
                            )
                            .monospace(),
                        );
                        ui.label(RichText::new(pct(a.coverage50)).monospace());
                        ui.label(RichText::new(pct(a.coverage80)).monospace());
                        ui.label(RichText::new(a.days.to_string()).monospace());
                        // The two naive models are the bar itself.
                        let vs = if i >= 2 && naive.is_finite() && naive > 0.0 {
                            let d = (a.mae / naive - 1.0) * 100.0;
                            RichText::new(format!("{d:+.0}%"))
                                .monospace()
                                .color(if d < 0.0 {
                                    skin.positive
                                } else {
                                    skin.negative
                                })
                        } else {
                            RichText::new(fmt::DASH)
                        };
                        ui.label(vs);
                    }
                    None => {
                        ui.label(
                            RichText::new(if m.hours.is_none() {
                                "not enough history"
                            } else {
                                "no backtest"
                            })
                            .color(skin.text_muted),
                        );
                    }
                }
                ui.end_row();
            }
        });
    for m in f.models.iter().filter(|m| !m.features.is_empty()) {
        ui.label(
            RichText::new(format!("{} uses: {}.", m.name, m.features.join(", ")))
                .small()
                .color(skin.text_muted),
        );
    }
}

fn security_view(
    ui: &mut Ui,
    cx: &mut PanelCx<'_>,
    sec: &Security,
    skill: bool,
    model: &mut Option<String>,
    state: SecurityState,
) {
    let skin = cx.skin;
    let (closes, f) = match state {
        SecurityState::Loading(error) => {
            widgets::placeholder(ui, skin, error);
            return;
        }
        SecurityState::Training => {
            waiting(
                ui,
                skin,
                "Fitting and backtesting the models (a few seconds).",
            );
            return;
        }
        SecurityState::Failed(e) => {
            ui.label(RichText::new(format!("The forecast failed: {e}")).color(skin.warning));
            return;
        }
        SecurityState::Ready(c, f) => (c, f),
    };
    if skill {
        security_skill(ui, skin, &f);
        return;
    }
    let offered: Vec<usize> = (0..f.models.len())
        .filter(|&i| f.models[i].offered && f.models[i].path.is_some())
        .collect();
    let Some(default) = f.default else {
        ui.label(
            RichText::new(format!(
                "Not enough daily history for {sec} to forecast (about a year is needed)."
            ))
            .color(skin.warning),
        );
        return;
    };
    let chosen = model
        .as_ref()
        .and_then(|name| offered.iter().copied().find(|&i| f.models[i].name == name))
        .unwrap_or(default);
    let m = &f.models[chosen];
    let Some(path) = &m.path else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        widgets::label(ui, skin, "Model");
        egui::ComboBox::from_id_salt(("fcst-sec-model", &sec.ticker))
            .selected_text(m.name)
            .show_ui(ui, |ui| {
                for &i in &offered {
                    let name = f.models[i].name;
                    let label = if i == default {
                        format!("{name} (best calibrated)")
                    } else {
                        name.to_owned()
                    };
                    if ui.selectable_label(i == chosen, label).clicked() {
                        *model = (i != default).then(|| name.to_owned());
                    }
                }
            });
        ui.label(
            RichText::new(
                "Daily returns are close to unpredictable: this is a range for the close, not a \
                 call on its direction.",
            )
            .small()
            .color(skin.text_muted),
        );
    });
    let last = f.last;
    let at = |h: usize| path.get(h - 1).copied();
    ui.horizontal_wrapped(|ui| {
        widgets::stat_tile(ui, skin, "Last close", &market::fmt::price(last), None);
        for h in [5, 20] {
            if let Some(q) = at(h.min(path.len())) {
                widgets::stat_tile(
                    ui,
                    skin,
                    &format!("In {h} trading days"),
                    &format!("{}–{}", market::fmt::price(q[0]), market::fmt::price(q[4])),
                    Some(
                        RichText::new(format!(
                            "80% band; median {} ({:+.1}%)",
                            market::fmt::price(q[2]),
                            (q[2] / last - 1.0) * 100.0
                        ))
                        .color(skin.text_muted),
                    ),
                );
            }
        }
        if let Some(a) = m.accuracy {
            widgets::stat_tile(
                ui,
                skin,
                "80% band held",
                &format!("{:.0}%", a.coverage80 * 100.0),
                Some(RichText::new(format!("{} past starts", a.origins)).color(skin.text_muted)),
            );
        }
    });
    // The next trading days: the market calendar where it reaches, then weekdays.
    let last_day = closes
        .last()
        .map_or_else(mt_core::time::market_today, |c| c.0);
    let calendar = market::calendar(cx.hub, cx.alpaca);
    let mut days: Vec<NaiveDate> = calendar
        .iter()
        .map(|d| d.date)
        .filter(|d| *d > last_day)
        .take(path.len())
        .collect();
    let mut d = days.last().copied().unwrap_or(last_day);
    while days.len() < path.len() {
        d += Duration::days(1);
        if !matches!(
            chrono::Datelike::weekday(&d),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        ) {
            days.push(d);
        }
    }
    let x = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .and_then(mt_core::exchange::exchange_to_utc)
            .map(utc_x)
    };
    let history: Vec<[f64; 2]> = closes
        .iter()
        .rev()
        .take(126)
        .rev()
        .filter_map(|(d, c)| Some([x(*d)?, *c]))
        .collect();
    let color = skin.series(0);
    exchange_plot(&format!("fcst-sec-{}", sec.ticker), skin)
        .height((ui.available_height() - 8.0).max(160.0))
        .legend(
            Legend::default()
                .position(Corner::LeftTop)
                .background_alpha(0.8)
                .follow_insertion_order(true),
        )
        .show(ui, |plot| {
            plot.line(
                Line::new("Close", PlotPoints::from(history.clone()))
                    .color(skin.text_muted)
                    .width(1.4),
            );
            let start = history.last().copied();
            let xs: Vec<f64> = days.iter().filter_map(|d| x(*d)).collect();
            for ((name, (lo, hi)), alpha) in [("80% band", (0, 4)), ("50% band", (1, 3))]
                .into_iter()
                .zip([0.18, 0.35])
            {
                let mut prev = start;
                for (q, xi) in path.iter().zip(&xs) {
                    if let Some([x0, _]) = prev {
                        plot.polygon(
                            Polygon::new(
                                name,
                                PlotPoints::from(vec![
                                    [x0, q[lo]],
                                    [*xi, q[lo]],
                                    [*xi, q[hi]],
                                    [x0, q[hi]],
                                ]),
                            )
                            .fill_color(color.gamma_multiply(alpha))
                            .stroke(egui::Stroke::NONE),
                        );
                    }
                    prev = Some([*xi, q[2]]);
                }
            }
            let median: Vec<[f64; 2]> = start
                .into_iter()
                .chain(path.iter().zip(&xs).map(|(q, xi)| [*xi, q[2]]))
                .collect();
            plot.line(
                Line::new("Median", PlotPoints::from(median))
                    .color(color)
                    .width(1.6)
                    .style(LineStyle::dashed_dense()),
            );
        });
}

fn security_skill(ui: &mut Ui, skin: &Skin, f: &CloseForecast) {
    ui.label(
        RichText::new(format!(
            "Each model forecast from a start a week apart over the last year, one to {} \
             trading days out; errors are in percent of the price. The random walk is the bar \
             to beat; a learned model is offered only where it beats it.",
            f.horizon
        ))
        .small()
        .color(skin.text_muted),
    );
    Grid::new("fcst-sec-skill")
        .striped(true)
        .spacing([20.0, 4.0])
        .show(ui, |ui| {
            for t in [
                "Model",
                "Median error",
                "Pinball",
                "50% held",
                "80% held",
                "Starts",
                "",
            ] {
                ui.label(RichText::new(t).small().strong().color(skin.text_muted));
            }
            ui.end_row();
            for (i, m) in f.models.iter().enumerate() {
                let best = f.default == Some(i);
                let name = RichText::new(if best {
                    format!("★ {}", m.name)
                } else {
                    m.name.to_owned()
                });
                ui.label(if best {
                    name.color(skin.text_strong)
                } else {
                    name
                });
                match m.accuracy {
                    Some(a) => {
                        ui.label(RichText::new(format!("{:.2}%", a.mae_pct)).monospace());
                        ui.label(RichText::new(format!("{:.2}%", a.pinball_pct)).monospace());
                        ui.label(
                            RichText::new(format!("{:.0}%", a.coverage50 * 100.0)).monospace(),
                        );
                        ui.label(
                            RichText::new(format!("{:.0}%", a.coverage80 * 100.0)).monospace(),
                        );
                        ui.label(RichText::new(a.origins.to_string()).monospace());
                    }
                    None => {
                        for _ in 0..5 {
                            ui.label(fmt::DASH);
                        }
                    }
                }
                ui.label(
                    RichText::new(if m.offered {
                        ""
                    } else {
                        "not offered: no better than the random walk"
                    })
                    .small()
                    .color(skin.text_muted),
                );
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(args: &[&str]) -> Result<String, String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        open(&args).map(|p| p.route().to_string())
    }

    #[test]
    fn routes() {
        assert_eq!(route(&["minn.hub"]).unwrap(), "FCST MINN.HUB");
        assert_eq!(route(&["MINN.HUB", "rt"]).unwrap(), "FCST MINN.HUB RT");
        assert_eq!(
            route(&["MINN.HUB", "DART", "skill"]).unwrap(),
            "FCST MINN.HUB DART SKILL"
        );
        assert_eq!(route(&["XLU US"]).unwrap(), "FCST XLU US");
        assert_eq!(route(&["XLU US", "SKILL"]).unwrap(), "FCST XLU US SKILL");
        assert_eq!(route(&[]).unwrap(), "FCST");
        assert!(route(&["MINN.HUB", "TOMORROW"]).is_err());
    }
}
