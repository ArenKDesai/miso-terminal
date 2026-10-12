//! Check that ASK's request is one the Messages API accepts, for free: the
//! same body (system prompt, tools, effort, caching) goes to the
//! token-counting endpoint, which validates it without generating or
//! charging. Uses the key in the secret store (SET) and the real lookups'
//! tool list.
//!
//! cargo run -p mt-ai --example check_anthropic_request

use std::sync::Arc;

use mt_ai::config::AiConfig;
use mt_ai::{Message, ToolSpec, TurnRequest};
use mt_data::{EventLog, FetchCtx, FetchCtxOptions, HttpTransport, Request};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let ctx = FetchCtx::new(
        Arc::new(HttpTransport::new("MISO-Terminal check")?),
        None,
        FetchCtxOptions {
            secrets: mt_data::os_store("miso-terminal"),
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let key = ctx.secret(mt_ai::api::KEY_NAME)?;
    let config = AiConfig::default();
    // A strict tool like the lookups' (the real list lives in mt-ui).
    let tools = vec![ToolSpec {
        name: "lmp_series".into(),
        description: "A node's prices".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"node": {"type": "string", "maxLength": 60},
                           "days": {"type": "integer", "minimum": 1, "maximum": 31},
                           "market": {"type": "string", "enum": ["da", "rt"]}},
            "required": ["node"], "additionalProperties": false}),
    }];
    for web_search in [false, true] {
        let req = TurnRequest {
            model: config.model_info(),
            effort: config.effort,
            max_tokens: config.max_tokens,
            system: "You answer questions about MISO.".into(),
            tools: tools.clone(),
            web_search,
            messages: vec![Message::user_text("Why did RT spike?")],
        };
        let mut body = req.body();
        // Fields only the generating endpoint takes.
        if let Some(o) = body.as_object_mut() {
            for k in ["stream", "max_tokens", "fallbacks"] {
                o.remove(k);
            }
        }
        let http = Request::post("https://api.anthropic.com/v1/messages/count_tokens")
            .secret_header("x-api-key", key.clone())
            .header("anthropic-version", mt_ai::api::API_VERSION)
            .json(&body)?;
        let resp = runtime.block_on(ctx.send(http))?;
        println!(
            "web_search={web_search}: HTTP {} {}",
            resp.status,
            resp.excerpt(400)
        );
    }
    Ok(())
}
