//! Reusable building blocks for panels. Panels should look alike because they
//! share these, not because each copies the same styling.

pub mod chart;
pub mod csv;
pub mod fmt;
pub mod heatmap;
pub mod node_picker;
pub mod scale;
pub mod table;

use egui::{Color32, Frame, Response, RichText, Sense, Stroke, Ui};
use mt_data::Snapshot;

use crate::skin::{Skin, label_style, readout_style};

/// A small filled circle: the status lamp.
pub fn lamp(ui: &mut Ui, color: Color32) -> Response {
    let size = ui.text_style_height(&egui::TextStyle::Body) * 0.55;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
    resp
}

/// Uppercase stamped label (column heads, units, section titles).
pub fn label(ui: &mut Ui, skin: &Skin, text: &str) -> Response {
    ui.label(
        RichText::new(text.to_uppercase())
            .text_style(label_style())
            .color(skin.text_muted),
    )
}

/// A section title with a hairline under it.
pub fn section(ui: &mut Ui, skin: &Skin, title: &str) {
    ui.add_space(4.0);
    label(ui, skin, title);
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, skin.border));
    ui.add_space(4.0);
}

/// A panel title row: heading on the left, optional status on the right.
pub fn title_bar(ui: &mut Ui, skin: &Skin, title: &str, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).heading().color(skin.text_strong));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
}

/// Inner width of a stat tile. Fixed so rows of tiles line up and wrap cleanly.
pub const TILE_WIDTH: f32 = 168.0;

/// A plate with a label, a big readout and an optional sub-line. Inside a
/// `horizontal_wrapped` row, a tile that would not fit starts a new row.
pub fn stat_tile(
    ui: &mut Ui,
    skin: &Skin,
    title: &str,
    value: &str,
    sub: Option<RichText>,
) -> Response {
    let (margin_x, margin_y) = (10, 6);
    let outer = TILE_WIDTH + 2.0 * f32::from(margin_x) + 2.0;
    // In a wrapping layout `available_width` is the whole row; this is what is left of it.
    if ui.available_size_before_wrap().x < outer && ui.cursor().left() > ui.max_rect().left() + 1.0
    {
        ui.end_row();
    }
    Frame::new()
        .fill(skin.surface)
        .stroke(Stroke::new(1.0, skin.border))
        .corner_radius(egui::CornerRadius::same(skin.theme.style.rounding as u8))
        .inner_margin(egui::Margin::symmetric(margin_x, margin_y))
        .show(ui, |ui| {
            ui.set_width(TILE_WIDTH);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                label(ui, skin, title);
                ui.label(
                    RichText::new(value)
                        .text_style(readout_style())
                        .color(skin.text_strong),
                );
                if let Some(sub) = sub {
                    ui.label(sub.small());
                }
            });
        })
        .response
}

/// "Updated 16:35:10 EST · 12s ago" with a lamp coloured by freshness.
pub fn freshness<T>(ui: &mut Ui, skin: &Skin, snap: &Snapshot<T>) {
    let (color, text) = match (&snap.error, snap.updated) {
        (Some(e), _) => (skin.negative, format!("Error: {e}")),
        (None, Some(t)) if snap.stale => (skin.warning, format!("Stale · {}", fmt::ago(t))),
        (None, Some(t)) => (skin.live, format!("Updated {}", fmt::ago(t))),
        (None, None) if snap.loading => (skin.text_muted, "Loading…".into()),
        (None, None) => (skin.text_muted, "Waiting".into()),
    };
    ui.label(RichText::new(text).small().color(skin.text_muted));
    lamp(ui, color);
}

/// Draw `body` with the snapshot's data, or an honest placeholder while it
/// loads or after it fails. Keeps every panel's empty/error states consistent.
pub fn with_data<T>(ui: &mut Ui, skin: &Skin, snap: &Snapshot<T>, body: impl FnOnce(&mut Ui, &T)) {
    match snap.data() {
        Some(data) => {
            if let Some(e) = &snap.error {
                ui.label(
                    RichText::new(format!(
                        "⚠ Refresh failed ({e}); showing the last good data."
                    ))
                    .small()
                    .color(skin.warning),
                );
            }
            body(ui, data);
        }
        None => placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string)),
    }
}

pub fn placeholder(ui: &mut Ui, skin: &Skin, error: Option<String>) {
    ui.add_space(12.0);
    ui.horizontal(|ui| match error {
        Some(e) => {
            ui.label(RichText::new(format!("Could not load: {e}")).color(skin.negative));
            ui.label(RichText::new("Retrying automatically.").color(skin.text_muted));
        }
        None => {
            ui.spinner();
            ui.label(RichText::new("Loading…").color(skin.text_muted));
        }
    });
}

/// A tiny line chart with no axes. `NaN` values are skipped.
pub fn sparkline(ui: &mut Ui, values: &[f32], color: Color32, size: egui::Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let finite = values.iter().copied().filter(|v| v.is_finite());
    let (lo, hi) = finite.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if !lo.is_finite() || values.len() < 2 {
        return resp;
    }
    let span = (hi - lo).max(1e-3);
    let n = (values.len() - 1) as f32;
    let pts: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .map(|(i, v)| {
            egui::pos2(
                rect.left() + rect.width() * i as f32 / n,
                rect.bottom() - rect.height() * (v - lo) / span,
            )
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(pts, Stroke::new(1.2, color)));
    resp
}

/// One horizontal bar split into coloured segments, with hover tooltips.
pub fn stacked_bar(ui: &mut Ui, segments: &[(String, f64, Color32)], height: f32) {
    let total: f64 = segments.iter().map(|(_, v, _)| v.max(0.0)).sum();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    if total <= 0.0 {
        return;
    }
    let mut x = rect.left();
    let hover = resp.hover_pos();
    for (name, value, color) in segments {
        let w = rect.width() * (value.max(0.0) / total) as f32;
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, height));
        ui.painter().rect_filled(seg, 0.0, *color);
        if hover.is_some_and(|p| seg.contains(p)) {
            resp.clone().on_hover_text(format!(
                "{name}: {} MW ({})",
                fmt::mw(*value),
                fmt::pct(value / total * 100.0)
            ));
        }
        x += w;
    }
}

/// Clickable text that looks like a link in the accent colour.
pub fn link(ui: &mut Ui, skin: &Skin, text: &str) -> Response {
    ui.add(egui::Label::new(RichText::new(text).color(skin.info)).sense(Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}
