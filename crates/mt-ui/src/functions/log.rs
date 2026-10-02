//! LOG: the data feeds behind every panel. What is being fetched, how fresh it
//! is, what failed and why. The first stop when a panel looks wrong.

use egui::{RichText, ScrollArea, Ui};
use egui_extras::{Column, TableBuilder};
use mt_data::{EntryState, EventLevel};

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "LOG",
    aliases: &["FEEDS", "STATUS", "DEBUG"],
    name: "Data feeds & log",
    category: Category::System,
    usage: "LOG",
    description: "Every data feed with its freshness and last error, recent fetch activity, cache and file locations.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Log { cache_size: None }))
}

struct Log {
    cache_size: Option<u64>,
}

impl Panel for Log {
    fn title(&self) -> String {
        "LOG".into()
    }

    fn route(&self) -> Route {
        Route::code("LOG")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let hub = cx.hub;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Source: {}", hub.ctx().transport_description()))
                        .color(skin.text_muted),
                );
                if ui.button("Refresh open feeds").clicked() {
                    cx.send(AppCommand::RefreshWatched);
                }
                let paused = hub.is_paused();
                if ui
                    .button(if paused {
                        "Resume fetching"
                    } else {
                        "Pause fetching"
                    })
                    .clicked()
                {
                    cx.send(AppCommand::SetPaused(!paused));
                }
            });

            for notice in cx.notices {
                ui.label(RichText::new(format!("⚠ {notice}")).color(skin.warning));
            }

            widgets::section(ui, skin, "Feeds");
            let status = hub.status();
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
            let mut refresh = None;
            TableBuilder::new(ui)
                .id_salt("log-feeds")
                .striped(true)
                .resizable(true)
                .vscroll(false)
                .column(Column::exact(18.0))
                .column(Column::initial(240.0).clip(true))
                .column(Column::initial(90.0))
                .column(Column::initial(70.0))
                .column(Column::initial(70.0))
                .column(Column::remainder().clip(true))
                .header(row_h + 4.0, |mut h| {
                    for title in ["", "Feed", "Updated", "Fetches", "Took", "Last error"] {
                        h.col(|ui| {
                            widgets::label(ui, skin, title);
                        });
                    }
                })
                .body(|mut body| {
                    for s in &status {
                        body.row(row_h, |mut row| {
                            row.col(|ui| {
                                let color = match s.state {
                                    EntryState::Fresh => skin.live,
                                    EntryState::Loading => skin.info,
                                    EntryState::Stale => skin.warning,
                                    EntryState::Error => skin.negative,
                                    EntryState::Empty => skin.text_muted,
                                };
                                widgets::lamp(ui, color).on_hover_text(format!("{:?}", s.state));
                            });
                            row.col(|ui| {
                                let text = RichText::new(&s.label).color(if s.watched {
                                    skin.text
                                } else {
                                    skin.text_muted
                                });
                                if ui
                                    .add(egui::Label::new(text).sense(egui::Sense::click()))
                                    .on_hover_text(format!("{}\nclick to refresh", s.key))
                                    .clicked()
                                {
                                    refresh = Some(s.key.clone());
                                }
                            });
                            row.col(|ui| {
                                ui.label(s.updated.map_or_else(|| fmt::DASH.into(), fmt::ago));
                            });
                            row.col(|ui| {
                                ui.label(s.fetches.to_string());
                            });
                            row.col(|ui| {
                                ui.label(s.last_duration.map_or_else(
                                    || fmt::DASH.into(),
                                    |d| format!("{} ms", d.as_millis()),
                                ));
                            });
                            row.col(|ui| {
                                if let Some(e) = &s.error {
                                    ui.label(RichText::new(e).color(skin.negative))
                                        .on_hover_text(e);
                                }
                            });
                        });
                    }
                });
            if let Some(key) = refresh {
                hub.refresh_key(&key);
            }
            if status.is_empty() {
                ui.label(
                    RichText::new("No feeds yet; open a function to start one.")
                        .color(skin.text_muted),
                );
            }

            widgets::section(ui, skin, "Recent activity");
            for e in hub.ctx().events().recent(80).iter().rev() {
                let color = match e.level {
                    EventLevel::Info => skin.text_muted,
                    EventLevel::Warn => skin.warning,
                    EventLevel::Error => skin.negative,
                };
                let local = e.at.with_timezone(&chrono::Local).format("%H:%M:%S");
                ui.label(
                    RichText::new(format!("{local}  {}", e.message))
                        .monospace()
                        .color(color),
                );
            }

            widgets::section(ui, skin, "Files");
            let p = cx.paths;
            egui::Grid::new("log-paths")
                .num_columns(3)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for (name, path) in [
                        ("Config", p.config_file.parent().unwrap_or(&p.config_file)),
                        ("Themes", p.themes_dir.as_path()),
                        ("Fonts", p.fonts_dir.as_path()),
                        ("Cache", p.cache_dir.as_path()),
                        ("Logs", p.log_dir.as_path()),
                    ] {
                        ui.label(name);
                        ui.label(RichText::new(path.display().to_string()).monospace());
                        if ui.small_button("Open").clicked() {
                            cx.send(AppCommand::RevealPath(path.to_path_buf()));
                        }
                        ui.end_row();
                    }
                });
            ui.horizontal(|ui| {
                if let Some(cache) = hub.ctx().cache() {
                    let size = *self.cache_size.get_or_insert_with(|| cache.size_bytes());
                    ui.label(format!("Report cache: {:.1} MB", size as f64 / 1_048_576.0));
                    if ui.button("Clear cache").clicked() {
                        cx.send(AppCommand::ClearCache);
                        self.cache_size = None;
                    }
                }
            });
        });
    }
}
