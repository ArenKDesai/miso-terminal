//! CONS: binding transmission constraints in the real-time market.

use std::collections::HashMap;

use chrono::NaiveDateTime;
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::table::{Sort, cmp_opt, num_cell, sort_header};
use crate::widgets::{self, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "CONS",
    aliases: &["BC", "CONSTRAINTS"],
    name: "Binding constraints",
    category: Category::Grid,
    usage: "CONS",
    description: "Real-time binding transmission constraints with shadow prices, and how long each has bound this session.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Cons {
        first_seen: HashMap::new(),
        seen_generation: 0,
        filter: String::new(),
        sort: Sort::new(1, false),
    }))
}

struct Cons {
    /// When this panel first saw each constraint binding (session memory).
    first_seen: HashMap<String, NaiveDateTime>,
    seen_generation: u64,
    filter: String,
    sort: Sort,
}

impl Panel for Cons {
    fn title(&self) -> String {
        "CONS".into()
    }

    fn route(&self) -> Route {
        Route::code("CONS")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let snap = cx.hub.watch(&cx.miso.binding_constraints());
        if let Some(c) = snap.data()
            && snap.generation != self.seen_generation
        {
            self.seen_generation = snap.generation;
            let now = c.interval.unwrap_or_else(mt_core::time::now_market);
            self.first_seen
                .retain(|name, _| c.constraints.iter().any(|k| &k.name == name));
            for k in &c.constraints {
                self.first_seen.entry(k.name.clone()).or_insert(now);
            }
        }

        widgets::title_bar(ui, skin, "Binding constraints", |ui| {
            widgets::freshness(ui, skin, &snap)
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("filter")
                .desired_width(200.0),
        );
        widgets::with_data(ui, skin, &snap, |ui, c| {
            let needle = self.filter.trim().to_ascii_uppercase();
            let mut rows: Vec<_> = c
                .constraints
                .iter()
                .filter(|k| needle.is_empty() || k.name.to_ascii_uppercase().contains(&needle))
                .collect();
            if rows.is_empty() {
                ui.label(RichText::new("Nothing binding.").color(skin.text_muted));
                return;
            }
            let s = self.sort;
            rows.sort_by(|a, b| match s.column {
                0 => s.apply(a.name.cmp(&b.name)),
                3 => s.apply(
                    self.first_seen
                        .get(&a.name)
                        .cmp(&self.first_seen.get(&b.name)),
                ),
                _ => cmp_opt(a.shadow_price, b.shadow_price, &s),
            });
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
            let mut sort = self.sort;
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .column(Column::remainder().at_least(220.0).clip(true))
                .column(Column::initial(100.0))
                .column(Column::initial(110.0))
                .column(Column::initial(110.0))
                .header(row_h + 4.0, |mut h| {
                    h.col(|ui| sort_header(ui, skin, "Constraint", 0, false, &mut sort));
                    h.col(|ui| sort_header(ui, skin, "Shadow $", 1, true, &mut sort));
                    h.col(|ui| {
                        widgets::label(ui, skin, "Curve");
                    });
                    h.col(|ui| sort_header(ui, skin, "Binding since", 3, false, &mut sort));
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let k = rows[row.index()];
                        row.col(|ui| {
                            ui.label(RichText::new(&k.name).monospace());
                        });
                        row.col(|ui| {
                            num_cell(
                                ui,
                                RichText::new(fmt::price_opt(k.shadow_price)).color(skin.warning),
                            )
                        });
                        row.col(|ui| {
                            let curve = match (k.overridden, &k.curve_type) {
                                (Some(true), Some(c)) => format!("{c} (override)"),
                                (_, Some(c)) => c.clone(),
                                _ => fmt::DASH.into(),
                            };
                            ui.label(RichText::new(curve).color(skin.text_muted));
                        });
                        row.col(|ui| {
                            let since = self
                                .first_seen
                                .get(&k.name)
                                .map_or_else(|| fmt::DASH.into(), |t| fmt::hm(*t));
                            ui.label(RichText::new(since).color(skin.text_muted));
                        });
                    });
                });
            self.sort = sort;
            ui.label(
                RichText::new(
                    "“Binding since” counts from when this tab first saw the constraint.",
                )
                .small()
                .color(skin.text_muted),
            );
        });

        // Reserve and sub-regional constraints rarely bind, so they get a line each.
        for (title, snap) in [
            (
                "Reserve constraints",
                cx.hub.watch(&cx.miso.reserve_constraints()),
            ),
            (
                "Sub-regional constraints",
                cx.hub.watch(&cx.miso.subregional_constraints()),
            ),
        ] {
            widgets::section(ui, skin, title);
            match snap.data() {
                Some(c) if c.constraints.is_empty() => {
                    ui.label(RichText::new("None binding.").color(skin.text_muted));
                }
                Some(c) => {
                    for k in &c.constraints {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&k.name).monospace());
                            ui.label(
                                RichText::new(fmt::price_opt(k.shadow_price)).color(skin.warning),
                            );
                        });
                    }
                }
                None => {
                    widgets::placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string))
                }
            }
        }
    }
}
