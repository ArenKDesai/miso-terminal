# MISO Terminal: notes for coding agents

A read-only, Bloomberg-style terminal for MISO market data. It is a Rust
workspace with an egui UI, and Windows is the primary platform. Read
`docs/ARCHITECTURE.md` before structural changes and `docs/EXTENDING.md` for
recipes.

## Commands

```powershell
cargo test --workspace                 # must pass; includes headless UI smoke tests
cargo clippy --workspace --all-targets # CI denies warnings
cargo fmt --all
cargo run -- --offline                 # UI work against recorded fixtures, no network
cargo run -p mt-miso --example capture_fixtures       # re-record fixtures (hits MISO once per endpoint)
cargo run -p mt-theme --example sync_everforge -- ..\everforge
uv run tools/screenshot.py out.png --run "GP MINN.HUB"   # PrintWindow capture of the app window only
```

## Conventions

- Crate boundaries are load-bearing. `mt-core`, `mt-data`, `mt-miso` and
  `mt-theme` must not depend on egui; CI tests them on Linux.
- New panel = new file in `crates/mt-ui/src/functions/` + one line in
  `functions/mod.rs` + a README row (a test checks the README).
- New MISO dataset = path in `endpoints.rs` → fixture → `mt-core` type →
  parser + fixture test → `api_spec!`/`Miso` method.
- Panels never block or spawn: `cx.hub.watch(&query)` every frame, render via
  `widgets::with_data`, colours from `cx.skin` slots only, numbers via `widgets::fmt`.
- All timestamps are market time (EST, UTC-5, no DST). Five-minute intervals are
  interval-beginning. Use `mt_core::time`, never local time, for market data.
- `*.HUB` names are the eight trading hubs. MISO's report `Type = Hub` also
  covers ~445 commercial nodes, so never filter on that column.
- Never put NaN into egui shapes or plot points (the tessellator panics in
  debug). Split lines at gaps instead (see `widgets::chart::step_runs`).
- `themes/everforge-*.toml` are generated: change the Everforge tokens and re-sync.
- Respect MISO's once-a-minute polling guidance. Real-time queries use
  `REALTIME_REFRESH`, and `FetchCtx` enforces a polite interval.
- Python helpers run through `uv` (inline script metadata), never global pip.
