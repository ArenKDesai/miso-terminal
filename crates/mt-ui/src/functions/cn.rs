//! CN: company news. Stories tagged with a security (Benzinga's, through
//! Alpaca): the latest fifty from its news API, then new ones from its news
//! stream as they are published, in the headline browser TOP and NEWS use.
//! Headlines and summaries only; articles open in the browser.

use std::sync::Arc;

use egui::{RichText, Ui};
use mt_core::instrument::Security;
use mt_core::news::Headline;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker};
use crate::news::Browser;
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "CN",
    aliases: &["CNEWS", "COMPANYNEWS"],
    name: "Company news",
    category: Category::News,
    usage: "CN [securities…]",
    description: "News about a stock or ETF (Benzinga, through Alpaca), live as it is published. CN alone covers your watchlist's securities, or the Power & gas list.",
    takes_node: false,
    takes_security: true,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Cn {
        securities: args.iter().filter_map(|a| market::security_of(a)).collect(),
        picker: SecurityPicker::default(),
        browser: Browser::new("cn"),
        merged: (0, 0, Arc::default()),
    }))
}

struct Cn {
    /// Empty: the watchlist's securities, or the default list.
    securities: Vec<Security>,
    picker: SecurityPicker,
    browser: Browser,
    /// The REST and stream generations the merged list was built from.
    merged: (u64, u64, Arc<Vec<Headline>>),
}

impl Cn {
    /// Which symbols, and how to describe them.
    fn symbols(&self, cx: &PanelCx<'_>) -> (Vec<String>, String) {
        if !self.securities.is_empty() {
            let names: Vec<String> = self.securities.iter().map(ToString::to_string).collect();
            return (
                self.securities.iter().map(|s| s.ticker.clone()).collect(),
                names.join(", "),
            );
        }
        let watched: Vec<String> = cx
            .config
            .ui
            .favorite_securities
            .iter()
            .filter_map(|s| market::security_of(s))
            .map(|s| s.ticker)
            .collect();
        if !watched.is_empty() {
            return (watched, "your watchlist".into());
        }
        let list = cx
            .config
            .markets
            .list(mt_alpaca::config::DEFAULT_LIST)
            .unwrap_or_else(|| mt_alpaca::builtin_lists().remove(0));
        (
            list.securities().into_iter().map(|s| s.ticker).collect(),
            list.display_title().to_owned(),
        )
    }
}

impl Panel for Cn {
    fn title(&self) -> String {
        match self.securities.as_slice() {
            [] => "CN".into(),
            [one] => format!("CN {one}"),
            many => format!("CN ({})", many.len()),
        }
    }

    fn route(&self) -> Route {
        Route::new("CN", self.securities.iter().map(ToString::to_string))
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let (symbols, about) = self.symbols(cx);
        widgets::title_bar(ui, skin, &format!("Company news · {about}"), |ui| {
            if cx.alpaca.is_ready() {
                let rest = cx.hub.peek(&cx.alpaca.news(&symbols));
                widgets::freshness(ui, skin, &rest);
            }
        });
        if market::needs_keys(ui, cx) {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(s) = self.picker.show(ui, cx, "cn", "news for…") {
                self.securities = vec![s];
            }
            if !self.securities.is_empty()
                && ui
                    .small_button("Watchlist news")
                    .on_hover_text("News for the securities in WL")
                    .clicked()
            {
                self.securities.clear();
            }
        });
        let rest = cx.hub.watch(&cx.alpaca.news(&symbols));
        let topics: Vec<String> = symbols.iter().map(|s| format!("news:{s}")).collect();
        let topics: Vec<&str> = topics.iter().map(String::as_str).collect();
        let live = cx.hub.watch_stream(&cx.alpaca.news_stream(), &topics);
        if (rest.generation, live.generation) != (self.merged.0, self.merged.1)
            || self.merged.2.is_empty()
        {
            let wanted: std::collections::HashSet<&str> =
                symbols.iter().map(String::as_str).collect();
            let fresh = live
                .data()
                .map(|l| l.items.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|h| h.sections.iter().any(|s| wanted.contains(s.as_str())));
            let mut items: Vec<Headline> = fresh.collect();
            for h in rest.data().map(|r| r.items.as_slice()).unwrap_or_default() {
                if !items.iter().any(|o| o.id == h.id) {
                    items.push(h.clone());
                }
            }
            items.sort_by_key(|h| std::cmp::Reverse(h.time()));
            self.merged = (rest.generation, live.generation, Arc::new(items));
        }
        if rest.data.is_none() && self.merged.2.is_empty() {
            widgets::placeholder(ui, skin, rest.error.as_ref().map(ToString::to_string));
            return;
        }
        let items = self.merged.2.clone();
        self.browser.ui(ui, cx, &items, None, None);
        ui.label(
            RichText::new(
                "Stories from Benzinga through Alpaca: headlines and summaries only, linked to the article.",
            )
            .small()
            .color(skin.text_muted),
        );
    }
}
