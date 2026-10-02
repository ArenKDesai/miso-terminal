//! Sortable data tables on top of `egui_extras::TableBuilder`.

use std::cmp::Ordering;

use egui::{RichText, Ui};

use crate::skin::{Skin, label_style};

/// Which column a table is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

impl Sort {
    pub const fn new(column: usize, descending: bool) -> Self {
        Self { column, descending }
    }

    /// Apply the direction to an ascending comparison.
    pub fn apply(&self, ord: Ordering) -> Ordering {
        if self.descending { ord.reverse() } else { ord }
    }
}

/// A clickable column header that toggles sorting. Numeric columns sort
/// descending first, since the biggest number is usually the interesting one.
pub fn sort_header(
    ui: &mut Ui,
    skin: &Skin,
    text: &str,
    column: usize,
    numeric: bool,
    sort: &mut Sort,
) {
    let arrow = match (sort.column == column, sort.descending) {
        (true, true) => " ▼",
        (true, false) => " ▲",
        (false, _) => "",
    };
    let color = if sort.column == column {
        skin.text_strong
    } else {
        skin.text_muted
    };
    let resp = ui
        .add(
            egui::Label::new(
                RichText::new(format!("{}{arrow}", text.to_uppercase()))
                    .text_style(label_style())
                    .color(color),
            )
            .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *sort = if sort.column == column {
            Sort::new(column, !sort.descending)
        } else {
            Sort::new(column, numeric)
        };
    }
}

/// Compare optional floats with missing values last regardless of direction.
pub fn cmp_opt(a: Option<f64>, b: Option<f64>, sort: &Sort) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => sort.apply(a.total_cmp(&b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// A right-aligned numeric cell.
pub fn num_cell(ui: &mut Ui, text: impl Into<RichText>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text.into());
    });
}
