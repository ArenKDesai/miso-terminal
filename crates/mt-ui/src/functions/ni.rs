//! NI: headlines on a topic (`NI ENERGY`), from keyword rules. The topics
//! are built in and editable in config (`[[news.topics]]`).

use std::sync::Arc;

use egui::{Grid, RichText, ScrollArea, Ui};
use mt_core::news::{Headline, Matcher, Topic};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::news::{self, Browser, Combined};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "NI",
    aliases: &["TOPIC", "TOPICS"],
    name: "News by topic",
    category: Category::News,
    usage: "NI [topic]",
    description: "Headlines on a topic, from keyword rules: NI ENERGY, POWER, GRID, GAS, OIL, UTILITIES, POLICY, CLIMATE, MACRO. NI alone lists them; add your own in config.",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Ni {
        topic: topic_arg(args),
        combined: Combined::default(),
        browser: Browser::new("ni"),
        counts: None,
    }))
}

fn topic_arg(args: &[String]) -> Option<String> {
    let t = args.join(" ");
    (!t.trim().is_empty()).then(|| t.trim().to_uppercase())
}

struct Ni {
    topic: Option<String>,
    combined: Combined,
    browser: Browser,
    counts: Option<Counts>,
}

/// Per topic: headlines in the last day and in all, for the list of topics;
/// recounted when the headlines (by list identity) or the topics change.
type Counts = (usize, Vec<Topic>, Vec<(usize, usize)>);

impl Panel for Ni {
    fn title(&self) -> String {
        match &self.topic {
            Some(t) => format!("NI {t}"),
            None => "NI".into(),
        }
    }

    fn route(&self) -> Route {
        Route::new("NI", self.topic.iter().cloned())
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        self.topic = topic_arg(args);
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        self.combined
            .watch(cx.hub, &news::queries(cx.config, false));
        let topics = cx.config.news.topics();
        let title = match &self.topic {
            Some(t) => format!("News · {t}"),
            None => "News by topic".into(),
        };
        widgets::title_bar(ui, skin, &title, |ui| {
            news::health_label(ui, skin, self.combined.health());
        });
        ui.horizontal_wrapped(|ui| {
            for t in &topics {
                let on = self.topic.as_deref() == Some(t.name.as_str());
                if ui
                    .selectable_label(on, &t.name)
                    .on_hover_text(&t.description)
                    .clicked()
                {
                    self.topic = (!on).then(|| t.name.clone());
                }
            }
        });
        let items = self.combined.items().clone();
        let Some(name) = self.topic.clone() else {
            self.overview(ui, cx, &topics, &items);
            return;
        };
        let matcher = match topics.iter().find(|t| t.name == name) {
            Some(t) => t.matcher(),
            None => {
                ui.label(
                    RichText::new(format!(
                        "No topic named {name}: showing headlines that mention it. Add topics \
                         under [[news.topics]] in config.toml."
                    ))
                    .small()
                    .color(skin.text_muted),
                );
                Matcher::from_list(&name)
            }
        };
        self.browser
            .ui(ui, cx, &items, Some((name.as_str(), &matcher)), None);
    }
}

impl Ni {
    /// Every topic with its keywords and how many headlines it has.
    fn overview(
        &mut self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        topics: &[Topic],
        items: &Arc<Vec<Headline>>,
    ) {
        let skin = cx.skin;
        let stamp = Arc::as_ptr(items) as usize;
        if self
            .counts
            .as_ref()
            .is_none_or(|(s, t, _)| *s != stamp || t != topics)
        {
            let day_ago = mt_core::time::now_utc() - chrono::Duration::days(1);
            let counts = topics
                .iter()
                .map(|t| {
                    let m = t.matcher();
                    let hits: Vec<&Headline> = items.iter().filter(|h| m.matches(h)).collect();
                    let recent = hits.iter().filter(|h| h.time() >= day_ago).count();
                    (recent, hits.len())
                })
                .collect();
            self.counts = Some((stamp, topics.to_vec(), counts));
        }
        let counts = self
            .counts
            .as_ref()
            .map(|(_, _, c)| c.clone())
            .unwrap_or_default();
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            Grid::new("ni-topics")
                .striped(true)
                .num_columns(4)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for h in ["Topic", "24 h", "Kept", "What it covers"] {
                        widgets::label(ui, skin, h);
                    }
                    ui.end_row();
                    for (t, (recent, all)) in topics.iter().zip(&counts) {
                        if widgets::link(ui, skin, &t.name).clicked() {
                            self.topic = Some(t.name.clone());
                        }
                        ui.label(recent.to_string());
                        ui.label(RichText::new(all.to_string()).color(skin.text_muted));
                        ui.label(&t.description)
                            .on_hover_text(t.keywords.join(", "));
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Topics match words and phrases in titles and summaries. A keyword in \
                     capitals (MISO, PJM, LNG) matches capitals only; utilit* matches any \
                     ending. Change them or add your own under [[news.topics]] in config.toml.",
                )
                .small()
                .color(skin.text_muted),
            );
        });
    }
}
