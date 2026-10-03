//! A pricing-node search field with live suggestions.

use egui::Ui;

use crate::context::PanelCx;
use crate::widgets;

/// Every node name the terminal currently knows: the consolidated board's key
/// nodes plus every CP node in today's five-minute feed. Sorted, unique.
pub fn known_nodes(cx: &PanelCx<'_>) -> Vec<String> {
    let mut names: Vec<String> = cx
        .hub
        .peek(&cx.miso.lmp_board())
        .data()
        .map(|b| b.rows.iter().map(|r| r.node.clone()).collect())
        .unwrap_or_default();
    if let Some(d) = cx.hub.peek(&cx.miso.rt_intraday()).data() {
        names.extend(d.node_names().iter().cloned());
    }
    names.sort();
    names.dedup();
    names
}

#[derive(Default)]
pub struct NodePicker {
    search: String,
}

impl NodePicker {
    /// Draw the field (and, while typing, matching nodes underneath). Returns a
    /// node when one is chosen: a suggestion click, or Enter on typed text.
    pub fn show(&mut self, ui: &mut Ui, cx: &PanelCx<'_>, id: &str, hint: &str) -> Option<String> {
        let mut chosen = None;
        ui.vertical(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id_salt(("node-picker", id))
                    .hint_text(hint)
                    .desired_width(180.0),
            );
            if resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !self.search.trim().is_empty()
            {
                chosen = Some(self.search.trim().to_ascii_uppercase());
            }
            let needle = self.search.trim().to_ascii_uppercase();
            if needle.len() >= 2 && chosen.is_none() {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(420.0);
                    for n in known_nodes(cx)
                        .iter()
                        .filter(|n| n.contains(&needle))
                        .take(10)
                    {
                        if widgets::link(ui, cx.skin, n).clicked() {
                            chosen = Some(n.clone());
                        }
                    }
                });
            }
        });
        if chosen.is_some() {
            self.search.clear();
        }
        chosen
    }
}
