//! SET: edit `config.toml` in-app. Changes are drafted, then applied together.

use egui::{Grid, RichText, ScrollArea, Ui};
use mt_miso::MisoEndpoints;

use crate::config::AppConfig;
use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "SET",
    aliases: &["SETTINGS", "CONFIG", "PREFS"],
    name: "Settings",
    category: Category::System,
    usage: "SET",
    description: "Display, price highlighting, data and endpoint settings. Saved to config.toml.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Settings { draft: None }))
}

struct Settings {
    /// Edits not yet applied; `None` means "showing the live config".
    draft: Option<AppConfig>,
}

/// MISO asks that each real-time link be polled at most once a minute; the
/// terminal never goes below this between repeat requests.
const MIN_POLITE_SECS: u64 = 30;

impl Panel for Settings {
    fn title(&self) -> String {
        if self.draft.is_some() {
            "SET •".into()
        } else {
            "SET".into()
        }
    }

    fn route(&self) -> Route {
        Route::code("SET")
    }

    fn absorb(&mut self, _args: &[String]) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let live = cx.config;
        let mut draft = self.draft.take().unwrap_or_else(|| live.clone());

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            widgets::title_bar(ui, skin, "Settings", |_| {});

            widgets::section(ui, skin, "Display");
            Grid::new("set-display")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Zoom");
                    ui.add(
                        egui::Slider::new(&mut draft.ui.zoom, 0.75..=2.0)
                            .step_by(0.05)
                            .suffix("×"),
                    );
                    ui.end_row();
                    ui.label("Highlight prices at or above");
                    ui.add(
                        egui::DragValue::new(&mut draft.ui.price_alert)
                            .speed(5.0)
                            .prefix("$")
                            .range(0.0..=10_000.0),
                    );
                    ui.end_row();
                    ui.label("Flag prices at or above");
                    ui.add(
                        egui::DragValue::new(&mut draft.ui.price_extreme)
                            .speed(10.0)
                            .prefix("$")
                            .range(0.0..=10_000.0),
                    );
                    ui.end_row();
                    ui.label("Local time in the status bar");
                    ui.checkbox(&mut draft.ui.show_local_clock, "");
                    ui.end_row();
                    ui.label("Windows notifications for alerts");
                    ui.checkbox(&mut draft.ui.notify_alerts, "")
                        .on_hover_text("When the terminal is not the active window");
                    ui.end_row();
                });

            widgets::section(ui, skin, "Data");
            Grid::new("set-data")
                .num_columns(3)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Default history (GP, SPRD)");
                    ui.add(
                        egui::DragValue::new(&mut draft.data.history_days)
                            .range(1..=90)
                            .suffix(" days"),
                    );
                    ui.label("");
                    ui.end_row();
                    ui.label("Report cache cap");
                    ui.add(
                        egui::DragValue::new(&mut draft.data.cache_max_mb)
                            .speed(64.0)
                            .range(100..=100_000)
                            .suffix(" MB"),
                    );
                    ui.label(
                        RichText::new("pruned at startup")
                            .small()
                            .color(skin.text_muted),
                    );
                    ui.end_row();
                    ui.label("Five-minute archive");
                    ui.add(
                        egui::DragValue::new(&mut draft.data.archive_days)
                            .range(0..=3650)
                            .suffix(" days"),
                    );
                    ui.label(
                        RichText::new("about 3 MB a day; 0 keeps everything")
                            .small()
                            .color(skin.text_muted),
                    );
                    ui.end_row();
                    ui.label("Simultaneous requests");
                    ui.add(
                        egui::DragValue::new(&mut draft.data.max_concurrent_requests).range(1..=16),
                    );
                    ui.label(
                        RichText::new("applies after restart")
                            .small()
                            .color(skin.text_muted),
                    );
                    ui.end_row();
                    ui.label("Minimum seconds between repeat requests");
                    ui.add(
                        egui::DragValue::new(&mut draft.data.polite_interval_secs)
                            .range(MIN_POLITE_SECS..=600)
                            .suffix(" s"),
                    );
                    ui.label(
                        RichText::new(
                            "MISO asks for at most one poll a minute; applies after restart",
                        )
                        .small()
                        .color(skin.text_muted),
                    );
                    ui.end_row();
                });

            widgets::section(ui, skin, "MISO endpoints");
            Grid::new("set-endpoints")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Real-time API");
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.endpoints.public_api)
                            .desired_width(420.0),
                    );
                    ui.end_row();
                    ui.label("Market reports");
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.endpoints.market_reports)
                            .desired_width(420.0),
                    );
                    ui.end_row();
                });
            if draft.endpoints != MisoEndpoints::default()
                && ui
                    .small_button("Reset endpoints to MISO's defaults")
                    .clicked()
            {
                draft.endpoints = MisoEndpoints::default();
            }

            ui.add_space(10.0);
            let dirty = &draft != live;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(dirty, egui::Button::new("Apply and save"))
                    .clicked()
                {
                    cx.send(AppCommand::ReplaceConfig(Box::new(draft.clone())));
                    draft = live.clone();
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("Discard changes"))
                    .clicked()
                {
                    draft = live.clone();
                }
                if ui.button("Open config folder").clicked() {
                    let dir = cx
                        .paths
                        .config_file
                        .parent()
                        .unwrap_or(&cx.paths.config_file)
                        .to_path_buf();
                    cx.send(AppCommand::RevealPath(dir));
                }
            });
            ui.label(
                RichText::new(format!(
                    "Saved to {}. Themes and alerts have their own functions (THEME, ALRT).",
                    cx.paths.config_file.display()
                ))
                .small()
                .color(skin.text_muted),
            );
        });

        self.draft = (&draft != live).then_some(draft);
    }
}
