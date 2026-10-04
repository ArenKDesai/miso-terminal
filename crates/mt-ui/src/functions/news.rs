//! NEWS: every headline kept from every feed, by publisher or by words.

use egui::Ui;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::news::{self, Browser, Combined};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "NEWS",
    aliases: &["N", "SEARCH"],
    name: "News search",
    category: Category::News,
    usage: "NEWS [source] [words…]",
    description: "Every headline from every feed, kept for a few weeks: by publisher (NEWS FT, NEWS BBG, NEWS WP) or by words (NEWS natural gas).",
    takes_node: false,
    // `NEWS XLU US` searches for the ticker; company news comes with Alpaca.
    takes_security: true,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(News {
        pending: Some(args.to_vec()),
        combined: Combined::default(),
        browser: Browser::new("news"),
    }))
}

struct News {
    /// Arguments not yet read: which are a publisher needs the configured feeds.
    pending: Option<Vec<String>>,
    combined: Combined,
    browser: Browser,
}

impl News {
    /// `[source] [words…]`: a first argument naming a publisher filters by it.
    fn apply_args(&mut self, cx: &PanelCx<'_>, args: &[String]) {
        let feeds = cx.config.news.all_feeds();
        let sources = feeds.iter().map(|f| f.source.as_str());
        let source = args
            .first()
            .and_then(|a| mt_news::source_named(a, sources))
            .map(str::to_owned);
        let skip = usize::from(source.is_some());
        self.browser.source = source;
        self.browser.query = args[skip..].join(" ");
    }
}

impl Panel for News {
    fn title(&self) -> String {
        let route = self.route();
        if route.args.is_empty() {
            "NEWS".into()
        } else {
            route.to_string()
        }
    }

    fn route(&self) -> Route {
        if let Some(args) = &self.pending {
            return Route::new("NEWS", args.clone());
        }
        let mut args: Vec<String> = self
            .browser
            .source
            .iter()
            .map(|s| mt_news::short_name(s))
            .collect();
        args.extend(self.browser.query.split_whitespace().map(str::to_owned));
        Route::new("NEWS", args)
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        self.pending = Some(args.to_vec());
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if let Some(args) = self.pending.take() {
            self.apply_args(cx, &args);
        }
        self.combined
            .watch(cx.hub, &news::queries(cx.config, false));
        widgets::title_bar(ui, skin, "News", |ui| {
            news::health_label(ui, skin, self.combined.health());
        });
        let items = self.combined.items().clone();
        self.browser.ui(ui, cx, &items, None, None);
    }
}
