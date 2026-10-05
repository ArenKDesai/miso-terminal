//! SET: edit `config.toml` in-app. Changes are drafted, then applied together.
//! API keys are not config: they go straight to the credential store.

use egui::{Grid, RichText, ScrollArea, Ui};
use mt_core::equity::Feed;
use mt_data::Secret;
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
    description: "Display, price highlighting, data, news feed, market data and endpoint settings (saved to config.toml, or reset to the defaults), and API keys (kept in Windows Credential Manager).",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Settings {
        draft: None,
        keys: Keys::default(),
        confirm_reset: false,
    }))
}

struct Settings {
    /// Edits not yet applied; `None` means "showing the live config".
    draft: Option<AppConfig>,
    keys: Keys,
    /// *Reset to defaults…* was pressed and awaits confirmation.
    confirm_reset: bool,
}

/// An API key the terminal can use, by its name in the credential store.
struct Credential {
    name: &'static str,
    label: &'static str,
}

/// Alpaca paper-trading keys. Live keys get their own entries, and only once
/// live trading exists; they are never shown here before then.
const CREDENTIALS: &[Credential] = &[
    Credential {
        name: "alpaca/paper/key-id",
        label: "Alpaca paper key ID",
    },
    Credential {
        name: "alpaca/paper/secret-key",
        label: "Alpaca paper secret key",
    },
];

/// The credentials section's state. Whether each key is stored is read from
/// the store once, not every frame; typed values live only until saved.
#[derive(Default)]
struct Keys {
    stored: Option<Vec<bool>>,
    typed: Vec<String>,
    message: Option<(String, bool)>,
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

            news(ui, cx, &mut draft);

            markets(ui, cx, &mut draft);

            credentials(ui, cx, &mut self.keys);

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
                if !self.confirm_reset
                    && ui
                        .button("Reset to defaults…")
                        .on_hover_text("Start again from the default config.toml")
                        .clicked()
                {
                    self.confirm_reset = true;
                }
            });
            if self.confirm_reset {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Reset every setting to its default? This also clears your watchlist,                          alerts, function keys, theme choice, and your own news feeds, topics                          and Q lists. API keys and the layout are kept. The current file is                          saved as config.toml.bak first.",
                    )
                    .color(skin.warning),
                );
                ui.horizontal(|ui| {
                    if ui.button("Reset to defaults").clicked() {
                        cx.send(AppCommand::ResetConfig);
                        // Unapplied edits go too; show the live config next frame.
                        draft = live.clone();
                        self.confirm_reset = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_reset = false;
                    }
                });
            }
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

/// How long headlines are kept, and which feeds are read.
fn news(ui: &mut Ui, cx: &PanelCx<'_>, draft: &mut AppConfig) {
    let skin = cx.skin;
    widgets::section(ui, skin, "News feeds");
    ui.horizontal(|ui| {
        ui.label("Keep headlines for");
        ui.add(
            egui::DragValue::new(&mut draft.news.keep_days)
                .range(0..=365)
                .suffix(" days"),
        );
        ui.label(
            RichText::new("for NEWS search across restarts; 0 keeps them all")
                .small()
                .color(skin.text_muted),
        );
    });
    let feeds = draft.news.all_feeds();
    Grid::new("set-news")
        .num_columns(3)
        .striped(true)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            for f in &feeds {
                let mut on = draft.news.is_enabled(&f.id);
                let name = if f.section.is_empty() {
                    f.source.clone()
                } else {
                    format!("{} · {}", f.source, f.section)
                };
                if ui.checkbox(&mut on, name).changed() {
                    let disabled = &mut draft.news.disabled_feeds;
                    disabled.retain(|d| !d.eq_ignore_ascii_case(&f.id));
                    if !on {
                        disabled.push(f.id.clone());
                    }
                }
                ui.label(
                    RichText::new(if f.top { "top stories" } else { "" })
                        .small()
                        .color(skin.text_muted),
                );
                ui.label(RichText::new(&f.url).small().color(skin.text_muted));
                ui.end_row();
            }
        });
    ui.label(
        RichText::new(
            "Add RSS or Atom feeds under [[news.feeds]] (id, source, section, url, top) \
             and topics for NI under [[news.topics]] (name, keywords) in config.toml.",
        )
        .small()
        .color(skin.text_muted),
    );
}

/// Where stock and ETF prices come from, and how many symbols stream.
fn markets(ui: &mut Ui, cx: &PanelCx<'_>, draft: &mut AppConfig) {
    let skin = cx.skin;
    widgets::section(ui, skin, "Stocks and ETFs (Alpaca)");
    Grid::new("set-markets")
        .num_columns(3)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label("Live prices");
            egui::ComboBox::from_id_salt("set-feed")
                .selected_text(draft.markets.feed.describe())
                .width(300.0)
                .show_ui(ui, |ui| {
                    for f in Feed::ALL {
                        ui.selectable_value(&mut draft.markets.feed, f, f.describe());
                    }
                });
            ui.label(
                RichText::new("daily history always comes from every exchange")
                    .small()
                    .color(skin.text_muted),
            );
            ui.end_row();
            ui.label("Live trade and quote subscriptions");
            ui.add(egui::DragValue::new(&mut draft.markets.stream_limit).range(1..=10_000));
            ui.label(
                RichText::new(format!(
                    "the free plan allows {} in all: trades first, then quotes; the rest update by the minute",
                    mt_alpaca::config::FREE_PLAN_STREAM_LIMIT
                ))
                .small()
                .color(skin.text_muted),
            );
            ui.end_row();
        });
    ui.label(
        RichText::new(
            "Lists for Q go under [[markets.lists]] (name, title, symbols) in config.toml; \
             a built-in list's name (POWER, UTILITIES, GENERATORS, GAS, ETFS) replaces it.",
        )
        .small()
        .color(skin.text_muted),
    );
}

fn credentials(ui: &mut Ui, cx: &mut PanelCx<'_>, keys: &mut Keys) {
    let skin = cx.skin;
    let store = cx.hub.ctx().secrets().clone();
    widgets::section(ui, skin, "Credentials");
    ui.label(
        RichText::new(format!(
            "Kept in {}, never in config.toml, saved layouts or logs. Alpaca's keys bring \
             stock and ETF prices (Q, GP, DES, CN) and, later, paper trading: sign up at \
             alpaca.markets and create paper-trading keys in its dashboard.",
            store.describe()
        ))
        .small()
        .color(skin.text_muted),
    );
    keys.typed.resize(CREDENTIALS.len(), String::new());
    let stored = keys.stored.get_or_insert_with(|| {
        CREDENTIALS
            .iter()
            .map(|c| matches!(store.get(c.name), Ok(Some(s)) if !s.is_empty()))
            .collect()
    });
    let mut changed = false;
    Grid::new("set-credentials")
        .num_columns(3)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for (i, c) in CREDENTIALS.iter().enumerate() {
                ui.label(c.label);
                if stored[i] {
                    ui.label(RichText::new("● stored").color(skin.live));
                } else {
                    ui.label(RichText::new("not set").color(skin.text_muted));
                }
                // Field and buttons share the last column: a Grid sizes
                // earlier columns from the previous frame, which pins a text
                // field at its first, tiny width.
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut keys.typed[i])
                            .password(true)
                            .hint_text(if stored[i] {
                                "replace…"
                            } else {
                                "paste key…"
                            })
                            .desired_width(260.0),
                    );
                    let typed = !keys.typed[i].trim().is_empty();
                    if ui.add_enabled(typed, egui::Button::new("Save")).clicked() {
                        // Into a Secret, which wipes its copy when dropped.
                        let secret = Secret::new(std::mem::take(&mut keys.typed[i]).trim());
                        keys.message = Some(match store.set(c.name, &secret) {
                            Ok(()) => (format!("Saved {}.", c.label), false),
                            Err(e) => (format!("Could not save {}: {e}", c.label), true),
                        });
                        changed = true;
                    }
                    if stored[i] && ui.button("Remove").clicked() {
                        keys.message = Some(match store.delete(c.name) {
                            Ok(()) => (format!("Removed {}.", c.label), false),
                            Err(e) => (format!("Could not remove {}: {e}", c.label), true),
                        });
                        changed = true;
                    }
                });
                ui.end_row();
            }
        });
    if let Some((text, error)) = &keys.message {
        let color = if *error {
            skin.negative
        } else {
            skin.text_muted
        };
        ui.label(RichText::new(text).small().color(color));
    }
    if changed {
        keys.stored = None;
        // Streams refused with the old keys may try again.
        cx.hub.restart_streams();
        cx.send(AppCommand::CredentialsChanged);
    }
    // Whether Alpaca accepts the keys: the market clock is the cheapest ask.
    if cx.alpaca.is_ready() && cx.hub.ctx().is_live() {
        let clock = cx.hub.watch(&cx.alpaca.clock());
        let (color, text) = match (&clock.error, clock.updated) {
            (Some(e), _) => (skin.negative, format!("Alpaca: {e}")),
            (None, Some(t)) => (
                skin.live,
                format!(
                    "Alpaca: connected (answered {})",
                    crate::widgets::fmt::ago(t)
                ),
            ),
            (None, None) => (skin.text_muted, "Alpaca: checking the keys…".to_owned()),
        };
        ui.horizontal(|ui| {
            widgets::lamp(ui, color);
            ui.label(RichText::new(text).small().color(skin.text_muted));
            if ui.small_button("Check again").clicked() {
                cx.hub.refresh(&cx.alpaca.clock());
            }
        });
    }
}
