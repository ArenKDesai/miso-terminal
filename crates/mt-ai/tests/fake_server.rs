//! The assistant's loop against a fake Messages API: streamed text, tool
//! rounds with parallel calls, pause_turn, refusals, max_tokens, retries,
//! caps and cancelling. Nothing here reaches the network.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::{Value, json};

use mt_ai::{
    AiConfig, Allowance, Anthropic, Cancel, Conversation, Ending, Progress, Role, ToolBox,
    ToolResult, ToolSpec, ask,
};
use mt_data::{
    BoxFuture, EventLog, FetchCtx, FetchCtxOptions, FetchError, MemorySecrets, Request, Response,
    Secret, Transport,
};

/// Answers from a script (in order) and records what it was asked.
#[derive(Default)]
struct FakeApi {
    answers: Mutex<Vec<Response>>,
    asked: Mutex<Vec<Request>>,
}

impl FakeApi {
    fn with(answers: Vec<Response>) -> Arc<Self> {
        let mut answers = answers;
        answers.reverse();
        Arc::new(Self {
            answers: Mutex::new(answers),
            asked: Mutex::default(),
        })
    }

    fn bodies(&self) -> Vec<Value> {
        self.asked
            .lock()
            .iter()
            .map(|r| serde_json::from_slice(r.body.as_deref().unwrap_or(b"{}")).expect("json"))
            .collect()
    }
}

impl Transport for FakeApi {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        self.asked.lock().push(req.clone());
        let next = self.answers.lock().pop();
        Box::pin(async move { next.ok_or_else(|| FetchError::Network("script ended".into())) })
    }
    fn describe(&self) -> String {
        "fake Anthropic".into()
    }
}

/// A streamed answer, written as the API writes it.
fn sse(events: &[Value]) -> Response {
    let mut body = String::new();
    for e in events {
        body.push_str(&format!(
            "event: {}\ndata: {e}\n\n",
            e["type"].as_str().unwrap_or("")
        ));
        if e["type"] == "message_start" {
            body.push_str("event: ping\ndata: {\"type\": \"ping\"}\n\n");
        }
    }
    Response::ok(Bytes::from(body))
}

fn start(input: u64, cache_read: u64) -> Value {
    json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5",
        "usage": {"input_tokens": input, "cache_read_input_tokens": cache_read,
                  "cache_creation_input_tokens": 0, "output_tokens": 1}}})
}

fn stop(reason: &str, output: u64) -> [Value; 2] {
    [
        json!({"type": "message_delta", "delta": {"stop_reason": reason, "stop_details": null},
               "usage": {"output_tokens": output}}),
        json!({"type": "message_stop"}),
    ]
}

/// A text answer in pieces.
fn text_answer(pieces: &[&str], reason: &str) -> Response {
    let mut ev = vec![
        start(100, 2000),
        json!({"type": "content_block_start", "index": 0,
               "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 0,
               "delta": {"type": "signature_delta", "signature": "sig=="}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1,
               "content_block": {"type": "text", "text": ""}}),
    ];
    for p in pieces {
        ev.push(json!({"type": "content_block_delta", "index": 1,
                       "delta": {"type": "text_delta", "text": p}}));
    }
    ev.push(json!({"type": "content_block_stop", "index": 1}));
    ev.extend(stop(reason, 50));
    sse(&ev)
}

/// Two tool calls in one message, their inputs split mid-JSON.
fn two_tool_calls() -> Response {
    let mut ev = vec![start(300, 0)];
    for (i, (id, input)) in [
        ("tu_1", "{\"node\": \"INDIANA.HUB\"}"),
        ("tu_2", "{\"node\": \"MINN.HUB\"}"),
    ]
    .iter()
    .enumerate()
    {
        ev.push(json!({"type": "content_block_start", "index": i,
            "content_block": {"type": "tool_use", "id": id, "name": "lmp", "input": {}}}));
        let (a, b) = input.split_at(7);
        for part in [a, b] {
            ev.push(json!({"type": "content_block_delta", "index": i,
                "delta": {"type": "input_json_delta", "partial_json": part}}));
        }
        ev.push(json!({"type": "content_block_stop", "index": i}));
    }
    ev.extend(stop("tool_use", 40));
    sse(&ev)
}

fn status(code: u16, headers: &[(&str, &str)], body: &str) -> Response {
    Response {
        status: code,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        body: Bytes::from(body.to_owned()),
    }
}

/// One tool, `lmp`, answering from a table; it records each call.
#[derive(Default)]
struct Prices {
    calls: Mutex<Vec<Value>>,
}

impl ToolBox for Prices {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "lmp".into(),
            description: "A node's latest price".into(),
            input_schema: json!({"type": "object", "properties": {"node": {"type": "string"}},
                "required": ["node"], "additionalProperties": false}),
        }]
    }
    fn call<'a>(&'a self, _name: &'a str, input: Value) -> BoxFuture<'a, ToolResult> {
        self.calls.lock().push(input.clone());
        Box::pin(async move {
            match input["node"].as_str() {
                Some("INDIANA.HUB") => Ok(json!({"lmp_usd_mwh": 41.5})),
                Some(n) => Err(format!("no prices for {n}")),
                None => Err("no node".into()),
            }
        })
    }
}

struct Setup {
    api: Arc<FakeApi>,
    client: Anthropic,
    events: EventLog,
}

fn setup(answers: Vec<Response>) -> Setup {
    let api = FakeApi::with(answers);
    let events = EventLog::default();
    let secrets = Arc::new(MemorySecrets::with([(
        mt_ai::api::KEY_NAME,
        Secret::new("sk-ant-test-123"),
    )]));
    let ctx = FetchCtx::new(
        api.clone(),
        None,
        FetchCtxOptions {
            secrets,
            ..FetchCtxOptions::default()
        },
        events.clone(),
    );
    let mut client = Anthropic::new(ctx);
    client.retry_base = Duration::from_millis(1);
    Setup {
        api,
        client,
        events,
    }
}

const PLENTY: Allowance = Allowance {
    cap_usd: 5.0,
    spent_usd: 0.0,
};

async fn run(
    s: &Setup,
    tools: &dyn ToolBox,
    config: &AiConfig,
    conv: &mut Conversation,
    question: &str,
) -> (mt_ai::Outcome, Vec<Progress>) {
    let seen = Mutex::new(Vec::new());
    let out = ask(
        &s.client,
        tools,
        config,
        "You answer questions about MISO.",
        PLENTY,
        conv,
        question,
        &Cancel::default(),
        &|p| seen.lock().push(p),
    )
    .await;
    (out, seen.into_inner())
}

#[tokio::test]
async fn a_streamed_answer_and_the_request_it_came_from() {
    let s = setup(vec![text_answer(
        &["RT hit ", "$10,000 ", "at 14:05."],
        "end_turn",
    )]);
    let mut conv = Conversation::default();
    let (out, seen) = run(
        &s,
        &Prices::default(),
        &AiConfig::default(),
        &mut conv,
        "Why?",
    )
    .await;
    assert_eq!(out.ending, Ending::Answered);
    assert_eq!(out.answer, "RT hit $10,000 at 14:05.");
    let texts: Vec<_> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["RT hit ", "$10,000 ", "at 14:05."]);
    // 100 in × $4 + 2000 cached × $0.20 + 50 out × $20, per million.
    assert!((out.cost_usd - (400.0 + 400.0 + 1000.0) / 1e6).abs() < 1e-12);

    let asked = s.api.asked.lock()[0].clone();
    assert_eq!(asked.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(
        asked.header_value("x-api-key").map(|v| v.is_secret()),
        Some(true)
    );
    assert_eq!(
        asked
            .header_value("anthropic-beta")
            .map(|v| v.expose().to_owned()),
        Some("server-side-fallback-2026-07-01".into())
    );
    let body = &s.api.bodies()[0];
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["output_config"]["effort"], "low");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["cache_control"]["type"], "ephemeral");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(
        body["tools"][0].get("strict").is_none(),
        "see api.rs on strict mode"
    );
    assert_eq!(body["tool_choice"]["type"], "auto");
    assert!(body.get("thinking").is_none(), "Opus 5.5 thinks adaptively");
    let logged: String = s
        .events
        .recent(10)
        .iter()
        .map(|e| e.message.clone())
        .collect();
    assert!(!logged.contains("sk-ant-test-123"), "{logged}");

    // The conversation keeps the thinking block, signature and all.
    assert_eq!(conv.messages.len(), 2);
    assert_eq!(conv.messages[1].content[0]["signature"], "sig==");
}

#[tokio::test]
async fn tool_calls_run_together_and_answer_in_one_message() {
    let s = setup(vec![
        two_tool_calls(),
        text_answer(&["Indiana was $41.50."], "end_turn"),
    ]);
    let tools = Prices::default();
    let mut conv = Conversation::default();
    let (out, seen) = run(&s, &tools, &AiConfig::default(), &mut conv, "Prices?").await;
    assert_eq!(out.ending, Ending::Answered);
    assert_eq!(out.rounds, 2);
    assert_eq!(tools.calls.lock().len(), 2);

    let second = &s.api.bodies()[1];
    let msgs = second["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3, "question, tool calls, all results together");
    assert_eq!(msgs[1]["content"][0]["input"]["node"], "INDIANA.HUB");
    let results = msgs[2]["content"].as_array().expect("results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["tool_use_id"], "tu_1");
    assert_eq!(results[0]["content"], "{\"lmp_usd_mwh\":41.5}");
    assert!(results[0].get("is_error").is_none());
    assert_eq!(results[1]["is_error"], true);
    assert_eq!(results[1]["content"], "no prices for MINN.HUB");

    let finished = seen
        .iter()
        .filter(|p| matches!(p, Progress::ToolFinished { .. }))
        .count();
    assert_eq!(finished, 2);
    assert_eq!(conv.messages.len(), 4);
}

#[tokio::test]
async fn invalid_tool_input_is_answered_without_running_the_tool() {
    let bad = sse(&[
        start(10, 0),
        json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "tu_9", "name": "lmp", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{\"node\": 5}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        stop("tool_use", 5)[0].clone(),
        stop("tool_use", 5)[1].clone(),
    ]);
    let s = setup(vec![bad, text_answer(&["Sorry."], "end_turn")]);
    let tools = Prices::default();
    let (out, _) = run(
        &s,
        &tools,
        &AiConfig::default(),
        &mut Conversation::default(),
        "?",
    )
    .await;
    assert_eq!(out.ending, Ending::Answered);
    assert!(tools.calls.lock().is_empty());
    let result = &s.api.bodies()[1]["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
    assert_eq!(
        result["content"],
        "invalid input: input.node should be string"
    );
}

#[tokio::test]
async fn pause_turn_goes_back_without_a_new_question() {
    let s = setup(vec![
        text_answer(&["Searching…"], "pause_turn"),
        text_answer(&["Found it."], "end_turn"),
    ]);
    let (out, _) = run(
        &s,
        &Prices::default(),
        &AiConfig::default(),
        &mut Conversation::default(),
        "?",
    )
    .await;
    assert_eq!(out.answer, "Found it.");
    let second = &s.api.bodies()[1]["messages"];
    assert_eq!(second.as_array().map(Vec::len), Some(2));
    assert_eq!(second[1]["role"], "assistant");
}

#[tokio::test]
async fn refusals_cut_offs_and_limits() {
    // A refusal leaves the conversation as it was.
    let refusal = sse(&[
        start(10, 0),
        json!({"type": "message_delta", "delta": {"stop_reason": "refusal",
            "stop_details": {"type": "refusal", "category": "cyber", "explanation": "no"}},
            "usage": {"output_tokens": 0}}),
        json!({"type": "message_stop"}),
    ]);
    let s = setup(vec![refusal]);
    let mut conv = Conversation::default();
    let (out, _) = run(&s, &Prices::default(), &AiConfig::default(), &mut conv, "?").await;
    assert_eq!(
        out.ending,
        Ending::Refused {
            category: Some("cyber".into()),
            explanation: Some("no".into())
        }
    );
    assert!(conv.messages.is_empty());

    let s = setup(vec![text_answer(&["The answer is"], "max_tokens")]);
    let (out, _) = run(&s, &Prices::default(), &AiConfig::default(), &mut conv, "?").await;
    assert_eq!(
        (out.ending, out.answer.as_str()),
        (Ending::CutOff, "The answer is")
    );

    // Two rounds allowed; the model keeps calling tools.
    let s = setup(vec![two_tool_calls(), two_tool_calls(), two_tool_calls()]);
    let config = AiConfig {
        max_rounds: 2,
        ..AiConfig::default()
    };
    let mut conv = Conversation::default();
    let (out, _) = run(&s, &Prices::default(), &config, &mut conv, "?").await;
    assert_eq!((out.ending, out.rounds), (Ending::RoundLimit, 2));
    assert_eq!(conv.messages.last().map(|m| m.role), Some(Role::Assistant));

    let s = setup(vec![two_tool_calls(), two_tool_calls()]);
    let config = AiConfig {
        max_question_tokens: 100,
        ..AiConfig::default()
    };
    let (out, _) = run(
        &s,
        &Prices::default(),
        &config,
        &mut Conversation::default(),
        "?",
    )
    .await;
    assert_eq!((out.ending, out.rounds), (Ending::TokenLimit, 1));
}

#[tokio::test]
async fn busy_and_rate_limited_requests_are_retried() {
    let overloaded_mid_stream = Response::ok(Bytes::from(format!(
        "event: message_start\ndata: {}\n\nevent: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\nevent: error\ndata: {}\n\n",
        start(10, 0),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Par"}}),
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    )));
    let s = setup(vec![
        status(
            429,
            &[("retry-after", "0")],
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        ),
        status(
            529,
            &[],
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        ),
        overloaded_mid_stream,
        text_answer(&["Done."], "end_turn"),
    ]);
    let (out, seen) = run(
        &s,
        &Prices::default(),
        &AiConfig::default(),
        &mut Conversation::default(),
        "?",
    )
    .await;
    assert_eq!(out.ending, Ending::Answered);
    assert_eq!(out.answer, "Done.");
    let restarts: Vec<_> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Restart { why, .. } => Some(why.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        restarts,
        ["rate_limit_error", "overloaded_error", "overloaded_error"]
    );

    // Four busy answers in a row: give up and say so.
    let busy = || {
        status(
            529,
            &[],
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        )
    };
    let s = setup(vec![busy(), busy(), busy(), busy()]);
    let (out, _) = run(
        &s,
        &Prices::default(),
        &AiConfig::default(),
        &mut Conversation::default(),
        "?",
    )
    .await;
    assert_eq!(
        out.ending,
        Ending::Failed(mt_ai::AiError::Busy("overloaded_error".into()))
    );

    // A bad request is not retried.
    let s = setup(vec![status(
        400,
        &[],
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
    )]);
    let (out, _) = run(
        &s,
        &Prices::default(),
        &AiConfig::default(),
        &mut Conversation::default(),
        "?",
    )
    .await;
    assert!(matches!(
        out.ending,
        Ending::Failed(mt_ai::AiError::Rejected { .. })
    ));
    assert_eq!(s.api.asked.lock().len(), 1);
}

#[tokio::test]
async fn the_cap_and_a_missing_key_send_nothing() {
    let s = setup(vec![]);
    let mut conv = Conversation::default();
    let out = ask(
        &s.client,
        &Prices::default(),
        &AiConfig::default(),
        "",
        Allowance {
            cap_usd: 0.0,
            spent_usd: 0.0,
        },
        &mut conv,
        "?",
        &Cancel::default(),
        &|_| {},
    )
    .await;
    assert!(matches!(out.ending, Ending::CapReached { .. }));
    assert!(s.api.asked.lock().is_empty());

    let s = setup(vec![two_tool_calls(), text_answer(&["x"], "end_turn")]);
    let out = ask(
        &s.client,
        &Prices::default(),
        &AiConfig::default(),
        "",
        Allowance {
            cap_usd: 1.0,
            spent_usd: 0.999_999,
        },
        &mut conv,
        "?",
        &Cancel::default(),
        &|_| {},
    )
    .await;
    assert!(
        matches!(out.ending, Ending::CapReached { .. }),
        "{:?}",
        out.ending
    );
    assert_eq!(s.api.asked.lock().len(), 1, "stops before the second round");

    let api = FakeApi::with(vec![]);
    let ctx = FetchCtx::new(
        api.clone(),
        None,
        FetchCtxOptions::default(),
        EventLog::default(),
    );
    let client = Anthropic::new(ctx);
    assert!(!client.has_key());
    let out = ask(
        &client,
        &Prices::default(),
        &AiConfig::default(),
        "",
        PLENTY,
        &mut Conversation::default(),
        "?",
        &Cancel::default(),
        &|_| {},
    )
    .await;
    assert_eq!(out.ending, Ending::Failed(mt_ai::AiError::NoKey));
    assert!(api.asked.lock().is_empty());
}

/// A transport that never answers, so the question can only end by cancelling.
struct Silent;

impl Transport for Silent {
    fn send<'a>(&'a self, _req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        Box::pin(std::future::pending())
    }
    fn describe(&self) -> String {
        "silent".into()
    }
}

#[tokio::test]
async fn cancelling_leaves_the_conversation_as_it_was() {
    let secrets = Arc::new(MemorySecrets::with([(
        mt_ai::api::KEY_NAME,
        Secret::new("k"),
    )]));
    let ctx = FetchCtx::new(
        Arc::new(Silent),
        None,
        FetchCtxOptions {
            secrets,
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let client = Anthropic::new(ctx);
    let cancel = Cancel::default();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        stopper.cancel();
    });
    let mut conv = Conversation {
        messages: vec![mt_ai::Message::user_text("earlier")],
    };
    let before = conv.clone();
    let out = ask(
        &client,
        &Prices::default(),
        &AiConfig::default(),
        "",
        PLENTY,
        &mut conv,
        "?",
        &cancel,
        &|_| {},
    )
    .await;
    assert_eq!(out.ending, Ending::Cancelled);
    assert_eq!(conv, before);
}
