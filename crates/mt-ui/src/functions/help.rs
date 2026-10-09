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
        "F11 / Alt+Enter",
        "Full screen, and back; in a popped-out window, that window (a command bound to F11 runs instead; Alt+Enter still works)",
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
            // The version for bug reports: a release build is a windowed
            // program, so `--version` at a prompt prints nothing unless piped.
            widgets::title_bar(ui, skin, "MISO Terminal", |ui| {
                ui.label(
                    RichText::new(format!("version {}", crate::version()))
                        .monospace()
                        .color(skin.text_muted),
                );
            });
            ui.label(
                "Type a function code on the command line and press Enter. Put a pricing node before or \
                 after a code (MINN.HUB GP), or type a bare node to graph it. Click any node or hub to drill in. \
                 Securities are a ticker and market code, XLU US: XLU US GP, DES XLU US, or XLU US alone. \
                 Options are OCC symbols (XLU261218C00045000); one typed alone opens its chain in OMON.",
            );
            // The code and its arguments on one line, the description wrapped
            // under them. (A grid sized its columns to the longest usage and
            // let descriptions run off the pane.)
            for cat in Category::ALL {
                widgets::section(ui, skin, cat.label());
                for spec in cx.registry.specs().iter().filter(|s| s.category == cat) {
                    ui.horizontal_wrapped(|ui| {
                        if widgets::link(ui, skin, spec.code).clicked() {
                            cx.open(Route::code(spec.code));
                        }
                        let args = spec.usage.strip_prefix(spec.code).unwrap_or(spec.usage).trim();
                        if !args.is_empty() {
                            ui.label(RichText::new(args).monospace().color(skin.text_muted));
                        }
                    });
                    ui.indent(spec.code, |ui| ui.label(spec.description));
                    ui.add_space(2.0);
                }
            }

            widgets::section(ui, skin, "Function keys");
            ui.horizontal_wrapped(|ui| {
                let config = cx.config;
                let mut hotkeys: Vec<_> = config.ui.hotkeys.iter().collect();
                // F2 before F10: by the key's number, not its spelling.
                hotkeys.sort_by_key(|(key, _)| {
                    let n = key.get(1..).and_then(|n| n.parse::<u32>().ok());
                    (n.unwrap_or(u32::MAX), key.as_str())
                });
                for (key, command) in hotkeys {
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
                 York time. Company news (CN) is Benzinga's, through Alpaca. Option chains come from \
                 Alpaca's indicative feed on the free plan: quotes derived from OPRA's, trades 15 \
                 minutes late.",
            );
            ui.label(
                "Orders go only to the Alpaca paper account, and only when you click Confirm on a \
                 ticket (BUY, SELL, MLEG): a typed command, --run or a hotkey opens a ticket and \
                 never sends it. Every ticket checks the order against your limits ([trading] in \
                 config.toml, or SET); ORD's kill switch cancels everything and turns trading off. \
                 Every order request and answer goes to the audit log (ORD shows where).",
            );
            ui.label(
                RichText::new("For information only. Not an official MISO product, and not for operational or settlement decisions.")
                    .color(skin.text_muted),
            );
        });
    }
}
