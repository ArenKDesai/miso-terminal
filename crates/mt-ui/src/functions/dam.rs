//! DAM: the day-ahead strip. Hourly DA ex-post prices at the eight trading
//! hubs for one market day, with on-peak, off-peak and around-the-clock
//! averages, optionally as the change from the day before. Until MISO posts
//! tomorrow's results, tomorrow shows FCST's forecast instead.

use chrono::{Datelike, Duration, NaiveDate};
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::time::market_today;
use mt_core::{DayLmpReport, DayReportKind, TRADING_HUBS, hub_short};

use crate::context::PanelCx;
use crate::forecast::{NodeForecaster, NodeState};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series::{self, Component};
use crate::widgets::scale::Diverging;
use crate::widgets::table::num_cell;
use crate::widgets::{self, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "DAM",
    aliases: &["DA", "STRIP", "DASTRIP"],
    name: "Day-ahead strip",
    category: Category::Prices,
    usage: "DAM [TODAY|TOMORROW|YESTERDAY]",
    description: "Hourly day-ahead prices at the eight hubs for one day, with on-peak, off-peak and all-hours averages, or the change from the day before.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let offset = match args.first().map(|a| a.to_ascii_uppercase()).as_deref() {
        None => None,
        Some("TODAY") => Some(0),
        Some("TOMORROW") => Some(1),
        Some("YESTERDAY") => Some(-1),
        Some(other) => {
            return Err(format!(
                "unknown day {other}; use TODAY, TOMORROW or YESTERDAY"
            ));
        }
    };
    Ok(Box::new(Dam {
        offset,
        component: Component::Lmp,
        change: false,
        forecasters: TRADING_HUBS
            .iter()
            .map(|_| NodeForecaster::default())
            .collect(),
    }))
}

struct Dam {
    /// Days from today; `None` shows tomorrow once it is published, else today.
    offset: Option<i64>,
    component: Component,
    /// Show the change from the previous day's DA instead of the level.
    change: bool,
    /// Tomorrow's forecast at each hub, until MISO posts the results.
    forecasters: Vec<NodeForecaster>,
}

/// One hub's 24 hours for the chosen day (and the change from the day before).
pub(crate) struct HubDay {
    pub(crate) hub: &'static str,
    pub(crate) hours: [Option<f64>; 24],
}

pub(crate) fn hub_days(report: Option<&DayLmpReport>, component: Component) -> Vec<HubDay> {
    TRADING_HUBS
        .iter()
        .map(|hub| HubDay {
            hub,
            hours: report
                .and_then(|r| r.node(hub))
                .map(|row| {
                    component
                        .hourly(row)
                        .map(|v| v.is_finite().then_some(f64::from(v)))
                })
                .unwrap_or([None; 24]),
        })
        .collect()
}

fn on_peak(day: NaiveDate, he_index: usize) -> bool {
    day.and_hms_opt(he_index as u32, 0, 0)
        .is_some_and(series::is_on_peak)
}

/// On-peak, off-peak and all-hours averages of one hub's day.
pub(crate) fn blocks(day: NaiveDate, hours: &[Option<f64>; 24]) -> [Option<f64>; 3] {
    let pick = |f: &dyn Fn(usize) -> bool| {
        series::mean(
            hours
                .iter()
                .enumerate()
                .filter(|(h, _)| f(*h))
                .filter_map(|(_, v)| *v),
        )
    };
    [
        pick(&|h| on_peak(day, h)),
        pick(&|h| !on_peak(day, h)),
        pick(&|_| true),
    ]
}

impl Dam {
    /// Each hub's forecast of tomorrow's DA LMP (the median of its best
    /// model), with a note saying so; `None` while none is ready.
    fn forecast_strip(
        &mut self,
        ui: &mut Ui,
        cx: &PanelCx<'_>,
        day: NaiveDate,
    ) -> Option<Vec<HubDay>> {
        let skin = cx.skin;
        let mut hubs = Vec::new();
        let mut waiting = 0;
        for (hub, f) in TRADING_HUBS.iter().zip(&mut self.forecasters) {
            let hours = match f.watch(cx, hub, mt_forecast::features::Target::DayAhead) {
                NodeState::Ready(f, _) => f
                    .chosen()
                    .and_then(|m| m.hours)
                    .map_or([None; 24], |d| d.map(Some)),
                NodeState::Gathering(_) | NodeState::Training => {
                    waiting += 1;
                    [None; 24]
                }
                NodeState::Failed(_) => [None; 24],
            };
            hubs.push(HubDay { hub, hours });
        }
        ui.horizontal_wrapped(|ui| {
            if waiting > 0 {
                ui.spinner();
            }
            ui.label(
                RichText::new(format!(
                    "Forecast: MISO posts the results for {} at about 13:30 EST. Each hub shows its \
                     best model's median (FCST <hub> has the bands and the record).{}",
                    day.format("%b %-d"),
                    if waiting > 0 {
                        format!(" {waiting} hubs still training.")
                    } else {
                        String::new()
                    }
                ))
                .color(skin.warning),
            );
        });
        if hubs.iter().any(|h| h.hours[0].is_some()) {
            return Some(hubs);
        }
        if waiting == 0 {
            ui.label(
                RichText::new("Not enough price history at the hubs to forecast yet.")
                    .color(skin.text_muted),
            );
        }
        None
    }
}

impl Panel for Dam {
    fn title(&self) -> String {
        match self.offset {
            Some(1) => "DAM tomorrow".into(),
            Some(-1) => "DAM yesterday".into(),
            _ => "DAM".into(),
        }
    }

    fn route(&self) -> Route {
        match self.offset {
            None => Route::code("DAM"),
            Some(1) => Route::new("DAM", ["TOMORROW"]),
            Some(-1) => Route::new("DAM", ["YESTERDAY"]),
            Some(_) => Route::new("DAM", ["TODAY"]),
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let today = market_today();
        let report = |cx: &PanelCx<'_>, day| {
            cx.hub
                .watch(&cx.miso.day_report(DayReportKind::DaExPost, day))
        };
        let tomorrow = report(cx, today + Duration::days(1));
        let tomorrow_out = tomorrow.data().is_some_and(Option::is_some);
        let offset = self.offset.unwrap_or(if tomorrow_out { 1 } else { 0 });
        let day = today + Duration::days(offset);
        let snap = report(cx, day);
        let prev = self.change.then(|| report(cx, day - Duration::days(1)));

        widgets::title_bar(
            ui,
            skin,
            &format!("Day-ahead · {}", day.format("%a %b %-d")),
            |ui| widgets::freshness(ui, skin, &snap),
        );
        ui.horizontal_wrapped(|ui| {
            for (label, o) in [("Yesterday", -1), ("Today", 0), ("Tomorrow", 1)] {
                if ui.selectable_label(offset == o, label).clicked() {
                    self.offset = Some(o);
                }
            }
            if self.offset.is_some()
                && ui
                    .small_button("auto")
                    .on_hover_text("Tomorrow once MISO posts it (about 13:30 EST), otherwise today")
                    .clicked()
            {
                self.offset = None;
            }
            ui.separator();
            for c in Component::ALL {
                ui.selectable_value(&mut self.component, c, c.label());
            }
            ui.separator();
            ui.checkbox(&mut self.change, "Change vs day before");
        });

        let Some(data) = snap.data() else {
            widgets::placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string));
            return;
        };
        let mut hubs = match data.as_ref() {
            Some(r) => hub_days(Some(r), self.component),
            // Tomorrow, not yet posted: the forecast, for the LMP.
            None if offset == 1 && self.component == Component::Lmp => {
                let Some(hubs) = self.forecast_strip(ui, cx, day) else {
                    return;
                };
                hubs
            }
            None => {
                ui.label(
                    RichText::new(format!(
                        "MISO has not posted day-ahead results for {} yet (usually about 13:30 EST the day before).{}",
                        day.format("%b %-d"),
                        if offset == 1 {
                            " The LMP view shows the forecast until then."
                        } else {
                            ""
                        }
                    ))
                    .color(skin.text_muted),
                );
                return;
            }
        };
        if let Some(prev) = &prev {
            let before = hub_days(prev.data().and_then(Option::as_ref), self.component);
            for (h, b) in hubs.iter_mut().zip(&before) {
                for (v, p) in h.hours.iter_mut().zip(b.hours) {
                    *v = v.zip(p).map(|(v, p)| v - p);
                }
            }
        }
        let summary: Vec<[Option<f64>; 3]> = hubs.iter().map(|h| blocks(day, &h.hours)).collect();

        let mut values: Vec<f64> = hubs
            .iter()
            .flat_map(|h| h.hours.iter().flatten().copied())
            .collect();
        let levels = !self.change && matches!(self.component, Component::Lmp | Component::Energy);
        let scale = Diverging::fit(&mut values, !levels);
        ui.horizontal_wrapped(|ui| {
            scale.legend(ui, skin);
            csv::copy_button(ui, skin, || {
                let mut headers = vec!["hour_ending"];
                headers.extend(TRADING_HUBS.iter().copied());
                csv::to_csv(
                    &headers,
                    (0..24).map(|h| {
                        std::iter::once(format!("{}", h + 1))
                            .chain(hubs.iter().map(|hub| {
                                hub.hours[h].map_or_else(String::new, |v| format!("{v:.2}"))
                            }))
                            .collect()
                    }),
                )
            });
        });

        let cell = |v: Option<f64>| match v {
            None => RichText::new(fmt::DASH).color(skin.text_muted),
            Some(v) if self.change || !levels => {
                RichText::new(fmt::signed(v)).color(scale.color(v, skin))
            }
            Some(v) => RichText::new(fmt::price(v)).color(scale.color(v, skin)),
        };
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
        let mut open = None;
        egui::ScrollArea::horizontal()
            .id_salt("dam-scroll")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("dam-table")
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(86.0))
                    .columns(Column::initial(88.0).at_least(64.0), hubs.len())
                    .header(row_h + 2.0, |mut h| {
                        h.col(|ui| {
                            widgets::label(ui, skin, "Hour (EST)");
                        });
                        for hub in &hubs {
                            h.col(|ui| {
                                // Right-aligned over the numbers.
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::link(ui, skin, hub_short(hub.hub)).clicked() {
                                            open = Some(hub.hub);
                                        }
                                    },
                                );
                            });
                        }
                    })
                    .body(|mut body| {
                        for (name, i) in [("On-peak", 0), ("Off-peak", 1), ("All hours", 2)] {
                            body.row(row_h, |mut row| {
                                row.col(|ui| {
                                    widgets::label(ui, skin, name);
                                });
                                for s in &summary {
                                    row.col(|ui| num_cell(ui, cell(s[i]).strong()));
                                }
                            });
                        }
                        for h in 0..24 {
                            let peak = on_peak(day, h);
                            body.row(row_h, |mut row| {
                                row.col(|ui| {
                                    let text = format!("HE {:>2} {:02}–{:02}", h + 1, h, h + 1);
                                    ui.label(
                                        RichText::new(text).monospace().small().color(if peak {
                                            skin.text
                                        } else {
                                            skin.text_muted
                                        }),
                                    );
                                });
                                for hub in &hubs {
                                    row.col(|ui| num_cell(ui, cell(hub.hours[h])));
                                }
                            });
                        }
                    });
            });
        let weekend = matches!(day.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        ui.label(
            RichText::new(if weekend {
                "Weekend: every hour is off-peak. Click a hub for its chart."
            } else {
                "On-peak is HE 8 to HE 23, Monday to Friday (brighter hour labels). Click a hub for its chart."
            })
            .small()
            .color(skin.text_muted),
        );
        if let Some(hub) = open {
            cx.open(Route::new("GP", [hub.to_owned()]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_split_on_and_off_peak() {
        let monday = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let mut hours = [Some(10.0); 24];
        for (h, v) in hours.iter_mut().enumerate() {
            if on_peak(monday, h) {
                *v = Some(30.0);
            }
        }
        let [on, off, all] = blocks(monday, &hours);
        assert_eq!(on, Some(30.0));
        assert_eq!(off, Some(10.0));
        assert!((all.unwrap() - (16.0 * 30.0 + 8.0 * 10.0) / 24.0).abs() < 1e-9);
        let sunday = monday - Duration::days(1);
        assert_eq!(
            blocks(sunday, &hours)[0],
            None,
            "no on-peak hours at weekends"
        );
        assert_eq!(
            open(&["tomorrow".into()]).unwrap().route(),
            Route::new("DAM", ["TOMORROW"])
        );
        assert!(open(&["friday".into()]).is_err());
    }
}
