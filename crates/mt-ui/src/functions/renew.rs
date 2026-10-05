//! RENEW: wind and solar, forecast against actual, today and tomorrow.

use chrono::NaiveDateTime;
use egui::{RichText, Ui};
use mt_core::RenewableHour;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "RENEW",
    aliases: &["WIND", "SOLAR", "WS"],
    name: "Wind & solar",
    category: Category::Grid,
    usage: "RENEW",
    description: "Hourly wind and solar forecast vs actual for today and tomorrow, with forecast error so far.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Renew))
}

struct Renew;

impl Panel for Renew {
    fn title(&self) -> String {
        "RENEW".into()
    }

    fn route(&self) -> Route {
        Route::code("RENEW")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let ren = cx.hub.watch(&cx.miso.renewables());
        let mix = cx.hub.watch(&cx.miso.fuel_mix());
        widgets::title_bar(ui, skin, "Wind & solar", |ui| {
            widgets::freshness(ui, skin, &ren)
        });

        ui.horizontal_wrapped(|ui| {
            if let Some(m) = mix.data() {
                for (cat, mw) in m
                    .fuels
                    .iter()
                    .filter(|(c, _)| matches!(mt_core::fuel_key(c), "wind" | "solar"))
                {
                    widgets::stat_tile(
                        ui,
                        skin,
                        &format!("{cat} now · MW"),
                        &fmt::mw(*mw),
                        Some(
                            RichText::new(fmt::pct(mw / m.total() * 100.0) + " of generation")
                                .color(skin.text_muted),
                        ),
                    );
                }
            }
            if let Some(r) = ren.data() {
                type Pick = fn(&RenewableHour) -> Option<f64>;
                let pairs: [(&str, Pick, Pick); 2] = [
                    ("Wind", |h| h.wind_forecast, |h| h.wind_actual),
                    ("Solar", |h| h.solar_forecast, |h| h.solar_actual),
                ];
                for (name, f, a) in pairs {
                    let errs: Vec<f64> =
                        r.hours.iter().filter_map(|h| Some(a(h)? - f(h)?)).collect();
                    if !errs.is_empty() {
                        let bias = errs.iter().sum::<f64>() / errs.len() as f64;
                        widgets::stat_tile(
                            ui,
                            skin,
                            &format!("{name} actual − fcst"),
                            &fmt::mw_signed(bias),
                            Some(
                                RichText::new(format!("mean over {} hours", errs.len()))
                                    .color(skin.text_muted),
                            ),
                        );
                    }
                }
            }
        });

        widgets::with_data(ui, skin, &ren, |ui, r| {
            let pts = |f: fn(&RenewableHour) -> Option<f64>| -> Vec<(NaiveDateTime, f64)> {
                r.hours
                    .iter()
                    .filter_map(|h| Some((h.start, f(h)?)))
                    .collect()
            };
            let (wind, solar) = (skin.fuel("wind"), skin.fuel("solar"));
            chart::time_plot("renew", skin).show(ui, |plot| {
                chart::forecast_steps(plot, "Wind forecast", &pts(|h| h.wind_forecast), wind);
                chart::hourly_steps(plot, "Wind actual", &pts(|h| h.wind_actual), wind);
                chart::forecast_steps(plot, "Solar forecast", &pts(|h| h.solar_forecast), solar);
                chart::hourly_steps(plot, "Solar actual", &pts(|h| h.solar_actual), solar);
            });
        });
    }
}
