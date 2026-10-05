//! HELP: every function, the keyboard, and where the data comes from.

use egui::{Grid, RichText, ScrollArea, Ui};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "HELP",
    aliases: &["?", "H", "MENU"],
    name: "Help",
    category: Category::System,
    usage: "HELP",
    description: "This page: functions, keyboard shortcuts and data sources.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Help))
}

struct Help;

pub const SHORTCUTS: &[(&str, &str)] = &[
    (
        "Ctrl+K  /  Esc",
        "Focus the command line (Esc also leaves a zoomed panel)",
    ),
    ("Enter", "Run the command (<GO>)"),
    ("Tab / ↑ ↓", "Pick a suggestion"),
    ("F1", "Help"),
    ("F5", "Refresh every open feed"),
    (
        "Ctrl+Tab / Ctrl+Shift+Tab",
        "Next / previous tab in the focused pane",
    ),
    ("Ctrl+W", "Close the current tab"),
    (
        "Ctrl+M / double-click a tab",
        "Zoom the panel to fill the window, and back",
    ),
    (
        "Right-click a tab",
        "Zoom, open in a new window, copy as an image, or save as PNG",
    ),
    ("Ctrl+Shift+L", "Reset the layout"),
    (
        "↑ ↓ / Enter in a headline list",
        "Move through TOP, NEWS or NI after clicking a headline; Enter opens it in your browser",
    ),
];

impl Panel for Help {
    fn title(&self) -> String {
        "HELP".into()
    }

    fn route(&self) -> Route {
        Route::code("HELP")
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            widgets::title_bar(ui, skin, "MISO Terminal", |_| {});
            ui.label(
                "Type a function code on the command line and press Enter. Put a pricing node before or \
                 after a code (MINN.HUB GP), or type a bare node to graph it. Click any node or hub to drill in. \
                 Securities are a ticker and market code, XLU US: XLU US GP, DES XLU US, or XLU US alone.",
            );
            for cat in Category::ALL {
                widgets::section(ui, skin, cat.label());
                Grid::new(("help", cat.label())).num_columns(3).spacing([14.0, 4.0]).striped(true).show(ui, |ui| {
                    for spec in cx.registry.specs().iter().filter(|s| s.category == cat) {
                        if widgets::link(ui, skin, spec.code).clicked() {
                            cx.open(Route::code(spec.code));
                        }
                        ui.label(RichText::new(spec.usage).monospace().color(skin.text_muted));
                        ui.label(spec.description);
                        ui.end_row();
                    }
                });
            }

            widgets::section(ui, skin, "Function keys");
            ui.horizontal_wrapped(|ui| {
                for (key, command) in &cx.config.ui.hotkeys {
                    if widgets::link(ui, skin, &format!("{key} {command}")).clicked() {
                        cx.send(crate::context::AppCommand::Run(command.clone()));
                    }
                    ui.add_space(10.0);
                }
            });
            ui.label(
                RichText::new("Change them under [ui.hotkeys] in config.toml (SET opens its folder).")
                    .small()
                    .color(skin.text_muted),
            );

            widgets::section(ui, skin, "Keyboard");
            Grid::new("help-keys").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                for (keys, what) in SHORTCUTS {
                    ui.label(RichText::new(*keys).monospace().color(skin.warning));
                    ui.label(*what);
                    ui.end_row();
                }
            });

            widgets::section(ui, skin, "Data");
            ui.label(
                "Market data is MISO's public market information: the real-time data API \
                 (public-api.misoenergy.org) and the daily market reports (docs.misoenergy.org). \
                 Times are market time, EST all year (no daylight saving). Real-time feeds refresh \
                 once a minute, as MISO asks. Final RT reports trail by about a week; until then GP \
                 uses the preliminary report.",
            );
            ui.label(
                "Headlines come from the public RSS feeds of the Financial Times, Bloomberg and \
                 the Washington Post, checked every 5 to 15 minutes. Only headlines and summaries \
                 are kept, with their publisher and link; articles open in your browser, where \
                 your subscriptions apply.",
            );
            ui.label(
                "Stock and ETF prices come from Alpaca, with your own account's keys (SET): on the \
                 free plan real time from IEX alone (thinly traded names can lag) or every exchange \
                 15 minutes late, and daily history from every exchange. Securities are shown in New \
                 York time. Company news (CN) is Benzinga's, through Alpaca.",
            );
            ui.label(
                RichText::new("For information only. Not an official MISO product, and not for operational or settlement decisions.")
                    .color(skin.text_muted),
            );
        });
    }
}
