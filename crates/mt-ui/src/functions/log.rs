//! LOG: the data feeds behind every panel. What is being fetched, how fresh it
//! is, what failed and why; live streams and request budgets when a source
//! uses them. The first stop when a panel looks wrong.

use egui::{RichText, ScrollArea, Ui};
use egui_extras::{Column, TableBuilder};
use mt_data::{EntryState, EventLevel, StreamPhase};
use mt_miso::history::Phase;

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "LOG",
    aliases: &["FEEDS", "STATUS", "DEBUG"],
    name: "Data feeds & log",
    category: Category::System,
    usage: "LOG",
    description: "Every data feed with its freshness and last error, the price history's download, recent fetch activity, cache and file locations.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
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
                    RichText::new(format!("MISO Terminal {}", crate::version()))
                        .monospace()
                        .color(skin.text_muted),
                );
                ui.separator();
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

            streams(ui, cx);
            budgets(ui, cx);
            price_history(ui, cx);

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
                    let size = *self.cache_size.get_or_insert_with(|| {
                        cache.size_bytes_outside(&crate::kept_cache_dirs(cache))
                    });
                    ui.label(format!("Report cache: {:.1} MB", size as f64 / 1_048_576.0));
                    if ui
                        .button("Clear cache")
                        .on_hover_text(
                            "Downloaded reports and responses. The five-minute archive, \
                             the price history and saved headlines are kept.",
                        )
                        .clicked()
                    {
                        cx.send(AppCommand::ClearCache);
                        self.cache_size = None;
                    }
                }
            });
        });
    }
}

/// Live connections (WebSocket streams), when any source has one open.
fn streams(ui: &mut Ui, cx: &mut PanelCx<'_>) {
    let (skin, hub) = (cx.skin, cx.hub);
    let status = hub.stream_status();
    if status.is_empty() {
        return;
    }
    widgets::section(ui, skin, "Streams");
    let mut restart = false;
    egui::Grid::new("log-streams")
        .num_columns(7)
        .min_col_width(12.0)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "",
                "Stream",
                "State",
                "Connected",
                "Messages",
                "Topics",
                "Last error",
            ] {
                widgets::label(ui, skin, title);
            }
            ui.end_row();
            for s in &status {
                let (color, state) = match s.phase {
                    StreamPhase::Connected { ready: true } => (skin.live, "live"),
                    StreamPhase::Connected { ready: false } => (skin.info, "logging in"),
                    StreamPhase::Connecting => (skin.info, "connecting"),
                    StreamPhase::Retrying => (skin.warning, "reconnecting"),
                    StreamPhase::Failed => (skin.negative, "refused"),
                    StreamPhase::Idle => (skin.text_muted, "closed"),
                };
                widgets::lamp(ui, color);
                ui.label(RichText::new(&s.label).color(if s.watched {
                    skin.text
                } else {
                    skin.text_muted
                }))
                .on_hover_text(&s.key);
                ui.label(state);
                ui.label(s.connected_since.map_or_else(|| fmt::DASH.into(), fmt::ago))
                    .on_hover_text(format!(
                        "{} connection{} since launch",
                        s.connects,
                        if s.connects == 1 { "" } else { "s" }
                    ));
                ui.label(s.messages.to_string());
                ui.label(s.topics.to_string());
                match &s.error {
                    // Truncated: a long error must not widen the pane.
                    Some(e) => ui
                        .add(egui::Label::new(RichText::new(e).color(skin.negative)).truncate())
                        .on_hover_text(e),
                    None => ui.label(""),
                };
                ui.end_row();
            }
        });
    if status
        .iter()
        .any(|s| matches!(s.phase, StreamPhase::Failed | StreamPhase::Retrying))
    {
        restart = ui
            .button("Reconnect now")
            .on_hover_text("After fixing keys in SET, or to skip a retry wait")
            .clicked();
    }
    if restart {
        hub.restart_streams();
    }
}

/// The price history's download: what it is doing, what is stored, and the
/// days it could not get.
fn price_history(ui: &mut Ui, cx: &mut PanelCx<'_>) {
    let skin = cx.skin;
    let s = cx.backfill.status();
    if s.phase == Phase::Unavailable {
        return;
    }
    widgets::section(ui, skin, "Price history");
    let (text, color) = crate::history::activity(&s, skin);
    ui.label(RichText::new(text).color(color));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("Stored: {}", crate::history::stored(&s))).color(skin.text_muted),
        );
        if ui.small_button("Settings").clicked() {
            cx.open(Route::code("SET"));
        }
    });
    if !s.not_published.is_empty() {
        let mut days: Vec<String> = s
            .not_published
            .iter()
            .take(8)
            .map(ToString::to_string)
            .collect();
        if s.not_published.len() > days.len() {
            days.push(format!("and {} more", s.not_published.len() - days.len()));
        }
        ui.label(
            RichText::new(format!(
                "Not published by MISO (yet), asked again within the hour: {}",
                days.join(", ")
            ))
            .color(skin.text_muted),
        );
    }
    for (job, error) in &s.unreadable {
        ui.label(RichText::new(format!("⚠ Could not read {job}: {error}")).color(skin.warning));
    }
    if s.pruned > 0
        && let Some(first) = s.first_day
    {
        ui.label(
            RichText::new(format!(
                "Removed {} reports from before {first} since launch.",
                s.pruned
            ))
            .color(skin.text_muted),
        );
    }
}

/// Request budgets (requests per minute a source allows), when any are set.
fn budgets(ui: &mut Ui, cx: &mut PanelCx<'_>) {
    let skin = cx.skin;
    let status = cx.hub.ctx().budget_status();
    if status.is_empty() {
        return;
    }
    widgets::section(ui, skin, "Request budgets");
    for b in &status {
        let mut text = format!(
            "{}: {} of {} requests in the last {} s",
            b.name,
            b.used,
            b.max,
            b.per.as_secs()
        );
        if b.waits > 0 {
            text.push_str(&format!("; {} waited for a slot", b.waits));
        }
        let color = if b.used >= b.max {
            skin.warning
        } else {
            skin.text
        };
        ui.label(RichText::new(text).color(color));
        if let Some(d) = b.blocked_for {
            ui.label(
                RichText::new(format!(
                    "⚠ {} asked us to slow down: paused for {} s",
                    b.name,
                    d.as_secs()
                ))
                .color(skin.negative),
            );
        }
    }
}
