//! Check that ASK's request is one the Messages API accepts: the real system
//! prompt and lookups' tool list, with ASK's effort and caching, go first to
//! the token counter (free; with and without web search), then once to each
//! configured model as a real, streamed request of a few output tokens (about
//! $0.03 in all), since the counter accepts some requests the real endpoint
//! refuses (it let through 23 nullable parameters where strict tool use allows
//! 16). Uses the key in the secret store (SET); the account needs credits.
//!
//! cargo run -p mt-ui --example check_anthropic_request

use std::sync::Arc;

use mt_ai::{Message, ToolBox, TurnRequest};
use mt_data::{DataHub, EventLog, FetchCtx, FetchCtxOptions, HttpTransport, Request};
use mt_ui::AppConfig;
use mt_ui::assistant::{Lookups, prompt};

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
    let config = AppConfig::default();
    let lookups = Lookups::new(DataHub::new(runtime.handle().clone(), ctx.clone()), &config);
    let mut ok = true;
    for model in config.ai.model_list() {
        for web_search in [false, true] {
            let req = TurnRequest {
                model: model.clone(),
                effort: config.ai.effort,
                max_tokens: config.ai.max_tokens,
                system: prompt::SYSTEM.into(),
                tools: lookups.specs(),
                web_search,
                messages: vec![Message::user_text(
                    "Why did RT spike at INDIANA.HUB yesterday?",
                )],
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
            ok &= resp.is_success();
            println!(
                "{} web_search={web_search}: HTTP {} {}",
                model.id,
                resp.status,
                resp.excerpt(300)
            );
        }
    }
    // One real request per model, through the client ASK uses.
    let client = mt_ai::Anthropic::new(ctx.clone());
    for model in config.ai.model_list() {
        let req = TurnRequest {
            model: model.clone(),
            effort: config.ai.effort,
            max_tokens: 64,
            system: prompt::SYSTEM.into(),
            tools: lookups.specs(),
            web_search: false,
            messages: vec![Message::user_text("Reply with the single word: ready")],
        };
        let result = runtime.block_on(mt_ai::Provider::turn(&client, &req, &|_| {}));
        match result {
            Ok(turn) => println!(
                "{} real request: {:?}, {} in + {} cached + {} written, {} out, ${:.4}",
                model.id,
                turn.stop,
                turn.usage.input_tokens,
                turn.usage.cache_read_input_tokens,
                turn.usage.cache_creation_input_tokens,
                turn.usage.output_tokens,
                turn.usage.cost(&model)
            ),
            Err(e) => {
                ok = false;
                println!("{} real request: {e}", model.id);
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err("the API refused a request".into())
    }
}
