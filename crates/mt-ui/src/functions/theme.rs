//! THEME: pick, preview and start customising themes.

use egui::{Color32, RichText, ScrollArea, Sense, Stroke, Ui};
use mt_theme::{GalleryTheme, Severity, Theme, ThemeRegistry};

use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::gallery::GalleryQuery;
use crate::skin::{Skin, c32};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "THEME",
    aliases: &["THEMES", "SKIN"],
    name: "Themes",
    category: Category::System,
    usage: "THEME [id]",
    description: "Switch themes, preview palettes and contrast checks, install more from the gallery, and copy a theme to edit. THEME <id> switches directly.",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(ThemePanel {
        apply: args.first().cloned(),
        message: None,
        gallery: GalleryQuery::default(),
    }))
}

struct ThemePanel {
    /// A theme id given on the command line, applied on first draw.
    apply: Option<String>,
    message: Option<String>,
    gallery: GalleryQuery,
}

fn swatch(ui: &mut Ui, color: Color32, border: Color32, tip: &str) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.painter().rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    resp.on_hover_text(tip);
}

impl Panel for ThemePanel {
    fn title(&self) -> String {
        "THEME".into()
    }

    fn route(&self) -> Route {
        Route::code("THEME")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        if let Some(id) = self.apply.take() {
            match cx.themes.get(&id) {
                Some(t) => cx.send(AppCommand::SetTheme(t.meta.id.clone())),
                None => self.message = Some(format!("No theme called {id:?}.")),
            }
        }
        let skin = cx.skin;
        let active = skin.theme.meta.id.clone();
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            widgets::title_bar(ui, skin, "Themes", |_| {});
            if let Some(m) = &self.message {
                ui.label(RichText::new(m).color(skin.warning));
            }
            for theme in cx.themes.themes() {
                let selected = theme.meta.id == active;
                let frame = egui::Frame::new()
                    .fill(if selected {
                        skin.surface_alt
                    } else {
                        skin.surface
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if selected { skin.accent } else { skin.border },
                    ))
                    .inner_margin(egui::Margin::same(8));
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&theme.meta.name)
                                    .strong()
                                    .color(skin.text_strong),
                            );
                            ui.label(
                                RichText::new(&theme.meta.id)
                                    .monospace()
                                    .color(skin.text_muted),
                            );
                            if selected {
                                ui.label(RichText::new("active").small().color(skin.accent));
                            }
                        });
                        if !theme.meta.description.is_empty() {
                            ui.label(
                                RichText::new(&theme.meta.description)
                                    .small()
                                    .color(skin.text_muted),
                            );
                        }
                        palette_row(ui, theme, skin.border_strong);
                        if selected {
                            for issue in theme.validate() {
                                let color = if issue.severity == Severity::Error {
                                    skin.negative
                                } else {
                                    skin.warning
                                };
                                ui.label(
                                    RichText::new(format!("contrast: {}", issue.message))
                                        .small()
                                        .color(color),
                                );
                            }
                        }
                    })
                    .response
                    .interact(Sense::click());
                if resp.clicked() && !selected {
                    cx.send(AppCommand::SetTheme(theme.meta.id.clone()));
                }
                ui.add_space(4.0);
            }

            for err in cx.themes.errors() {
                ui.label(
                    RichText::new(format!("⚠ {}: {}", err.path.display(), err.message))
                        .color(skin.negative),
                );
            }

            widgets::section(ui, skin, "Follow Windows light/dark");
            follow_system(ui, cx);

            widgets::section(ui, skin, "Gallery");
            ui.label(
                RichText::new(
                    "More themes, published with the terminal's docs. Install one to add it above.",
                )
                .small()
                .color(skin.text_muted),
            );
            let snap = cx.hub.watch(&self.gallery);
            let mut commands = Vec::new();
            widgets::with_data(ui, skin, &snap, |ui, gallery| {
                for g in &gallery.themes {
                    gallery_card(ui, skin, cx.themes, g, &active, &mut commands);
                    ui.add_space(4.0);
                }
                for skipped in &gallery.skipped {
                    ui.label(
                        RichText::new(format!("Left out: {skipped}"))
                            .small()
                            .color(skin.warning),
                    );
                }
            });
            for c in commands {
                cx.send(c);
            }
            ui.horizontal(|ui| {
                ui.hyperlink_to("The gallery on the web", mt_theme::GALLERY_URL);
                ui.add_space(8.0);
                if ui.button("Open themes folder").clicked() {
                    let _ = std::fs::create_dir_all(&cx.paths.themes_dir);
                    cx.send(AppCommand::RevealPath(cx.paths.themes_dir.clone()));
                }
            });

            widgets::section(ui, skin, "Make your own");
            ui.label(
                "Themes are TOML files in the folder below. A file with an existing id replaces \
                 that theme. Start from a copy of the current one:",
            );
            ui.label(RichText::new(cx.paths.themes_dir.display().to_string()).monospace());
            if ui
                .button(format!("Copy “{}” to edit", skin.theme.meta.name))
                .clicked()
            {
                self.message = Some(copy_theme(&skin.theme, &cx.paths.themes_dir));
            }
        });
    }
}

/// One gallery theme: what it looks like and what installing it would do.
fn gallery_card(
    ui: &mut Ui,
    skin: &Skin,
    installed: &ThemeRegistry,
    g: &GalleryTheme,
    active: &str,
    out: &mut Vec<AppCommand>,
) {
    let meta = &g.theme.meta;
    let install = |activate| AppCommand::InstallTheme {
        theme: Box::new(g.clone()),
        activate,
    };
    egui::Frame::new()
        .fill(skin.surface)
        .stroke(Stroke::new(1.0, skin.border))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&meta.name).strong().color(skin.text_strong));
                ui.label(RichText::new(&meta.id).monospace().color(skin.text_muted));
                let mode = if meta.dark { "dark" } else { "light" };
                ui.label(RichText::new(mode).small().color(skin.text_muted));
                if !meta.author.is_empty() {
                    ui.label(
                        RichText::new(format!("by {}", meta.author))
                            .small()
                            .color(skin.text_muted),
                    );
                }
            });
            if !meta.description.is_empty() {
                ui.label(
                    RichText::new(&meta.description)
                        .small()
                        .color(skin.text_muted),
                );
            }
            palette_row(ui, &g.theme, skin.border_strong);
            ui.horizontal(|ui| match installed.get(&meta.id) {
                None => {
                    if ui.button("Install").clicked() {
                        out.push(install(false));
                    }
                    if ui.button("Install and use").clicked() {
                        out.push(install(true));
                    }
                }
                Some(have) => {
                    if *have == g.theme {
                        ui.label(RichText::new("✓ Installed").color(skin.positive));
                    } else if ui
                        .button("Update")
                        .on_hover_text(format!(
                            "Replace {}.toml in the themes folder with the gallery's version",
                            meta.id
                        ))
                        .clicked()
                    {
                        out.push(install(false));
                    }
                    if meta.id != active && ui.button("Use").clicked() {
                        out.push(AppCommand::SetTheme(meta.id.clone()));
                    }
                    if ui
                        .button("Remove")
                        .on_hover_text(format!("Delete {}.toml from the themes folder", meta.id))
                        .clicked()
                    {
                        out.push(AppCommand::UninstallTheme(meta.id.clone()));
                    }
                }
            });
        });
}

fn palette_row(ui: &mut Ui, t: &Theme, border: Color32) {
    let p = &t.palette;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (name, c) in [
            ("background", p.background),
            ("surface", p.surface),
            ("surface_alt", p.surface_alt),
            ("border", p.border),
            ("border_strong", p.border_strong),
            ("text", p.text),
            ("text_strong", p.text_strong),
            ("text_muted", p.text_muted),
            ("accent", p.accent),
            ("live", p.live),
            ("positive", p.positive),
            ("negative", p.negative),
            ("warning", p.warning),
            ("info", p.info),
        ] {
            swatch(ui, c32(c), border, &format!("{name} {c}"));
        }
        ui.add_space(10.0);
        for (i, c) in t.chart.series.iter().enumerate() {
            swatch(ui, c32(*c), border, &format!("series[{i}] {c}"));
        }
    });
}

/// Write a copy of `theme` with a new id into the user themes folder.
fn copy_theme(theme: &Theme, dir: &std::path::Path) -> String {
    let mut copy = theme.clone();
    copy.meta.id = format!("{}-custom", theme.meta.id);
    copy.meta.name = format!("{} (custom)", theme.meta.name);
    copy.meta.source = format!("copied from {}", theme.meta.id);
    let path = dir.join(format!("{}.toml", copy.meta.id));
    if path.exists() {
        return format!("{} already exists; edit that file.", path.display());
    }
    let result = std::fs::create_dir_all(dir)
        .and_then(|()| copy.to_toml().map_err(std::io::Error::other))
        .and_then(|body| std::fs::write(&path, body));
    match result {
        Ok(()) => format!(
            "Wrote {}. Edit it and it reloads automatically; select it above.",
            path.display()
        ),
        Err(e) => format!("Could not write {}: {e}", path.display()),
    }
}

/// The "switch with Windows" toggle and its light/dark theme pair.
fn follow_system(ui: &mut Ui, cx: &mut PanelCx<'_>) {
    let skin = cx.skin;
    let cfg = &cx.config.ui;
    let (mut follow, mut light, mut dark) = (
        cfg.follow_system_theme,
        cfg.light_theme.clone(),
        cfg.dark_theme.clone(),
    );
    let mut changed = ui
        .checkbox(
            &mut follow,
            "Switch themes with the Windows light/dark setting",
        )
        .changed();
    let name = |id: &str| {
        cx.themes
            .get(id)
            .map_or_else(|| id.to_owned(), |t| t.meta.name.clone())
    };
    ui.horizontal(|ui| {
        for (label, slot, want_dark) in [
            ("Light mode", &mut light, false),
            ("Dark mode", &mut dark, true),
        ] {
            ui.label(label);
            egui::ComboBox::from_id_salt(("theme-follow", want_dark))
                .selected_text(name(slot))
                .show_ui(ui, |ui| {
                    for t in cx
                        .themes
                        .themes()
                        .iter()
                        .filter(|t| t.meta.dark == want_dark)
                    {
                        changed |= ui
                            .selectable_value(slot, t.meta.id.clone(), &t.meta.name)
                            .changed();
                    }
                });
            ui.add_space(12.0);
        }
    });
    if !cx.themes.themes().iter().any(|t| !t.meta.dark) {
        ui.label(
            RichText::new("No light theme is installed. Install one from the gallery below.")
                .small()
                .color(skin.warning),
        );
    }
    let os = match ui.ctx().system_theme() {
        Some(egui::Theme::Dark) => "Windows is in dark mode right now.",
        Some(egui::Theme::Light) => "Windows is in light mode right now.",
        None => "Windows hasn't reported a mode, so the theme above stays put.",
    };
    ui.label(RichText::new(os).small().color(skin.text_muted));
    if changed {
        cx.send(AppCommand::SetThemeFollow {
            follow,
            light,
            dark,
        });
    }
}
