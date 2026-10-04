//! Headlines for the news functions (TOP, NEWS, NI), HOME's tile and
//! headline alerts: every configured feed watched through the hub and
//! combined into one de-duplicated list, and a browser for it (filters, a
//! keyboard-driven list, a preview). Articles open in the reader's own
//! browser, where they are signed in; the terminal never fetches them.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use egui::{Key, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::news::{Headline, Matcher};
use mt_data::{DataHub, Query};
use mt_news::FeedQuery;

use crate::config::AppConfig;
use crate::context::{AppCommand, PanelCx};
use crate::skin::Skin;
use crate::widgets::{self, fmt};

/// Queries for the configured feeds; `top_only` keeps the top-stories feeds.
pub fn queries(config: &AppConfig, top_only: bool) -> Vec<FeedQuery> {
    config
        .news
        .feeds()
        .into_iter()
        .filter(|f| !top_only || f.top)
        .map(|f| FeedQuery::new(f, config.news.keep_days))
        .collect()
}

/// How a set of feeds is doing, for a title bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub feeds: usize,
    pub loaded: usize,
    pub loading: bool,
    pub stale: usize,
    /// Feed label and error, for each failing feed.
    pub failing: Vec<(String, String)>,
    /// The most recent update of any feed.
    pub updated: Option<DateTime<Utc>>,
}

/// Several feeds' headlines as one list, rebuilt only when a feed changes.
#[derive(Default)]
pub struct Combined {
    stamp: Vec<(String, u64)>,
    items: Arc<Vec<Headline>>,
    health: Health,
}

impl Combined {
    /// Watch `feeds` (call every frame, like `hub.watch`) and refresh the list.
    pub fn watch(&mut self, hub: &DataHub, feeds: &[FeedQuery]) {
        let snaps: Vec<_> = feeds.iter().map(|q| (q, hub.watch(q))).collect();
        let mut health = Health {
            feeds: feeds.len(),
            ..Health::default()
        };
        for (q, s) in &snaps {
            health.loaded += usize::from(s.data.is_some());
            health.loading |= s.loading;
            health.stale += usize::from(s.stale);
            if let Some(e) = &s.error {
                health.failing.push((q.label(), e.to_string()));
            }
            health.updated = health.updated.max(s.updated);
        }
        self.health = health;
        let stamp: Vec<(String, u64)> =
            snaps.iter().map(|(q, s)| (q.key(), s.generation)).collect();
        if stamp != self.stamp {
            self.items = Arc::new(mt_news::combine(
                snaps
                    .iter()
                    .filter_map(|(_, s)| s.data())
                    .map(|d| d.items.as_slice()),
            ));
            self.stamp = stamp;
        }
    }

    pub fn items(&self) -> &Arc<Vec<Headline>> {
        &self.items
    }

    pub fn health(&self) -> &Health {
        &self.health
    }
}

/// "13 feeds · updated 2m ago" with a lamp, or what is failing.
pub fn health_label(ui: &mut Ui, skin: &Skin, h: &Health) {
    let (color, text) = if h.feeds == 0 {
        (skin.text_muted, "No feeds configured".to_owned())
    } else if !h.failing.is_empty() {
        let color = if h.failing.len() == h.feeds {
            skin.negative
        } else {
            skin.warning
        };
        (
            color,
            format!("{} of {} feeds failing", h.failing.len(), h.feeds),
        )
    } else if h.loaded == 0 {
        (skin.text_muted, "Loading…".to_owned())
    } else {
        let when = h.updated.map_or_else(|| fmt::DASH.into(), fmt::ago);
        let color = if h.stale > 0 { skin.warning } else { skin.live };
        let feeds = if h.feeds == 1 { "feed" } else { "feeds" };
        (color, format!("{} {feeds} · updated {when}", h.feeds))
    };
    let resp = ui.label(RichText::new(text).small().color(skin.text_muted));
    if !h.failing.is_empty() {
        let detail: Vec<String> = h
            .failing
            .iter()
            .map(|(feed, e)| format!("{feed}: {e}"))
            .collect();
        resp.on_hover_text(format!("{}\nLOG shows every feed.", detail.join("\n")));
    }
    widgets::lamp(ui, color);
}

/// A headline's time for a list: `16:11` today, `Oct 03` before (market time).
pub fn list_time(t: DateTime<Utc>) -> String {
    let t = mt_core::time::to_market(t);
    if t.date() == mt_core::time::market_today() {
        fmt::hm(t)
    } else {
        t.format("%b %d").to_string()
    }
}

/// `Sun Oct 04 16:11 EST`
pub fn full_time(t: DateTime<Utc>) -> String {
    format!(
        "{} {}",
        mt_core::time::to_market(t).format("%a %b %d %H:%M"),
        mt_core::time::MARKET_TZ_LABEL
    )
}

/// Ask the shell to open a headline in the browser (and mark it read).
pub fn open(cx: &mut PanelCx<'_>, h: &Headline) {
    cx.send(AppCommand::OpenHeadline {
        id: h.id.clone(),
        link: h.link.clone(),
    });
}

/// What a browser shows: filters on top of the panel's own selection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct FilterKey {
    items: usize,
    len: usize,
    source: Option<String>,
    query: String,
    topic: Option<String>,
}

/// The list of headlines with filters, keyboard navigation and a preview,
/// shared by TOP, NEWS and NI.
pub struct Browser {
    /// Show one publisher only.
    pub source: Option<String>,
    /// Words that must all appear (title, summary, source or section).
    pub query: String,
    pub unread_only: bool,
    selected: Option<String>,
    scroll_to: Option<usize>,
    filtered: (FilterKey, Vec<usize>),
    id: egui::Id,
}

impl Browser {
    pub fn new(id: &str) -> Self {
        Self {
            source: None,
            query: String::new(),
            unread_only: false,
            selected: None,
            scroll_to: None,
            filtered: (FilterKey::default(), Vec::new()),
            id: egui::Id::new(("mt-news-browser", id)),
        }
    }

    /// Select a headline, as a click would.
    #[cfg(test)]
    pub(crate) fn select(&mut self, id: &str) {
        self.selected = Some(id.to_owned());
    }

    /// Indices into `items` that pass the filters (`topic` adds a keyword
    /// rule), newest first, at most `limit`.
    fn visible(
        &mut self,
        items: &Arc<Vec<Headline>>,
        topic: Option<(&str, &Matcher)>,
        read: &mt_news::ReadMarks,
        limit: Option<usize>,
    ) -> Vec<usize> {
        let key = FilterKey {
            items: Arc::as_ptr(items) as usize,
            len: items.len(),
            source: self.source.clone(),
            query: self.query.trim().to_owned(),
            topic: topic.map(|(name, _)| name.to_owned()),
        };
        if key != self.filtered.0 {
            let matched = items
                .iter()
                .enumerate()
                .filter(|(_, h)| self.source.as_ref().is_none_or(|s| &h.source == s))
                .filter(|(_, h)| topic.is_none_or(|(_, m)| m.matches(h)))
                .filter(|(_, h)| h.matches_query(&key.query))
                .map(|(i, _)| i)
                .collect();
            self.filtered = (key, matched);
        }
        self.filtered
            .1
            .iter()
            .copied()
            .filter(|&i| !self.unread_only || !read.is_read(&items[i].id))
            .take(limit.unwrap_or(usize::MAX))
            .collect()
    }

    /// The filters, the list and the preview of the selected headline.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        items: &Arc<Vec<Headline>>,
        topic: Option<(&str, &Matcher)>,
        limit: Option<usize>,
    ) {
        let skin = cx.skin;
        self.filters(ui, items);
        let rows = self.visible(items, topic, cx.news_read, limit);
        ui.horizontal_wrapped(|ui| {
            let unread = rows
                .iter()
                .filter(|&&i| !cx.news_read.is_read(&items[i].id))
                .count();
            ui.label(
                RichText::new(format!("{} headlines · {unread} unread", rows.len()))
                    .small()
                    .color(skin.text_muted),
            );
            if unread > 0
                && ui
                    .small_button("Mark all read")
                    .on_hover_text("Mark every headline listed here as read")
                    .clicked()
            {
                cx.send(AppCommand::MarkRead {
                    ids: rows.iter().map(|&i| items[i].id.clone()).collect(),
                    read: true,
                });
            }
        });
        if rows.is_empty() {
            ui.add_space(8.0);
            let msg = if items.is_empty() {
                "No headlines yet."
            } else {
                "No headlines match. Clear the search or pick another source."
            };
            ui.label(RichText::new(msg).color(skin.text_muted));
            return;
        }
        self.keyboard(ui, cx, items, &rows);
        let preview_height = 132.0;
        let list_height = (ui.available_height() - preview_height).max(140.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), list_height), |ui| {
            self.list(ui, cx, items, &rows, list_height);
        });
        widgets::section(ui, skin, "Headline");
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| rows.iter().map(|&i| &items[i]).find(|h| &h.id == id));
        match selected {
            Some(h) => preview(ui, cx, h),
            None => {
                ui.label(
                    RichText::new(
                        "Click a headline to read its summary. ↑ ↓ move, Enter (or a \
                         double-click) opens the article in your browser.",
                    )
                    .color(skin.text_muted),
                );
            }
        }
    }

    fn filters(&mut self, ui: &mut Ui, items: &[Headline]) {
        let mut sources: Vec<String> = Vec::new();
        for s in mt_news::SOURCES
            .iter()
            .map(|s| s.name.to_owned())
            .chain(items.iter().map(|h| h.source.clone()))
        {
            if !sources.contains(&s) && items.iter().any(|h| h.source == s) {
                sources.push(s);
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.source.is_none(), "All").clicked() {
                self.source = None;
            }
            for s in &sources {
                let on = self.source.as_ref() == Some(s);
                if ui
                    .selectable_label(on, mt_news::short_name(s))
                    .on_hover_text(s)
                    .clicked()
                {
                    self.source = (!on).then(|| s.clone());
                }
            }
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("search headlines…")
                    .desired_width(200.0),
            );
            if !self.query.is_empty() && ui.small_button("✕").clicked() {
                self.query.clear();
            }
            ui.checkbox(&mut self.unread_only, "Unread only");
        });
    }

    fn keyboard(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, items: &[Headline], rows: &[usize]) {
        if !ui.memory(|m| m.has_focus(self.id)) {
            return;
        }
        let pos = self
            .selected
            .as_ref()
            .and_then(|id| rows.iter().position(|&i| &items[i].id == id));
        let (down, up, page_down, page_up, home, end, enter) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::PageDown),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::Enter),
            )
        });
        let last = rows.len() - 1;
        let next = match pos {
            _ if home => Some(0),
            _ if end => Some(last),
            None if down || up || page_down || page_up => Some(0),
            Some(p) if down => Some((p + 1).min(last)),
            Some(p) if up => Some(p.saturating_sub(1)),
            Some(p) if page_down => Some((p + 10).min(last)),
            Some(p) if page_up => Some(p.saturating_sub(10)),
            _ => None,
        };
        if let Some(n) = next {
            self.selected = Some(items[rows[n]].id.clone());
            self.scroll_to = Some(n);
        }
        if enter && let Some(p) = pos {
            open(cx, &items[rows[p]]);
        }
    }

    fn list(
        &mut self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        items: &[Headline],
        rows: &[usize],
        height: f32,
    ) {
        let skin = cx.skin;
        // The list as a whole takes keyboard focus; rows on top take clicks.
        let area = ui.available_rect_before_wrap();
        let focus = ui.interact(area, self.id, Sense::click());
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                self.id,
                egui::EventFilter {
                    vertical_arrows: true,
                    horizontal_arrows: true,
                    tab: false,
                    escape: false,
                },
            );
        });
        if focus.clicked() {
            focus.request_focus();
        }
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
        let mut table = TableBuilder::new(ui)
            .id_salt(self.id)
            .striped(true)
            .sense(Sense::click())
            .max_scroll_height(height)
            .auto_shrink(false)
            .column(Column::exact(52.0))
            .column(Column::exact(40.0))
            .column(Column::remainder().clip(true));
        if let Some(row) = self.scroll_to.take() {
            table = table.scroll_to_row(row, None);
        }
        let mut clicked = None;
        let mut opened = None;
        table.body(|body| {
            body.rows(row_h, rows.len(), |mut row| {
                let h = &items[rows[row.index()]];
                let read = cx.news_read.is_read(&h.id);
                row.set_selected(self.selected.as_ref() == Some(&h.id));
                row.col(|ui| {
                    ui.label(
                        RichText::new(list_time(h.time()))
                            .monospace()
                            .color(skin.text_muted),
                    );
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(mt_news::short_name(&h.source))
                            .monospace()
                            .color(skin.accent),
                    )
                    .on_hover_text(&h.source);
                });
                row.col(|ui| {
                    let color = if read {
                        skin.text_muted
                    } else {
                        skin.text_strong
                    };
                    let resp = ui.add(
                        egui::Label::new(RichText::new(&h.title).color(color))
                            .truncate()
                            .selectable(false),
                    );
                    if !h.summary.is_empty() {
                        resp.on_hover_text(&h.summary);
                    }
                });
                let resp = row.response();
                if resp.double_clicked() {
                    opened = Some(h.clone());
                } else if resp.clicked() {
                    clicked = Some(h.id.clone());
                }
            });
        });
        if let Some(id) = clicked {
            self.selected = Some(id);
            focus.request_focus();
        }
        if let Some(h) = opened {
            self.selected = Some(h.id.clone());
            focus.request_focus();
            open(cx, &h);
        }
    }
}

/// The selected headline: title, attribution, summary and what to do with it.
fn preview(ui: &mut Ui, cx: &mut PanelCx<'_>, h: &Headline) {
    let skin = cx.skin;
    ui.label(RichText::new(&h.title).strong().color(skin.text_strong));
    let mut meta = vec![h.source.clone()];
    if !h.sections.is_empty() {
        meta.push(h.sections.join(", "));
    }
    if let Some(a) = &h.author {
        meta.push(a.clone());
    }
    meta.push(full_time(h.time()));
    ui.label(
        RichText::new(meta.join(" · "))
            .small()
            .color(skin.text_muted),
    );
    if !h.summary.is_empty() {
        ui.label(&h.summary);
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Open in browser ↗")
            .on_hover_text(&h.link)
            .clicked()
        {
            open(cx, h);
        }
        if ui.small_button("Copy link").clicked() {
            ui.ctx().copy_text(h.link.clone());
        }
        let read = cx.news_read.is_read(&h.id);
        if ui
            .small_button(if read { "Mark unread" } else { "Mark read" })
            .clicked()
        {
            cx.send(AppCommand::MarkRead {
                ids: vec![h.id.clone()],
                read: !read,
            });
        }
        ui.label(
            RichText::new(mt_data::host_of(&h.link))
                .small()
                .color(skin.text_muted),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_times_are_market_time() {
        let now = mt_core::time::now_utc();
        assert_eq!(list_time(now), fmt::hm(mt_core::time::to_market(now)));
        let old = now - chrono::Duration::days(3);
        assert_eq!(
            list_time(old),
            mt_core::time::to_market(old).format("%b %d").to_string()
        );
        assert!(full_time(now).ends_with(mt_core::time::MARKET_TZ_LABEL));
    }
}
