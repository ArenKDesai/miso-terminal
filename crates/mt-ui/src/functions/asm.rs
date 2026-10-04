//! ASM: ancillary-service market clearing prices by reserve zone.

use egui::{Grid, RichText, Ui};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ASM",
    aliases: &["MCP", "RESERVES"],
    name: "Ancillary MCPs",
    category: Category::Prices,
    usage: "ASM",
    description: "Real-time regulation, spinning, supplemental, short-term reserve and ramp MCPs by reserve zone.",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Asm))
}

struct Asm;

impl Panel for Asm {
    fn title(&self) -> String {
        "ASM".into()
    }

    fn route(&self) -> Route {
        Route::code("ASM")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let snap = cx.hub.watch(&cx.miso.ancillary());
        widgets::title_bar(ui, skin, "Ancillary-service MCPs · $/MW", |ui| {
            widgets::freshness(ui, skin, &snap)
        });
        widgets::with_data(ui, skin, &snap, |ui, a| {
            if let Some(t) = a.interval {
                ui.label(
                    RichText::new(format!("Interval {} EST", fmt::day_hm(t)))
                        .small()
                        .color(skin.text_muted),
                );
            }
            Grid::new("asm")
                .striped(true)
                .num_columns(7)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Zone",
                        "Regulation",
                        "Spinning",
                        "Supplemental",
                        "Short-term",
                        "Ramp up",
                        "Ramp down",
                    ] {
                        widgets::label(ui, skin, h);
                    }
                    ui.end_row();
                    for z in &a.zones {
                        ui.label(&z.zone);
                        for v in [
                            z.regulation,
                            z.spinning,
                            z.supplemental,
                            z.short_term,
                            z.ramp_up,
                            z.ramp_down,
                        ] {
                            let color = match v {
                                Some(v) if v > 0.0 => skin.text,
                                _ => skin.text_muted,
                            };
                            ui.label(RichText::new(fmt::price_opt(v)).color(color));
                        }
                        ui.end_row();
                    }
                });
        });
    }
}
