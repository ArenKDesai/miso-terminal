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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use egui_kittest::Harness;
use mt_data::{DataHub, EventLog, FetchCtx, FetchCtxOptions, FixtureTransport};
use mt_ui::{AppConfig, AppPaths, Deps, TerminalApp};

struct Args {
    out: PathBuf,
    run: Vec<String>,
    zoom: bool,
    theme: Option<String>,
    size: (f32, f32),
    scale: f32,
    fixtures: PathBuf,
}

fn parse() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut args = Args {
        out: PathBuf::new(),
        run: Vec::new(),
        zoom: false,
        theme: None,
        size: (1600.0, 960.0),
        scale: 1.0,
        fixtures: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
    };
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--run" => args.run.push(value()?),
            "--zoom" => args.zoom = true,
            "--theme" => args.theme = Some(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').context("--size is WxH")?;
                args.size = (w.parse()?, h.parse()?);
            }
            "--scale" => args.scale = value()?.parse()?,
            "--fixtures" => args.fixtures = value()?.into(),
            other if other.starts_with("--") => bail!("unknown option {other}"),
            other => args.out = other.into(),
        }
    }
    if args.out.as_os_str().is_empty() {
        bail!("usage: render OUT.png [--run CMD]... [--zoom] [--theme ID] [--size WxH]");
    }
    Ok(args)
}

fn main() -> Result<()> {
    let mut args = parse()?;
    if let Ok(dir) = std::fs::canonicalize(&args.fixtures) {
        // Without Windows' verbatim prefix, which the status bar would show.
        args.fixtures = PathBuf::from(dir.to_string_lossy().trim_start_matches(r"\\?\"));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let hub = DataHub::new(
        runtime.handle().clone(),
        FetchCtx::new(
            Arc::new(FixtureTransport::new(&args.fixtures)),
            None,
            FetchCtxOptions {
                max_concurrent: 8,
                polite_interval: Duration::ZERO,
            },
            EventLog::default(),
        ),
    );
    let home = std::env::temp_dir().join(format!("mt-render-{}", std::process::id()));
    let mut config = AppConfig::default();
    if let Some(theme) = &args.theme {
        config.theme = theme.clone();
    }
    let deps = Deps {
        hub: hub.clone(),
        config,
        config_error: None,
        paths: AppPaths::under(&home),
        reset_layout: true,
        startup_commands: args.run.clone(),
        remote: None,
    };
    let mut harness = Harness::builder()
        .with_size(egui::vec2(args.size.0, args.size.1))
        .with_pixels_per_point(args.scale)
        .wgpu()
        .build_eframe(|cc| TerminalApp::new(cc, deps));

    // Let every fixture load: step until nothing has been in flight for a while.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut quiet = 0;
    while quiet < 10 && Instant::now() < deadline {
        harness.step();
        std::thread::sleep(Duration::from_millis(30));
        quiet = if hub.in_flight() == 0 { quiet + 1 } else { 0 };
    }
    if args.zoom {
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::M);
    }
    // A few more frames for layout (tables size columns on their first pass).
    for _ in 0..5 {
        harness.step();
    }
    let image = harness.render().map_err(anyhow::Error::msg)?;
    image
        .save(&args.out)
        .with_context(|| format!("writing {}", args.out.display()))?;
    println!(
        "wrote {} ({}x{})",
        args.out.display(),
        image.width(),
        image.height()
    );
    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}
