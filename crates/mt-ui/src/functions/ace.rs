//! ACE: area control error, MISO's real-time balance between generation and load.

use egui::{RichText, Ui};
use egui_plot::HLine;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series;
use crate::widgets::{self, chart, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ACE",
    aliases: &["BALANCE"],
    name: "Area control error",
    category: Category::Grid,
    usage: "ACE",
    description: "MISO's area control error over the last two hours (30-second): how far generation is from balancing load.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(AcePanel))
}

struct AcePanel;

impl Panel for AcePanel {
    fn title(&self) -> String {
        "ACE".into()
    }

    fn route(&self) -> Route {
        Route::code("ACE")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let snap = cx.hub.watch(&cx.miso.ace());
        widgets::title_bar(ui, skin, "Area control error · MW", |ui| {
            widgets::freshness(ui, skin, &snap);
        });
        ui.label(
            RichText::new(
                "Positive = over-generating relative to load and schedules; negative = under.",
            )
            .small()
            .color(skin.text_muted),
        );
        widgets::with_data(ui, skin, &snap, |ui, a| {
            let Some(&(t, now)) = a.points.last() else {
                ui.label(RichText::new("No ACE samples yet.").color(skin.text_muted));
                return;
            };
            ui.horizontal_wrapped(|ui| {
                widgets::stat_tile(
                    ui,
                    skin,
                    "ACE now",
                    &fmt::mw_signed(now),
                    Some(
                        RichText::new(format!("at {} EST", t.format("%H:%M:%S")))
                            .color(skin.text_muted),
                    ),
                );
                let abs_mean = series::mean(a.points.iter().map(|p| p.1.abs()));
                widgets::stat_tile(ui, skin, "Mean |ACE|", &fmt::mw_opt(abs_mean), None);
                let (lo, hi) = a
                    .points
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                        (lo.min(p.1), hi.max(p.1))
                    });
                widgets::stat_tile(
                    ui,
                    skin,
                    "Range",
                    &fmt::mw_signed(hi),
                    Some(
                        RichText::new(format!("min {}", fmt::mw_signed(lo))).color(skin.text_muted),
                    ),
                );
            });
            chart::time_plot("ace", skin).show(ui, |plot| {
                plot.hline(HLine::new("", 0.0).color(skin.border_strong).width(1.0));
                plot.line(chart::line("ACE", &a.points, skin.series(0)).fill(0.0));
            });
        });
    }
}
