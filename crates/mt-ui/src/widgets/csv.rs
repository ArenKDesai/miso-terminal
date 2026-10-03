//! Copy tables to the clipboard as CSV, for pasting into Excel.

use std::time::Duration;

use egui::{RichText, Ui};

use crate::skin::Skin;

/// RFC 4180 CSV: fields with commas, quotes or newlines are quoted.
pub fn to_csv(headers: &[&str], rows: impl IntoIterator<Item = Vec<String>>) -> String {
    fn field(s: &str) -> String {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    }
    let mut out = headers
        .iter()
        .map(|h| field(h))
        .collect::<Vec<_>>()
        .join(",");
    out.push_str("\r\n");
    for row in rows {
        out.push_str(&row.iter().map(|c| field(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

/// A "Copy CSV" button that says "Copied" for a moment after a click. The CSV
/// is only built when clicked.
pub fn copy_button(ui: &mut Ui, skin: &Skin, make_csv: impl FnOnce() -> String) {
    let id = ui.id().with("copy-csv");
    let now = ui.input(|i| i.time);
    let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
    let recently = copied_at.is_some_and(|t| now - t < 2.0);
    let label = if recently {
        RichText::new("Copied ✓").color(skin.positive)
    } else {
        RichText::new("Copy CSV")
    };
    if ui
        .small_button(label)
        .on_hover_text("Copy this table for Excel")
        .clicked()
    {
        ui.ctx().copy_text(make_csv());
        ui.data_mut(|d| d.insert_temp(id, now));
        ui.ctx().request_repaint_after(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        let csv = to_csv(
            &["node", "note"],
            [vec!["MINN.HUB".into(), "a, \"b\"".into()]],
        );
        assert_eq!(csv, "node,note\r\nMINN.HUB,\"a, \"\"b\"\"\"\r\n");
    }
}
