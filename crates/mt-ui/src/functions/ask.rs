//! ASK: questions about the market, answered by Claude with the terminal's
//! own lookups (`assistant::Lookups`). The answer streams in as Markdown,
//! the lookups show as they run, and the commands it names in backticks
//! are links that open the panels showing the same data, when clicked.
//!
//! In the app this needs an Anthropic API key (SET) and a monthly spending
//! cap above $0; until then the panel explains that and the other route,
//! Claude Desktop or Claude Code with `miso-terminal --mcp`, which uses a
//! Claude plan.

use std::sync::Arc;

use egui::{RichText, ScrollArea, Ui};
use parking_lot::Mutex;

use mt_ai::{Allowance, Anthropic, Cancel, Conversation, Ending, Progress, Usage};

use crate::assistant::{Lookups, ledger, prompt};
use crate::context::{AppCommand, PanelCx};
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ASK",
    aliases: &["AI", "CLAUDE"],
    name: "Ask Claude",
    category: Category::Overview,
    usage: "ASK [question]",
    description: "Ask Claude about MISO and the markets: it looks things up with the terminal's data (prices, constraints, reserves, load, weather, gas, news, forecasts) and shows its lookups and the commands to check them. Needs an Anthropic API key and a monthly cap (SET), or use Claude Desktop with --mcp.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

/// Functions an answer's commands may never open: the order tickets and
/// the order list. Everything else only shows data.
const NEVER_OPENED: &[&str] = &["BUY", "SELL", "MLEG", "ORD"];

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Ask {
        input: args.join(" "),
        ask_now: !args.is_empty(),
        ..Ask::default()
    }))
}

#[derive(Default)]
struct Ask {
    input: String,
    /// Opened with a question: ask it on the first frame (if allowed).
    ask_now: bool,
    conversation: Conversation,
    turns: Vec<Turn>,
    running: Option<Running>,
}

/// One question and its answer, as shown.
#[derive(Default)]
struct Turn {
    question: String,
    live: Arc<Mutex<Live>>,
}

/// What the background task has reported so far.
#[derive(Default)]
struct Live {
    text: String,
    lookups: Vec<Lookup>,
    usage: Usage,
    cost_usd: f64,
    note: Option<String>,
    ending: Option<Ending>,
}

struct Lookup {
    id: String,
    name: String,
    input: String,
    /// `None` while running; `Some(ok)` once done.
    done: Option<bool>,
}

struct Running {
    cancel: Cancel,
    /// The conversation the task hands back when the question ends.
    result: Arc<Mutex<Option<Conversation>>>,
}

impl Panel for Ask {
    fn title(&self) -> String {
        "ASK".into()
    }

    fn route(&self) -> Route {
        // A restart reopens the panel, never re-asks (or re-pays for) a question.
        Route::code("ASK")
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        if self.running.is_some() || args.is_empty() {
            return args.is_empty();
        }
        self.input = args.join(" ");
        self.ask_now = true;
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        self.collect();
        let has_key = Anthropic::new(cx.hub.ctx().clone()).has_key();
        let cap = cx.config.ai.monthly_cap_usd;
        let ledger_path = ledger::path(&cx.paths.state_file);
        widgets::title_bar(ui, skin, "Ask Claude", |ui| {
            let spent = ledger::spent_this_month(&ledger_path);
            let text = if spent.is_finite() {
                format!("${spent:.2} of ${cap:.2} this month")
            } else {
                "usage ledger unreadable".into()
            };
            ui.label(RichText::new(text).small().color(skin.text_muted));
            if self.running.is_none() && !self.turns.is_empty() && ui.small_button("New").clicked()
            {
                self.conversation = Conversation::default();
                self.turns.clear();
            }
        });
        // A key that is refused, or an account without credits, fails every
        // question: say so before one is asked (checked for free).
        let key_status = (has_key && cx.hub.ctx().is_live())
            .then(|| {
                cx.hub.watch(&mt_ai::KeyCheckQuery {
                    model: cx.config.ai.model.clone(),
                })
            })
            .and_then(|s| s.data);
        let blocked = key_status.as_deref().filter(|s| s.blocks()).cloned();
        let ready = has_key && cap > 0.0 && cx.hub.ctx().is_live() && blocked.is_none();
        if let Some(status) = &blocked {
            ui.horizontal(|ui| {
                ui.label(RichText::new(status.describe()).color(skin.negative));
                if ui.button("Open SET").clicked() {
                    cx.send(AppCommand::Open(Route::code("SET")));
                }
            });
        } else if !ready {
            routes_help(ui, cx, has_key, cap);
        }

        // The question box.
        let mut ask = false;
        ui.horizontal(|ui| {
            let field = ui.add_enabled(
                self.running.is_none(),
                egui::TextEdit::singleline(&mut self.input)
                    .hint_text("Why did RT spike at INDIANA.HUB yesterday?")
                    .desired_width(ui.available_width() - 90.0),
            );
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(run) = &self.running {
                if ui.button("Stop").clicked() {
                    run.cancel.cancel();
                }
            } else if ui
                .add_enabled(
                    ready && !self.input.trim().is_empty(),
                    egui::Button::new("Ask"),
                )
                .clicked()
                || enter
            {
                ask = true;
            }
        });
        ui.label(
            RichText::new(if cx.config.ai.share_paper_account {
                "Sends your question and the lookups' results (public market data, and the \
                 paper account's positions) to Anthropic. Never keys, the order audit log or \
                 your files. See the privacy policy."
            } else {
                "Sends your question and the lookups' results (public market data) to \
                 Anthropic. Never keys, the paper account, the order audit log or your files. \
                 See the privacy policy."
            })
            .small()
            .color(skin.text_muted),
        );
        if (ask || std::mem::take(&mut self.ask_now)) && ready && self.running.is_none() {
            let question = self.input.trim().to_owned();
            if !question.is_empty() {
                self.input.clear();
                self.start(cx, ui.ctx().clone(), question);
            }
        }

        ui.separator();
        ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (i, turn) in self.turns.iter().enumerate() {
                    show_turn(ui, cx, turn, i);
                    ui.add_space(10.0);
                }
            });
    }
}

impl Ask {
    /// Take back the conversation from a finished question.
    fn collect(&mut self) {
        let finished = self.running.as_ref().and_then(|r| r.result.lock().take());
        if let Some(conv) = finished {
            self.conversation = conv;
            self.running = None;
        }
    }

    fn start(&mut self, cx: &PanelCx<'_>, ctx: egui::Context, question: String) {
        let live = Arc::new(Mutex::new(Live::default()));
        self.turns.push(Turn {
            question: question.clone(),
            live: live.clone(),
        });
        let cancel = Cancel::default();
        let result = Arc::new(Mutex::new(None));
        self.running = Some(Running {
            cancel: cancel.clone(),
            result: result.clone(),
        });
        let hub = cx.hub.clone();
        let config = cx.config.ai.clone();
        let lookups = Lookups::new(hub.clone(), cx.config);
        let ledger_path = ledger::path(&cx.paths.state_file);
        let mut conversation = self.conversation.clone();
        let now = mt_core::time::now_market();
        let model = config.model_info();
        hub.runtime().clone().spawn(async move {
            let client = Anthropic::new(hub.ctx().clone());
            let events = hub.ctx().events().clone();
            let allowance = Allowance {
                cap_usd: config.monthly_cap_usd,
                spent_usd: ledger::spent_this_month(&ledger_path),
            };
            let progress = |p: Progress| {
                {
                    let mut l = live.lock();
                    match p {
                        Progress::Text(t) => l.text.push_str(&t),
                        Progress::Restart { wait, why } => {
                            l.text.clear();
                            l.note = Some(format!("{why}; trying again in {} s", wait.as_secs()));
                        }
                        Progress::ToolStarted { id, name, input } => {
                            events.info(format!("ASK lookup {name} {input}"));
                            l.lookups.push(Lookup {
                            id,
                            name,
                                input: input.to_string(),
                                done: None,
                            });
                        }
                        Progress::ToolFinished { id, ok, output } => {
                            if !ok {
                                events.warn(format!("ASK lookup failed: {output}"));
                            }
                            if let Some(x) = l.lookups.iter_mut().find(|x| x.id == id) {
                                x.done = Some(ok);
                            }
                        }
                        Progress::Round { number, usage, cost_usd } => {
                            l.usage.add(&usage);
                            l.cost_usd += cost_usd;
                            // Text before a tool round is narration, not the answer.
                            l.text.clear();
                            let month = ledger::record(&ledger_path, cost_usd).ok();
                            events.info(format!(
                                "ASK round {number} on {}: {} input ({} cached, {} written to the cache), {} output, ${cost_usd:.4}{}",
                                model.id,
                                usage.input_tokens,
                                usage.cache_read_input_tokens,
                                usage.cache_creation_input_tokens,
                                usage.output_tokens,
                                month.map_or_else(String::new, |m| format!("; ${m:.2} this month")),
                            ));
                        }
                        Progress::Fallback { model } => {
                            l.note = Some(format!("answered by {model} (the first model declined)"));
                        }
                    }
                }
                ctx.request_repaint();
            };
            let asked = format!("{}\n\n{question}", prompt::question_context(now));
            let out = mt_ai::ask(
                &client,
                &lookups,
                &config,
                prompt::SYSTEM,
                allowance,
                &mut conversation,
                &asked,
                &cancel,
                &progress,
            )
            .await;
            {
                let mut l = live.lock();
                if !out.answer.is_empty() {
                    l.text = out.answer.clone();
                }
                l.ending = Some(out.ending);
            }
            *result.lock() = Some(conversation);
            ctx.request_repaint();
        });
    }
}

fn routes_help(ui: &mut Ui, cx: &mut PanelCx<'_>, has_key: bool, cap: f64) {
    let skin = cx.skin;
    if !cx.hub.ctx().is_live() {
        ui.label(RichText::new("ASK is off while replaying recorded data.").color(skin.warning));
        return;
    }
    ui.label(RichText::new("Two ways to ask Claude about the market:").strong());
    ui.label(
        "• With your Claude plan: in Claude Desktop or Claude Code, add this terminal as an \
         MCP server (miso-terminal --mcp; SET shows the setup). Nothing extra to pay.",
    );
    ui.label(
        "• Here, with an Anthropic API key, paid per question: add the key in SET and raise \
         the monthly cap above $0.",
    );
    let missing = match (has_key, cap > 0.0) {
        (false, false) => "No API key yet, and the monthly cap is $0.",
        (false, true) => "No API key yet.",
        (true, false) => "The monthly cap is $0, so nothing is sent.",
        (true, true) => "",
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(missing).color(skin.warning));
        if ui.button("Open SET").clicked() {
            cx.send(AppCommand::Open(Route::code("SET")));
        }
    });
    ui.separator();
}

fn show_turn(ui: &mut Ui, cx: &mut PanelCx<'_>, turn: &Turn, index: usize) {
    let skin = cx.skin;
    ui.label(
        RichText::new(format!("› {}", turn.question))
            .strong()
            .color(skin.accent),
    );
    let live = turn.live.lock();
    for l in &live.lookups {
        let (mark, color) = match l.done {
            None => ("…", skin.text_muted),
            Some(true) => ("✓", skin.live),
            Some(false) => ("✗", skin.negative),
        };
        let input: String = l.input.chars().take(120).collect();
        ui.label(
            RichText::new(format!("{mark} {} {input}", l.name))
                .small()
                .monospace()
                .color(color),
        );
    }
    if let Some(note) = &live.note {
        ui.label(RichText::new(note).small().color(skin.warning));
    }
    if live.text.is_empty() && live.ending.is_none() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("thinking…").color(skin.text_muted));
        });
    } else {
        let clicked = {
            let cx_ref: &PanelCx<'_> = cx;
            widgets::markdown::show(ui, skin, &format!("ask-{index}"), &live.text, |code| {
                is_command(code, cx_ref)
            })
        };
        if let Some(cmd) = clicked {
            cx.send(AppCommand::Run(cmd.trim().to_owned()));
        }
    }
    if let Some(ending) = &live.ending
        && let Some(text) = ending_text(ending)
    {
        ui.label(RichText::new(text).color(skin.warning));
    }
    if live.usage.total_tokens() > 0 {
        ui.label(
            RichText::new(format!(
                "{} in ({} cached) · {} out · ${:.4}",
                live.usage.input_tokens + live.usage.cache_creation_input_tokens,
                live.usage.cache_read_input_tokens,
                live.usage.output_tokens,
                live.cost_usd
            ))
            .small()
            .color(skin.text_muted),
        );
    }
}

fn ending_text(e: &Ending) -> Option<String> {
    Some(match e {
        Ending::Answered => return None,
        Ending::CutOff => "The answer reached its length limit and is cut off.".into(),
        Ending::Refused {
            category,
            explanation,
        } => format!(
            "Claude declined to answer{}{}.",
            category
                .as_ref()
                .map_or_else(String::new, |c| format!(" ({c})")),
            explanation
                .as_ref()
                .map_or_else(String::new, |e| format!(": {e}")),
        ),
        Ending::RoundLimit => "Stopped: the question used its lookups' limit (SET).".into(),
        Ending::TokenLimit => "Stopped: the question used its token limit (SET).".into(),
        Ending::CapReached { spent_usd, cap_usd } => format!(
            "Not sent: this month's cap is ${cap_usd:.2} and ${spent_usd:.2} is spent (SET)."
        ),
        Ending::Cancelled => "Stopped.".into(),
        Ending::Failed(e) => e.to_string(),
    })
}

/// Whether a code span in an answer is a command to offer as a link: a
/// known function, never a trading one (`GP INDIANA.HUB 2`, `BCH 2026-10-06`).
fn is_command(code: &str, cx: &PanelCx<'_>) -> bool {
    let cmd = code.trim();
    if cmd.is_empty() || cmd.len() > 60 || cmd.contains('\n') {
        return false;
    }
    let first = cmd.split_whitespace().next().unwrap_or_default();
    match cx.registry.find(first) {
        Some(spec) => !NEVER_OPENED.contains(&spec.code),
        None => false,
    }
}
