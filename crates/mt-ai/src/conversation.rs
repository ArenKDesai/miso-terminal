//! The loop that answers one question: send it with the tools; while Claude
//! calls tools, run them (all of one round at once) and send every result
//! back in one message; stop at the answer, or at a cap on rounds, tokens or
//! money.

use std::time::Duration;

use futures_util::future::join_all;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::api::{AiError, Delta, Message, Provider, Role, StopReason, Turn, TurnRequest};
use crate::config::AiConfig;
use crate::tools::{self, ToolBox};
use crate::usage::Usage;

/// The longest tool result sent back, in characters; longer ones are cut
/// with a note, so one lookup cannot fill the context.
const MAX_RESULT_CHARS: usize = 60_000;

/// What the panel shows while a question is answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// More of the answer's text.
    Text(String),
    /// Drop the text shown since the last round: the request is being retried.
    Restart {
        wait: Duration,
        why: String,
    },
    ToolStarted {
        id: String,
        name: String,
        input: Value,
    },
    ToolFinished {
        id: String,
        ok: bool,
        /// The result (or the error) as sent to Claude.
        output: String,
    },
    /// One request finished.
    Round {
        number: u32,
        usage: Usage,
        cost_usd: f64,
    },
    Fallback {
        model: String,
    },
}

/// How a question ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Ending {
    Answered,
    /// The answer reached `max_tokens` and is cut off.
    CutOff,
    Refused {
        category: Option<String>,
        explanation: Option<String>,
    },
    RoundLimit,
    TokenLimit,
    /// The month's spending cap is reached (or is zero).
    CapReached {
        spent_usd: f64,
        cap_usd: f64,
    },
    Cancelled,
    Failed(AiError),
}

/// The result of [`ask`].
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub ending: Ending,
    /// The final answer's text (partial when cut off).
    pub answer: String,
    pub usage: Usage,
    pub cost_usd: f64,
    pub rounds: u32,
}

/// A conversation kept between questions (in memory unless saved). Every
/// assistant message in it is complete: a question that fails or is
/// cancelled leaves it as it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conversation {
    pub messages: Vec<Message>,
}

/// Lets the panel stop a question. Cancelling drops the request in flight;
/// tools already running finish on their own and are ignored.
#[derive(Clone)]
pub struct Cancel(watch::Sender<bool>);

impl Default for Cancel {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl Cancel {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }
}

/// What a question may spend: the cap and what the month has used before it.
#[derive(Clone, Copy, Debug)]
pub struct Allowance {
    pub cap_usd: f64,
    pub spent_usd: f64,
}

/// Answer `question`, extending `conv` with it and the answer.
#[allow(clippy::too_many_arguments)]
pub async fn ask(
    provider: &dyn Provider,
    tools: &dyn ToolBox,
    config: &AiConfig,
    system: &str,
    allowance: Allowance,
    conv: &mut Conversation,
    question: &str,
    cancel: &Cancel,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> Outcome {
    let model = config.model_info();
    let mut messages = conv.messages.clone();
    messages.push(Message::user_text(question));
    let mut out = Outcome {
        ending: Ending::Answered,
        answer: String::new(),
        usage: Usage::default(),
        cost_usd: 0.0,
        rounds: 0,
    };
    let specs = tools.specs();
    let on_delta = |d: Delta| match d {
        Delta::Text(t) => progress(Progress::Text(t)),
        Delta::Fallback(model) => progress(Progress::Fallback { model }),
        Delta::Retrying { wait, why } => progress(Progress::Restart { wait, why }),
    };
    loop {
        let spent = allowance.spent_usd + out.cost_usd;
        if allowance.cap_usd <= 0.0 || spent >= allowance.cap_usd {
            out.ending = Ending::CapReached {
                spent_usd: spent,
                cap_usd: allowance.cap_usd,
            };
            return out;
        }
        if out.rounds >= config.max_rounds {
            out.ending = Ending::RoundLimit;
            break;
        }
        if out.usage.total_tokens() >= config.max_question_tokens {
            out.ending = Ending::TokenLimit;
            break;
        }
        let req = TurnRequest {
            model: model.clone(),
            effort: config.effort,
            max_tokens: config.max_tokens,
            system: system.to_owned(),
            tools: specs.clone(),
            web_search: config.web_search,
            messages: messages.clone(),
        };
        let turn: Turn = tokio::select! {
            r = provider.turn(&req, &on_delta) => match r {
                Ok(t) => t,
                Err(e) => {
                    out.ending = Ending::Failed(e);
                    return out;
                }
            },
            () = cancel.cancelled() => {
                out.ending = Ending::Cancelled;
                return out;
            }
        };
        out.rounds += 1;
        let cost = turn.usage.cost(&model);
        out.usage.add(&turn.usage);
        out.cost_usd += cost;
        progress(Progress::Round {
            number: out.rounds,
            usage: turn.usage,
            cost_usd: cost,
        });
        match turn.stop {
            StopReason::ToolUse => {
                let calls: Vec<(String, String, Value)> = turn
                    .message
                    .content
                    .iter()
                    .filter(|b| b["type"] == "tool_use")
                    .map(|b| {
                        (
                            b["id"].as_str().unwrap_or_default().to_owned(),
                            b["name"].as_str().unwrap_or_default().to_owned(),
                            b["input"].clone(),
                        )
                    })
                    .collect();
                messages.push(turn.message);
                for (id, name, input) in &calls {
                    progress(Progress::ToolStarted {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                let runs = calls.iter().map(|(_, name, input)| async move {
                    if input.get("__unparsed").is_some() {
                        return Err("the input was not valid JSON".to_owned());
                    }
                    tools::run(tools, name, input.clone()).await
                });
                let results = tokio::select! {
                    r = join_all(runs) => r,
                    () = cancel.cancelled() => {
                        out.ending = Ending::Cancelled;
                        return out;
                    }
                };
                let mut blocks = Vec::with_capacity(calls.len());
                for ((id, _, _), result) in calls.iter().zip(results) {
                    let (ok, text) = match result {
                        Ok(v) => (true, cut(v.to_string())),
                        Err(e) => (false, e),
                    };
                    progress(Progress::ToolFinished {
                        id: id.clone(),
                        ok,
                        output: text.clone(),
                    });
                    let mut block =
                        json!({"type": "tool_result", "tool_use_id": id, "content": text});
                    if !ok {
                        block["is_error"] = json!(true);
                    }
                    blocks.push(block);
                }
                messages.push(Message {
                    role: Role::User,
                    content: blocks,
                });
            }
            StopReason::PauseTurn => messages.push(turn.message),
            StopReason::EndTurn | StopReason::Other(_) => {
                out.answer = turn.message.text();
                messages.push(turn.message);
                break;
            }
            StopReason::MaxTokens => {
                out.answer = turn.message.text();
                out.ending = Ending::CutOff;
                messages.push(turn.message);
                break;
            }
            StopReason::Refusal {
                category,
                explanation,
            } => {
                // The question is dropped with the refusal, so the next
                // question does not carry it.
                out.ending = Ending::Refused {
                    category,
                    explanation,
                };
                return out;
            }
        }
    }
    // A cap reached mid-question leaves tool results waiting for an answer;
    // the conversation keeps them, and the next question follows them.
    if matches!(out.ending, Ending::RoundLimit | Ending::TokenLimit)
        && messages.last().is_some_and(|m| m.role == Role::User)
    {
        messages.push(Message {
            role: Role::Assistant,
            content: vec![json!({"type": "text", "text": "(stopped: limit reached)"})],
        });
    }
    conv.messages = messages;
    out
}

fn cut(mut s: String) -> String {
    if let Some((i, _)) = s.char_indices().nth(MAX_RESULT_CHARS) {
        s.truncate(i);
        s.push_str("… [cut: the result was too long; ask for a narrower window]");
    }
    s
}
