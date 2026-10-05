//! WX: weather at MISO's load centres (one per local resource zone), from the
//! National Weather Service. Temperature drives load.

use egui::{Grid, RichText, Ui};
use mt_core::CityForecast;
use mt_data::Snapshot;
use mt_nws::{City, MISO_CITIES};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::skin::Skin;
use crate::widgets::{self, chart};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "WX",
    aliases: &["WEATHER", "TEMP"],
    name: "Weather",
    category: Category::Grid,
    usage: "WX",
    description: "Current temperature, highs and lows and the hourly outlook for a city in each MISO zone (National Weather Service).",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Wx { selected: None }))
}

struct Wx {
    /// A city to chart in detail; `None` charts every city's temperature.
    selected: Option<usize>,
}

/// Hot and cold temperatures stand out; mild ones read as plain text.
pub(crate) fn temp_color(skin: &Skin, f: f64) -> egui::Color32 {
    if f >= 90.0 {
        skin.negative
    } else if f >= 80.0 {
        skin.warning
    } else if f <= 20.0 {
        skin.info
    } else {
        skin.text
    }
}

pub(crate) fn temp(f: f64) -> String {
    format!("{f:.0}°")
}

/// Every city's forecast snapshot, in `MISO_CITIES` order.
pub(crate) fn forecasts(cx: &PanelCx<'_>) -> Vec<(&'static City, Snapshot<CityForecast>)> {
    MISO_CITIES
        .iter()
        .map(|c| (c, cx.hub.watch(&cx.nws.hourly(c))))
        .collect()
}

impl Panel for Wx {
    fn title(&self) -> String {
        "WX".into()
    }

    fn route(&self) -> Route {
        Route::code("WX")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let all = forecasts(cx);
        let loaded = all.iter().filter(|(_, s)| s.data.is_some()).count();
        widgets::title_bar(ui, skin, "Weather · MISO load centres", |ui| {
            if let Some((_, s)) = all.iter().find(|(_, s)| s.data.is_some()) {
                widgets::freshness(ui, skin, s);
            }
        });
        if loaded == 0 {
            let err = all
                .iter()
                .find_map(|(_, s)| s.error.as_ref().map(ToString::to_string));
            widgets::placeholder(ui, skin, err);
            return;
        }

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            Grid::new("wx-table")
                .striped(true)
                .num_columns(8)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "City",
                        "Zone",
                        "Now",
                        "Today",
                        "Tomorrow",
                        "Dew point",
                        "Wind",
                        "Next 48 h",
                    ] {
                        widgets::label(ui, skin, h);
                    }
                    ui.end_row();
                    for (i, (city, snap)) in all.iter().enumerate() {
                        let selected = self.selected == Some(i);
                        let name = RichText::new(city.label()).color(if selected {
                            skin.accent
                        } else {
                            skin.info
                        });
                        if ui
                            .add(egui::Label::new(name).sense(egui::Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.selected = if selected { None } else { Some(i) };
                        }
                        ui.label(RichText::new(city.zone).color(skin.text_muted));
                        let Some(f) = snap.data() else {
                            let msg = if snap.error.is_some() {
                                "unavailable"
                            } else {
                                "loading…"
                            };
                            ui.label(RichText::new(msg).small().color(skin.text_muted));
                            ui.end_row();
                            continue;
                        };
                        let days = f.daily();
                        match f.current() {
                            Some(h) => {
                                ui.label(
                                    RichText::new(temp(h.temp_f))
                                        .strong()
                                        .color(temp_color(skin, h.temp_f)),
                                );
                            }
                            None => {
                                ui.label("");
                            }
                        }
                        for d in days.iter().take(2) {
                            ui.label(
                                RichText::new(format!("{} / {}", temp(d.high), temp(d.low)))
                                    .color(temp_color(skin, d.high)),
                            );
                        }
                        for _ in days.len().min(2)..2 {
                            ui.label("");
                        }
                        let now = f.current();
                        ui.label(
                            now.and_then(|h| h.dewpoint_f)
                                .map_or_else(String::new, temp),
                        );
                        ui.label(
                            now.and_then(|h| h.wind_mph)
                                .map_or_else(String::new, |w| format!("{w:.0} mph")),
                        );
                        let next: Vec<f32> = f.hours.iter().take(48).map(|h| h.temp_f as f32).collect();
                        widgets::sparkline(ui, &next, skin.series(i), egui::vec2(120.0, 16.0))
                            .on_hover_text(now.map(|h| h.short.clone()).unwrap_or_default());
                        ui.end_row();
                    }
                });

            ui.horizontal(|ui| {
                widgets::label(ui, skin, "Hourly outlook");
                if let Some(i) = self.selected {
                    ui.label(RichText::new(MISO_CITIES[i].label()).color(skin.accent));
                    if ui.small_button("all cities").clicked() {
                        self.selected = None;
                    }
                } else {
                    ui.label(
                        RichText::new("click a city for its detail")
                            .small()
                            .color(skin.text_muted),
                    );
                }
            });
            chart::time_plot("wx-chart", skin).height(260.0).show(ui, |plot| match self.selected {
                Some(i) => {
                    if let Some(f) = all[i].1.data() {
                        let pts = |g: fn(&mt_core::WxHour) -> Option<f64>| -> Vec<_> {
                            f.hours
                                .iter()
                                .filter_map(|h| Some((h.time, g(h)?)))
                                .collect()
                        };
                        plot.line(chart::line(
                            "Temperature °F",
                            &pts(|h| Some(h.temp_f)),
                            skin.series(0),
                        ));
                        plot.line(chart::line(
                            "Dew point °F",
                            &pts(|h| h.dewpoint_f),
                            skin.series(2),
                        ));
                        plot.line(
                            chart::line("Chance of rain %", &pts(|h| h.precip_pct), skin.series(1))
                                .style(egui_plot::LineStyle::dashed_dense()),
                        );
                    }
                }
                None => {
                    for (i, (city, snap)) in all.iter().enumerate() {
                        if let Some(f) = snap.data() {
                            let pts: Vec<_> = f.hours.iter().map(|h| (h.time, h.temp_f)).collect();
                            plot.line(chart::line(city.name, &pts, skin.series(i)));
                        }
                    }
                }
            });
            ui.label(
                RichText::new("Forecasts: National Weather Service (api.weather.gov), refreshed every 30 minutes. Times on the chart are EST.")
                    .small()
                    .color(skin.text_muted),
            );
        });
    }
}
