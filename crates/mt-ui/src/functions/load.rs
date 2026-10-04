//! LOAD: real-time system load against the medium-term forecast and the
//! day-ahead cleared load.

use chrono::Timelike;
use egui::{RichText, Ui};
use mt_core::time::{hour_ending_start, now_market};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "LOAD",
    aliases: &["DEMAND"],
    name: "System load",
    category: Category::Grid,
    usage: "LOAD",
    description: "Five-minute actual load vs the MTLF forecast and DA cleared load, with forecast error.",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Load))
}

struct Load;

impl Panel for Load {
    fn title(&self) -> String {
        "LOAD".into()
    }

    fn route(&self) -> Route {
        Route::code("LOAD")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let load = cx.hub.watch(&cx.miso.load());
        widgets::title_bar(ui, skin, "System load", |ui| {
            widgets::freshness(ui, skin, &load)
        });
        widgets::with_data(ui, skin, &load, |ui, l| {
            let Some(day) = l.market_day else { return };
            let he_now = u8::try_from(now_market().hour() + 1).unwrap_or(1);
            let at = |v: &[(u8, f64)], he: u8| v.iter().find(|(h, _)| *h == he).map(|(_, mw)| *mw);
            let latest = l.latest().map(|(_, v)| v);
            let forecast_now = at(&l.forecast, he_now);

            ui.horizontal_wrapped(|ui| {
                widgets::stat_tile(
                    ui,
                    skin,
                    "Actual · MW",
                    &fmt::mw_opt(latest),
                    l.latest().map(|(t, _)| {
                        RichText::new(format!("at {} EST", fmt::hm(t))).color(skin.text_muted)
                    }),
                );
                widgets::stat_tile(
                    ui,
                    skin,
                    &format!("Forecast HE{he_now}"),
                    &fmt::mw_opt(forecast_now),
                    None,
                );
                if let (Some(a), Some(f)) = (latest, forecast_now) {
                    let err = a - f;
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Actual − forecast",
                        &fmt::mw_signed(err),
                        Some(RichText::new(fmt::pct(err / f * 100.0)).color(skin.delta(err))),
                    );
                }
                if let Some((he, mw)) = l.forecast_peak() {
                    widgets::stat_tile(
                        ui,
                        skin,
                        "Forecast peak",
                        &fmt::mw(mw),
                        Some(RichText::new(format!("HE{he}")).color(skin.text_muted)),
                    );
                }
                widgets::stat_tile(
                    ui,
                    skin,
                    &format!("DA cleared HE{he_now}"),
                    &fmt::mw_opt(at(&l.da_cleared, he_now)),
                    None,
                );
            });

            let hourly = |v: &[(u8, f64)]| -> Vec<_> {
                v.iter()
                    .map(|(he, mw)| (hour_ending_start(day, *he), *mw))
                    .collect()
            };
            chart::time_plot("load-today", skin).show(ui, |plot| {
                chart::hourly_steps(plot, "DA cleared", &hourly(&l.da_cleared), skin.series(2));
                chart::forecast_steps(plot, "MTLF forecast", &hourly(&l.forecast), skin.series(1));
                plot.line(chart::line(
                    "Actual (5-min)",
                    &l.actual_5min,
                    skin.series(0),
                ));
            });
        });
    }
}
