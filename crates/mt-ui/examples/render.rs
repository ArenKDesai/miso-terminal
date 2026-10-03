//! Render the terminal to a PNG without opening a window: the real app, real
//! input handling and the real wgpu renderer, driven by egui_kittest against
//! the recorded fixtures. For reviewing UI changes, docs screenshots and
//! states that are awkward to reach by hand.
//!
//!     cargo run -p mt-ui --example render -- out.png --run "MAP MCC" --zoom
//!
//! Options:
//!   --run CMD        run a command first (repeatable)
//!   --zoom           zoom the focused panel (as Ctrl+M does)
//!   --theme ID       theme id, e.g. amber-terminal
//!   --size WxH       window size in points (default 1600x960)
//!   --scale F        pixels per point (default 1)
//!   --fixtures DIR   recorded responses to replay (default: the repo's)
//!   --cache DIR      a disk cache to read local data from (e.g. the long-history
//!                    archive written by tools/export_history.py)
//!   --at TIME        freeze the clock at this market time (EST), e.g.
//!                    "2026-10-02 16:55". Default: when the fixtures were
//!                    recorded, so they look as they did live. `--at now`
//!                    leaves the clock running.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

#[path = "../tests/common/scene.rs"]
mod scene;

fn main() -> Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut out, mut run, mut zoom, mut theme) = (PathBuf::new(), Vec::new(), false, None);
    let (mut size, mut scale, mut fixtures, mut at) =
        ((1600.0, 960.0), 1.0, scene::fixtures_dir(), None::<String>);
    let mut cache: Option<PathBuf> = None;
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--run" => run.push(value()?),
            "--zoom" => zoom = true,
            "--theme" => theme = Some(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').context("--size is WxH")?;
                size = (w.parse()?, h.parse()?);
            }
            "--scale" => scale = value()?.parse()?,
            "--fixtures" => fixtures = value()?.into(),
            "--at" => at = Some(value()?),
            "--cache" => cache = Some(value()?.into()),
            other if other.starts_with("--") => bail!("unknown option {other}"),
            other => out = other.into(),
        }
    }
    if out.as_os_str().is_empty() {
        bail!(
            "usage: render OUT.png [--run CMD]... [--zoom] [--theme ID] [--size WxH] [--at TIME]"
        );
    }

    let frozen = match at.as_deref() {
        Some("now") => None,
        Some(t) => Some(
            mt_core::time::parse_market_datetime(t)
                .or_else(|_| mt_core::time::parse_market_datetime(&format!("{t}:00")))
                .map_err(|e| anyhow::anyhow!("--at: {e}"))?,
        ),
        None => scene::fixtures_time(&fixtures),
    };
    if let Some(t) = frozen {
        mt_core::time::freeze_clock(Some(mt_core::time::market_to_utc(t)));
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let hub = scene::offline_hub_with_cache(&runtime, &fixtures, cache.as_deref());
    let run: Vec<&str> = run.iter().map(String::as_str).collect();
    let image = scene::render(
        &hub,
        &scene::Scene {
            run: &run,
            zoom,
            theme: theme.as_deref(),
            size,
            pixels_per_point: scale,
        },
    )
    .map_err(anyhow::Error::msg)?;
    image
        .save(&out)
        .with_context(|| format!("writing {}", out.display()))?;
    println!(
        "wrote {} ({}x{})",
        out.display(),
        image.width(),
        image.height()
    );
    Ok(())
}
