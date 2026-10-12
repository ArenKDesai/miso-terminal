//! The Claude Messages API over [`mt_data`]: requests, the streamed answer
//! folded back into a message, and retries. There is no official Rust SDK,
//! so this speaks the HTTP API directly (`POST /v1/messages`, `stream: true`).
//!
//! [`Provider`] is the seam another provider could fill; [`Anthropic`] is the
//! only one.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};

use mt_data::sse::SseParser;
use mt_data::{BoxFuture, FetchCtx, FetchError, Request};

use crate::config::{Effort, ModelInfo};
use crate::tools::ToolSpec;
use crate::usage::Usage;

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2023-06-01";
/// Where the key lives in the secret store (`miso-terminal/anthropic/api-key`).
pub const KEY_NAME: &str = "anthropic/api-key";
/// The beta that lets a declined request be retried on Anthropic's
/// recommended model (`fallbacks: "default"`).
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Anthropic's web search, as a server tool (results come back cited).
const WEB_SEARCH_TOOL: &str = "web_search_20260209";
const WEB_SEARCHES_PER_REQUEST: u32 = 5;
/// Tries per request when the API is busy or limits the rate.
const MAX_TRIES: u32 = 4;

/// One message of a conversation: `user` or `assistant`, and its content
/// blocks exactly as the API sent or expects them (thinking blocks must go
/// back unchanged).
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Message {
    pub fn user_text(text: &str) -> Self {
        Self {
            role: Role::User,
            content: vec![json!({"type": "text", "text": text})],
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "role": match self.role { Role::User => "user", Role::Assistant => "assistant" },
            "content": self.content,
        })
    }

    /// The text blocks, joined.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect()
    }
}

/// Everything one request needs.
#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub model: ModelInfo,
    pub effort: Effort,
    pub max_tokens: u32,
    pub system: String,
    pub tools: Vec<ToolSpec>,
    pub web_search: bool,
    pub messages: Vec<Message>,
}

impl TurnRequest {
    /// The request body. The tools (in their fixed order) and the system
    /// prompt never change between questions, so a cache breakpoint after
    /// the system prompt covers both; the top-level `cache_control` caches
    /// the conversation so far, so each tool round re-reads it cheaply.
    pub fn body(&self) -> Value {
        let mut tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        if self.web_search {
            tools.push(json!({
                "type": WEB_SEARCH_TOOL,
                "name": "web_search",
                "max_uses": WEB_SEARCHES_PER_REQUEST,
            }));
        }
        let mut body = json!({
            "model": self.model.id,
            "max_tokens": self.max_tokens,
            "stream": true,
            "system": [{
                "type": "text",
                "text": self.system,
                "cache_control": {"type": "ephemeral"},
            }],
            "messages": self.messages.iter().map(Message::to_json).collect::<Vec<_>>(),
            "output_config": {"effort": self.effort.as_str()},
            "cache_control": {"type": "ephemeral"},
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
            body["tool_choice"] = json!({"type": "auto"});
        }
        if self.model.fallback {
            body["fallbacks"] = json!("default");
        }
        body
    }
}

// Tools are not sent in strict mode. Tried 2026-10-12: strict mode drops
// every optional property from Claude's input unless the schema makes it
// required and nullable, refuses `minimum`, `maxLength` and the like, and
// allows at most 16 nullable parameters across all tools (the lookups have
// 23). The full schemas go as they are, and `tools::run` checks every input
// against them before a tool runs, so a bad input comes back to Claude as an
// error result instead of reaching a tool.

/// Why the model stopped.
#[derive(Clone, Debug, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// A long server-side turn (web search) paused; send it back to go on.
    PauseTurn,
    Refusal {
        category: Option<String>,
        explanation: Option<String>,
    },
    Other(String),
}

/// One finished request: the assistant message and what it cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub message: Message,
    pub stop: StopReason,
    pub usage: Usage,
    /// The model that answered (a fallback can differ from the one asked).
    pub model: String,
}

/// What a streamed request reports while it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Delta {
    Text(String),
    /// A declined request moved to another model.
    Fallback(String),
    /// The request is being sent again (the stream so far is void).
    Retrying {
        wait: Duration,
        why: String,
    },
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AiError {
    #[error("no Anthropic API key is set")]
    NoKey,
    #[error("Anthropic rejected the request ({kind}): {message}")]
    Rejected { kind: String, message: String },
    #[error("Anthropic is busy or limiting requests ({0}); try again later")]
    Busy(String),
    #[error("the answer could not be read: {0}")]
    Protocol(String),
    #[error(transparent)]
    Fetch(#[from] FetchError),
}

pub type DeltaSink<'a> = &'a (dyn Fn(Delta) + Send + Sync);

/// Something that answers one request.
pub trait Provider: Send + Sync {
    fn turn<'a>(
        &'a self,
        req: &'a TurnRequest,
        on: DeltaSink<'a>,
    ) -> BoxFuture<'a, Result<Turn, AiError>>;
}

/// Anthropic's API, through the hub's [`FetchCtx`] (its request log, budgets
/// and secret store). The key goes only to `api.anthropic.com` over TLS, as a
/// secret header, and never through a redirect ([`mt_data::HttpTransport`]).
#[derive(Clone)]
pub struct Anthropic {
    ctx: FetchCtx,
    url: String,
    /// The first wait before a retry (doubling); tests shorten it.
    pub retry_base: Duration,
}

impl Anthropic {
    pub fn new(ctx: FetchCtx) -> Self {
        Self {
            ctx,
            url: API_URL.into(),
            retry_base: Duration::from_secs(2),
        }
    }

    /// Whether a key is stored (the panel explains how to add one if not).
    pub fn has_key(&self) -> bool {
        self.ctx.secret(KEY_NAME).is_ok()
    }

    async fn once(&self, req: &TurnRequest, on: DeltaSink<'_>) -> Result<Turn, Attempt> {
        let key = self
            .ctx
            .secret(KEY_NAME)
            .map_err(|_| Attempt::Fatal(AiError::NoKey))?;
        let mut http = Request::post(&self.url)
            .secret_header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .header("Accept", "text/event-stream")
            .json(&req.body())
            .map_err(|e| Attempt::Fatal(AiError::Protocol(e.to_string())))?;
        if req.model.fallback {
            http = http.header("anthropic-beta", FALLBACK_BETA);
        }
        let resp = self
            .ctx
            .send_streaming(http)
            .await
            .map_err(|e| Attempt::Retry(e.to_string(), None, AiError::Fetch(e)))?;
        if !resp.is_success() {
            let status = resp.status;
            let retry_after = resp
                .header("retry-after")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = resp.collect().await.map(|r| r.body).unwrap_or_default();
            let (kind, message) = api_error(&body).unwrap_or_else(|| {
                (
                    format!("HTTP {status}"),
                    String::from_utf8_lossy(&body).into(),
                )
            });
            let err = AiError::Rejected {
                kind: kind.clone(),
                message,
            };
            return Err(match status {
                401 | 403 => Attempt::Fatal(AiError::Rejected {
                    kind,
                    message: "the API key was refused; check it in SET".into(),
                }),
                408 | 409 | 429 | 500..=599 => Attempt::Retry(kind, retry_after, err),
                _ => Attempt::Fatal(err),
            });
        }
        let mut acc = Accumulator::default();
        let mut parser = SseParser::new();
        let mut body = resp.body;
        while let Some(piece) = body.next().await {
            let piece = piece.map_err(|e| Attempt::Retry(e.to_string(), None, e.into()))?;
            for ev in parser.push(&piece) {
                if let Some(done) = acc.event(&ev.event, &ev.data, on)? {
                    return Ok(done);
                }
            }
        }
        Err(Attempt::Retry(
            "the stream ended early".into(),
            None,
            AiError::Protocol("the stream ended before the answer did".into()),
        ))
    }
}

/// How one try ended, when it did not answer.
enum Attempt {
    /// Worth another try (busy, rate limited, a dropped stream), after the
    /// server's `retry-after` when it gave one.
    Retry(String, Option<Duration>, AiError),
    Fatal(AiError),
}

impl From<AiError> for Attempt {
    fn from(e: AiError) -> Self {
        Self::Fatal(e)
    }
}

impl Provider for Anthropic {
    fn turn<'a>(
        &'a self,
        req: &'a TurnRequest,
        on: DeltaSink<'a>,
    ) -> BoxFuture<'a, Result<Turn, AiError>> {
        Box::pin(async move {
            let mut tries = 0;
            loop {
                tries += 1;
                match self.once(req, on).await {
                    Ok(turn) => return Ok(turn),
                    Err(Attempt::Fatal(e)) => return Err(e),
                    Err(Attempt::Retry(why, after, e)) => {
                        if tries >= MAX_TRIES {
                            return Err(match e {
                                AiError::Rejected { kind, .. } => AiError::Busy(kind),
                                e => e,
                            });
                        }
                        let wait = after.unwrap_or(self.retry_base * 2u32.pow(tries - 1));
                        on(Delta::Retrying {
                            wait,
                            why: why.clone(),
                        });
                        tokio::time::sleep(wait).await;
                    }
                }
            }
        })
    }
}

/// `{"type":"error","error":{"type":"overloaded_error","message":"…"}}`
fn api_error(body: &[u8]) -> Option<(String, String)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let e = v.get("error")?;
    Some((
        e.get("type")?.as_str()?.to_owned(),
        e.get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    ))
}

/// Folds the stream's events into the finished message.
#[derive(Default)]
struct Accumulator {
    blocks: Vec<Value>,
    /// Tool inputs arrive as pieces of JSON text, by block index.
    partial_json: Vec<String>,
    usage: Usage,
    model: String,
    stop: Option<StopReason>,
}

impl Accumulator {
    fn event(
        &mut self,
        name: &str,
        data: &str,
        on: DeltaSink<'_>,
    ) -> Result<Option<Turn>, Attempt> {
        let v: Value = serde_json::from_str(data)
            .map_err(|e| AiError::Protocol(format!("{name} event: {e}")))?;
        let kind = v["type"].as_str().unwrap_or(name);
        match kind {
            "message_start" => {
                let m = &v["message"];
                self.model = m["model"].as_str().unwrap_or_default().to_owned();
                self.usage.merge_json(&m["usage"]);
            }
            "content_block_start" => {
                let i = index(&v)?;
                let mut block = v["content_block"].clone();
                match block["type"].as_str() {
                    Some("tool_use") => block["input"] = json!({}),
                    Some("fallback") => {
                        let to = block["model"].as_str().unwrap_or("another model");
                        on(Delta::Fallback(to.to_owned()));
                    }
                    _ => {}
                }
                if self.blocks.len() <= i {
                    self.blocks.resize(i + 1, Value::Null);
                    self.partial_json.resize(i + 1, String::new());
                }
                self.blocks[i] = block;
            }
            "content_block_delta" => {
                let i = index(&v)?;
                let d = &v["delta"];
                let block = self.blocks.get_mut(i).ok_or_else(|| {
                    AiError::Protocol(format!("a delta for block {i} came first"))
                })?;
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let t = d["text"].as_str().unwrap_or_default();
                        append(block, "text", t);
                        on(Delta::Text(t.to_owned()));
                    }
                    Some("input_json_delta") => {
                        self.partial_json[i].push_str(d["partial_json"].as_str().unwrap_or(""));
                    }
                    Some("thinking_delta") => {
                        append(block, "thinking", d["thinking"].as_str().unwrap_or(""));
                    }
                    Some("signature_delta") => {
                        block["signature"] = d["signature"].clone();
                    }
                    Some("citations_delta") => {
                        let c = &mut block["citations"];
                        if !c.is_array() {
                            *c = json!([]);
                        }
                        if let Some(list) = c.as_array_mut() {
                            list.push(d["citation"].clone());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let i = index(&v)?;
                if let Some(block) = self.blocks.get_mut(i)
                    && block["type"] == "tool_use"
                {
                    let text = self.partial_json[i].trim();
                    if !text.is_empty() {
                        // An unparsable input is kept as text; the loop
                        // answers it with an error result rather than running it.
                        block["input"] = serde_json::from_str(text)
                            .unwrap_or_else(|_| json!({"__unparsed": text}));
                    }
                }
            }
            "message_delta" => {
                self.usage.merge_json(&v["usage"]);
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(stop_reason(r, &v["delta"]["stop_details"]));
                }
            }
            "message_stop" => {
                return Ok(Some(Turn {
                    message: Message {
                        role: Role::Assistant,
                        content: std::mem::take(&mut self.blocks)
                            .into_iter()
                            .filter(|b| !b.is_null())
                            .collect(),
                    },
                    stop: self
                        .stop
                        .take()
                        .unwrap_or_else(|| StopReason::Other("none".into())),
                    usage: self.usage,
                    model: std::mem::take(&mut self.model),
                }));
            }
            "error" => {
                let kind = v["error"]["type"].as_str().unwrap_or("error").to_owned();
                let message = v["error"]["message"].as_str().unwrap_or("").to_owned();
                let err = AiError::Rejected {
                    kind: kind.clone(),
                    message,
                };
                return Err(match kind.as_str() {
                    "overloaded_error" | "rate_limit_error" | "api_error" => {
                        Attempt::Retry(kind, None, err)
                    }
                    _ => Attempt::Fatal(err),
                });
            }
            // `ping` and anything newer.
            _ => {}
        }
        Ok(None)
    }
}

fn index(v: &Value) -> Result<usize, Attempt> {
    v["index"]
        .as_u64()
        .map(|i| i as usize)
        .ok_or_else(|| AiError::Protocol("an event without an index".into()).into())
}

fn append(block: &mut Value, field: &str, text: &str) {
    let mut s = block[field].as_str().unwrap_or_default().to_owned();
    s.push_str(text);
    block[field] = Value::String(s);
}

fn stop_reason(r: &str, details: &Value) -> StopReason {
    match r {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "pause_turn" => StopReason::PauseTurn,
        "refusal" => StopReason::Refusal {
            category: details["category"].as_str().map(str::to_owned),
            explanation: details["explanation"].as_str().map(str::to_owned),
        },
        other => StopReason::Other(other.to_owned()),
    }
}
