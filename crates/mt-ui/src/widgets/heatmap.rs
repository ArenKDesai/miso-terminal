//! Hour × day heatmap of hourly values: the shape of prices at a glance
//! (evening peaks, weekend lulls, a congested week).

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use egui::{Align2, RichText, Sense, Stroke, Ui, vec2};

use crate::skin::{Skin, label_style};
use crate::widgets::fmt;
use crate::widgets::scale::Diverging;

/// Group hourly points into days of 24 hour-endings (`None` where missing).
pub fn by_day(points: &[(NaiveDateTime, f64)]) -> BTreeMap<NaiveDate, [Option<f64>; 24]> {
    let mut days: BTreeMap<NaiveDate, [Option<f64>; 24]> = BTreeMap::new();
    for (t, v) in points {
        days.entry(t.date()).or_insert([None; 24])[t.hour() as usize] = Some(*v);
    }
    days
}

/// Draw the heatmap: newest day on top, HE 1-24 across, colour by `scale`.
pub fn hour_day(ui: &mut Ui, skin: &Skin, points: &[(NaiveDateTime, f64)], zero_centred: bool) {
    let days = by_day(points);
    if days.is_empty() {
        ui.label(RichText::new("No hourly data yet.").color(skin.text_muted));
        return;
    }
    let mut values: Vec<f64> = points.iter().map(|p| p.1).collect();
    let scale = Diverging::fit(&mut values, zero_centred);
    ui.horizontal(|ui| scale.legend(ui, skin));

    let label_w = 84.0;
    let header_h = 16.0;
    let avail = ui.available_size();
    // A little slack on the right so the last column never clips.
    let cell_w = ((avail.x - label_w - 8.0) / 24.0).max(8.0);
    // A year of days still fits: rows thin down to a pixel and a half.
    let cell_h = ((avail.y - header_h) / days.len() as f32).clamp(1.5, 26.0);
    let gap = if cell_h >= 4.0 { 1.0 } else { 0.0 };
    // Label every day while they fit, else every n-th.
    let label_every = ((14.0 / cell_h).ceil() as usize).max(1);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(
            label_w + cell_w * 24.0,
            header_h + cell_h * days.len() as f32,
        ),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let font = ui.style().text_styles[&label_style()].clone();
    for he in (1..=24).step_by(3) {
        let x = rect.left() + label_w + cell_w * (he as f32 - 0.5);
        painter.text(
            egui::pos2(x, rect.top()),
            Align2::CENTER_TOP,
            format!("HE{he}"),
            font.clone(),
            skin.text_muted,
        );
    }
    let mut hovered = None;
    for (row, (day, hours)) in days.iter().rev().enumerate() {
        let y = rect.top() + header_h + cell_h * row as f32;
        if row % label_every == 0 {
            painter.text(
                egui::pos2(rect.left(), y + cell_h / 2.0),
                Align2::LEFT_CENTER,
                day.format("%a %b %d").to_string(),
                font.clone(),
                skin.text_muted,
            );
        }
        for (h, v) in hours.iter().enumerate() {
            let cell = egui::Rect::from_min_size(
                egui::pos2(rect.left() + label_w + cell_w * h as f32, y),
                vec2(cell_w - 1.0, cell_h - gap),
            );
            let fill = v.map_or(skin.surface, |v| scale.color(v, skin));
            painter.rect_filled(cell, 0.0, fill);
            if resp.hover_pos().is_some_and(|p| cell.contains(p)) {
                painter.rect_stroke(
                    cell,
                    0.0,
                    Stroke::new(1.5, skin.live),
                    egui::StrokeKind::Inside,
                );
                hovered = Some((*day, h + 1, *v));
            }
        }
    }
    if let Some((day, he, v)) = hovered {
        resp.on_hover_text(format!(
            "{} HE{he}: {}",
            day.format("%a %b %d"),
            fmt::price_opt(v)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_hours_into_days() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        let pts = [
            (d.and_hms_opt(0, 0, 0).unwrap(), 1.0),
            (d.and_hms_opt(23, 0, 0).unwrap(), 24.0),
            (d.succ_opt().unwrap().and_hms_opt(5, 0, 0).unwrap(), 6.0),
        ];
        let days = by_day(&pts);
        assert_eq!(days.len(), 2);
        assert_eq!(days[&d][0], Some(1.0));
        assert_eq!(days[&d][23], Some(24.0));
        assert_eq!(days[&d][1], None);
    }
}
