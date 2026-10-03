//! ALRT: alert rules and what they have fired. Rules are evaluated every frame
//! by the shell; a firing rule flashes the taskbar button and shows a ⚠ badge.

use egui::{Grid, RichText, ScrollArea, Ui};

use crate::alerts::AlertRule;
use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets;
use crate::widgets::node_picker::NodePicker;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ALRT",
    aliases: &["ALERT", "ALERTS"],
    name: "Alerts",
    category: Category::System,
    usage: "ALRT",
    description: "Price and constraint alerts: add rules, see which hold now, and the history of what fired.",
    takes_node: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Alrt {
        kind: Kind::PriceAbove,
        node: None,
        picker: NodePicker::default(),
        value: 100.0,
        contains: String::new(),
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    PriceAbove,
    PriceBelow,
    ConstraintBinds,
    ShadowPriceAbove,
}

impl Kind {
    const ALL: [Self; 4] = [
        Self::PriceAbove,
        Self::PriceBelow,
        Self::ConstraintBinds,
        Self::ShadowPriceAbove,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::PriceAbove => "RT price at or above",
            Self::PriceBelow => "RT price at or below",
            Self::ConstraintBinds => "Constraint binds",
            Self::ShadowPriceAbove => "Any |shadow price| at or above",
        }
    }
}

struct Alrt {
    kind: Kind,
    node: Option<String>,
    picker: NodePicker,
    value: f64,
    contains: String,
}

impl Alrt {
    fn rule(&self) -> Option<AlertRule> {
        let node = || self.node.clone();
        Some(match self.kind {
            Kind::PriceAbove => AlertRule::PriceAbove {
                node: node()?,
                value: self.value,
            },
            Kind::PriceBelow => AlertRule::PriceBelow {
                node: node()?,
                value: self.value,
            },
            Kind::ConstraintBinds if !self.contains.trim().is_empty() => {
                AlertRule::ConstraintBinds {
                    contains: self.contains.trim().to_owned(),
                }
            }
            Kind::ConstraintBinds => return None,
            Kind::ShadowPriceAbove => AlertRule::ShadowPriceAbove {
                value: self.value.abs(),
            },
        })
    }
}

impl Panel for Alrt {
    fn title(&self) -> String {
        "ALRT".into()
    }

    fn route(&self) -> Route {
        Route::code("ALRT")
    }

    fn absorb(&mut self, _args: &[String]) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if cx.alerts.unseen > 0 {
            cx.send(AppCommand::AlertsSeen);
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            widgets::title_bar(ui, skin, "Alerts", |_| {});
            widgets::section(ui, skin, "Rules");
            if cx.config.alerts.is_empty() {
                ui.label(RichText::new("No rules yet. Add one below.").color(skin.text_muted));
            }
            let mut remove = None;
            Grid::new("alrt-rules").striped(true).num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                for (i, rule) in cx.config.alerts.iter().enumerate() {
                    let (color, state) = match cx.alerts.is_active(rule) {
                        Some(true) => (skin.warning, "triggered"),
                        Some(false) => (skin.live, "armed"),
                        None => (skin.text_muted, "waiting for data"),
                    };
                    ui.horizontal(|ui| {
                        widgets::lamp(ui, color);
                        ui.label(RichText::new(state).small().color(color));
                    });
                    ui.label(rule.describe());
                    if ui.small_button("✕").on_hover_text("Remove this rule").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                cx.send(AppCommand::RemoveAlert(i));
            }

            widgets::section(ui, skin, "Add a rule");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("alrt-kind")
                    .selected_text(self.kind.label())
                    .show_ui(ui, |ui| {
                        for k in Kind::ALL {
                            ui.selectable_value(&mut self.kind, k, k.label());
                        }
                    });
                match self.kind {
                    Kind::PriceAbove | Kind::PriceBelow => {
                        ui.add(egui::DragValue::new(&mut self.value).speed(1.0).prefix("$"));
                        ui.label("at");
                        if let Some(n) = &self.node {
                            ui.label(RichText::new(n).strong().color(skin.text_strong));
                        }
                        if let Some(n) = self.picker.show(ui, cx, "alrt-node", "node…") {
                            self.node = Some(n);
                        }
                    }
                    Kind::ConstraintBinds => {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.contains)
                                .hint_text("name contains…")
                                .desired_width(220.0),
                        );
                    }
                    Kind::ShadowPriceAbove => {
                        ui.add(egui::DragValue::new(&mut self.value).speed(5.0).prefix("$"));
                    }
                }
                let rule = self.rule();
                if ui.add_enabled(rule.is_some(), egui::Button::new("Add")).clicked()
                    && let Some(rule) = rule
                {
                    cx.send(AppCommand::AddAlert(rule));
                }
            });
            ui.label(
                RichText::new(
                    "Rules fire once when they become true and re-arm when they clear. A firing rule \
                     flashes the taskbar button. Rules are saved in config.toml under [[alerts]].",
                )
                .small()
                .color(skin.text_muted),
            );

            widgets::section(ui, skin, "Fired");
            if cx.alerts.history.is_empty() {
                ui.label(RichText::new("Nothing has fired this session.").color(skin.text_muted));
            }
            for e in &cx.alerts.history {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(e.at.format("%H:%M:%S").to_string()).monospace().color(skin.text_muted));
                    ui.label(RichText::new(&e.rule).color(skin.warning));
                    ui.label(&e.detail);
                });
            }
        });
    }
}
