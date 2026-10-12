//! Record an ASK conversation for replays (`--offline`), the snapshot test
//! and the tutorial: one question, answered by Claude live (the key in the
//! secret store; about $0.10), with the lookups answered from the recorded
//! fixtures and the clock frozen when they were recorded, so the answer
//! matches the data a replay shows. Each streamed answer is saved in turn as
//! `fixtures/api.anthropic.com/v1/messages.post.<n>.json`, which a replay
//! plays back in order.
//!
//! cargo run -p mt-ui --example capture_ask -- "Why is INDIANA.HUB above MINN.HUB today?"

#[path = "../tests/common/scene.rs"]
mod scene;

use std::path::PathBuf;
use std::sync::Arc;

use mt_ai::{AiConfig, Allowance, Anthropic, Cancel, Conversation, Progress};
use mt_data::{
    BoxFuture, EventLog, FetchCtx, FetchCtxOptions, FetchError, HttpTransport, Request, Response,
    Transport,
};
use mt_ui::AppConfig;
use mt_ui::assistant::{Lookups, prompt};
use parking_lot::Mutex;

/// The live API, writing down each answer.
struct Recorder {
    inner: HttpTransport,
    dir: PathBuf,
    count: Mutex<usize>,
}

impl Transport for Recorder {
    fn send<'a>(&'a self, req: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
        Box::pin(async move {
            let resp = self.inner.send(req).await?;
            if resp.is_success() {
                let n = {
                    let mut c = self.count.lock();
                    *c += 1;
                    *c
                };
                let path = self.dir.join(format!("messages.post.{n}.json"));
                std::fs::write(&path, &resp.body)
                    .map_err(|e| FetchError::Other(format!("{}: {e}", path.display())))?;
                println!("recorded {}", path.display());
            }
            Ok(resp)
        })
    }

    fn describe(&self) -> String {
        "Anthropic, recorded".into()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let question = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Why is INDIANA.HUB's price above MINN.HUB's today?".into());
    let fixtures = scene::fixtures_dir();
    let at = scene::fixtures_time(&fixtures).ok_or("the fixtures have no LMP table")?;
    mt_core::time::freeze_clock(Some(mt_core::time::market_to_utc(at)));

    let dir = fixtures.join("api.anthropic.com").join("v1");
    std::fs::create_dir_all(&dir)?;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("messages.post.")
        {
            std::fs::remove_file(entry.path())?;
        }
    }

    let runtime = tokio::runtime::Runtime::new()?;
    let api = FetchCtx::new(
        Arc::new(Recorder {
            inner: HttpTransport::new("MISO-Terminal capture_ask")?,
            dir,
            count: Mutex::new(0),
        }),
        None,
        FetchCtxOptions {
            secrets: mt_data::os_store("miso-terminal"),
            ..FetchCtxOptions::default()
        },
        EventLog::default(),
    );
    let hub = scene::offline_hub(&runtime, &fixtures);
    let lookups = Lookups::new(hub, &AppConfig::default());
    let config = AiConfig {
        monthly_cap_usd: 1.0,
        ..AiConfig::default()
    };
    let asked = format!("{}\n\n{question}", prompt::question_context(at));
    let out = runtime.block_on(mt_ai::ask(
        &Anthropic::new(api),
        &lookups,
        &config,
        prompt::SYSTEM,
        Allowance {
            cap_usd: 1.0,
            spent_usd: 0.0,
        },
        &mut Conversation::default(),
        &asked,
        &Cancel::default(),
        &|p| match p {
            Progress::ToolStarted { name, input, .. } => println!("lookup {name} {input}"),
            Progress::Round {
                number, cost_usd, ..
            } => println!("round {number}: ${cost_usd:.4}"),
            _ => {}
        },
    ));
    println!("\n{:?}, ${:.4}\n\n{}", out.ending, out.cost_usd, out.answer);
    Ok(())
}
