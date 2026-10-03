# MISO Terminal: notes for coding agents

A read-only, Bloomberg-style terminal for MISO market data. It is a Rust
workspace with an egui UI, and Windows is the primary platform. Read
`docs/ARCHITECTURE.md` before structural changes and `docs/EXTENDING.md` for
recipes.

## Commands

```powershell
cargo test --workspace                 # must pass; includes headless UI smoke tests
cargo clippy --workspace --all-targets -- -D warnings   # what CI runs
cargo fmt --all
cargo run -- --offline                 # UI work against recorded fixtures, no network
cargo run -p mt-miso --example capture_fixtures       # re-record MISO fixtures (one hit per endpoint)
cargo run -p mt-nws --example capture_fixtures        # re-record weather fixtures (two per city)
cargo run -p mt-theme --example sync_everforge -- ..\everforge
uv run tools/screenshot.py out.png --run "GP MINN.HUB"   # PrintWindow capture of the app window only
uv run tools/screenshot.py out.png --home some\dir --offline   # with a prepared config, no network
uv run tools/build_map_asset.py        # rebuild assets/map/miso_map.json from ../3D-MISO-Map
```

The screenshot tool runs the app in portable mode under a throwaway home (or
`--home`), so your real config and layout are untouched. It closes the app
with WM_CLOSE, which lets it save.

## Where things live

- `crates/mt-ui/src/functions/`: one file per function (panel). `functions/mod.rs` lists them.
- `crates/mt-ui/src/series.rs`: node price series (today / N-day history /
  spreads / stats / percentiles / on-peak). Reuse it rather than re-fetching.
- `crates/mt-ui/src/widgets/`: tiles, tables (`table`), charts (`chart`:
  time plots, step lines, duration plots), `node_picker`, `csv` copy button,
  `scale` (diverging colours), `heatmap`.
- `crates/mt-ui/src/alerts.rs`: alert rules and the edge-triggered engine (pure).
- `crates/mt-ui/src/capture.rs`: copy/save a panel as an image.
- `crates/mt-ui/src/geo.rs`: map asset and projection.
- `crates/mt-nws`: National Weather Service source, the template for non-MISO sources.

## Conventions

- Crate boundaries are load-bearing. `mt-core`, `mt-data`, `mt-miso`,
  `mt-nws` and `mt-theme` must not depend on egui; CI tests them on Linux.
- New panel = new file in `crates/mt-ui/src/functions/` + one line in
  `functions/mod.rs` + a README row (a test checks the README). Add any
  argument variants to `routes()` in `smoke_tests.rs`.
- New MISO dataset = path in `endpoints.rs` (and `paths::ALL`) → fixture →
  `mt-core` type → parser + fixture test → `api_spec!`/`Miso` method.
- Panels never block or spawn: `cx.hub.watch(&query)` every frame, render via
  `widgets::with_data`, colours from `cx.skin` slots only, numbers via `widgets::fmt`.
- Panels talk to the shell through `AppCommand`s. Single-instance panels
  implement `Panel::absorb`.
- Wrap wide rows (`horizontal_wrapped`) or put wide tables in a horizontal
  `ScrollArea`: anything wider than a pane stretches it and clips charts.
- All timestamps are market time (EST, UTC-5, no DST). Five-minute intervals are
  interval-beginning. Use `mt_core::time`, never local time, for market data.
- `*.HUB` names are the eight trading hubs. MISO's report `Type = Hub` also
  covers ~445 commercial nodes, so never filter on that column.
- Never put NaN into egui shapes or plot points (the tessellator panics in
  debug). Split lines at gaps instead (see `widgets::chart::step_runs`).
- `themes/everforge-*.toml` are generated: change the Everforge tokens and re-sync.
- Respect MISO's once-a-minute polling guidance. Real-time queries use
  `REALTIME_REFRESH`, and `FetchCtx` enforces a polite interval.
- The rolling five-minute feed can take most of a minute to download late in
  the day; today's store is saved to the disk cache and restored at launch.
- Python helpers run through `uv` (inline script metadata), never global pip.
- Disk on the main dev machine is tight; `cargo clean` reclaims stale
  artifacts after profile changes.
