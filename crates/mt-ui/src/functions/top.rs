//! TOP: top stories, merged from each publisher's front-page feed.

use egui::Ui;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::news::{self, Browser, Combined};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "TOP",
    aliases: &["TOPS", "HEADLINES"],
    name: "Top stories",
    category: Category::News,
    usage: "TOP",
    description: "Top stories from the Financial Times, Bloomberg and the Washington Post, newest first. Enter opens one in your browser.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

/// Top stories shown (the feeds keep more, for NEWS).
const LIMIT: usize = 100;

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Top {
        combined: Combined::default(),
        browser: Browser::new("top"),
    }))
}

struct Top {
    combined: Combined,
    browser: Browser,
}

impl Panel for Top {
    fn title(&self) -> String {
        "TOP".into()
    }

    fn route(&self) -> Route {
        Route::code("TOP")
    }

    fn absorb(&mut self, _args: &[String]) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        self.combined.watch(cx.hub, &news::queries(cx.config, true));
        widgets::title_bar(ui, skin, "Top stories", |ui| {
            news::health_label(ui, skin, self.combined.health());
        });
        let items = self.combined.items().clone();
        self.browser.ui(ui, cx, &items, None, Some(LIMIT));
    }
}
