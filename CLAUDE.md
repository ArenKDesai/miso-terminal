# MISO Terminal: notes for coding agents

A Bloomberg-style terminal for MISO market data. It is read-only for MISO;
trading, when it comes, goes only through Alpaca, paper by default. It is a
Rust workspace with an egui UI, and Windows is the primary platform. Read
`docs/ARCHITECTURE.md` before structural changes and `docs/EXTENDING.md` for
recipes. `docs/MARKETS-PLAN.md` is the agreed plan for news, Alpaca market
data, trading and portfolio tracking: Phase 0 (foundations), Phase 1 (news)
and Phase 2 (market data) are built, the README's roadmap tracks the rest.

## Commands

```powershell
cargo test --workspace                 # must pass; includes headless UI smoke tests
cargo clippy --workspace --all-targets -- -D warnings   # what CI runs
cargo fmt --all
cargo run -- --offline                 # UI work against recorded fixtures, no network
cargo run -p mt-miso --example capture_fixtures       # re-record MISO fixtures (one hit per endpoint)
cargo run -p mt-nws --example capture_weather        # re-record weather fixtures (two per city)
cargo run -p mt-eia --example capture_gas            # re-record the Henry Hub gas workbook
cargo run -p mt-news --example capture_news          # re-record the news feeds (story text replaced by samples)
cargo run -p mt-alpaca --example capture_alpaca      # re-record Alpaca (needs keys; prices made synthetic, stories sampled)
cargo run -p mt-theme --example sync_everforge -- ..\everforge
cargo run -p mt-ui --example render -- out.png --run "MAP MCC" --zoom   # offscreen PNG, fixtures, no window
$env:UPDATE_SNAPSHOTS=force; cargo test -p mt-ui --test snapshots           # accept intended visual changes
uv run tools/screenshot.py out.png --run "GP MINN.HUB"   # PrintWindow capture of the app window only (live data)
uv run tools/screenshot.py out.png --home some\dir --offline   # with a prepared config, no network
uv run tools/build_map_asset.py        # rebuild assets/map/miso_map.json from ../3D-MISO-Map
uv run tools/export_history.py         # long history from the EPJ DuckDB into the app's cache
uv run tools/build_docs.py site        # the GitHub Pages docs site, from README.md, docs/ and themes/README.md
```

`tests/snapshots.rs` compares renders with `crates/mt-ui/tests/snapshots/*.png`.
A visual change fails it on purpose: look at the `.new.png` / `.diff.png` it
leaves, and accept with `UPDATE_SNAPSHOTS=force` if the change is intended.
Renders freeze the clock (`mt_core::time::freeze_clock`) at the fixtures'
recording time; route any new "now" through `mt_core::time::now_utc`.

Prefer the `render` example for reviewing UI changes: it drives the real app
offscreen (egui_kittest + wgpu), so it never touches the desktop and can reach
states like a zoomed panel or a theme. Use the screenshot tool for live data.
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
- `crates/mt-data/src/`: `request.rs` (methods, headers, redaction), `budget.rs`
  (per-host request budgets), `secret.rs` (Credential Manager), `stream.rs`
  (hub-owned WebSockets), `ctx.rs` (polite interval, conditional GETs).
- `crates/mt-core/src/`: `instrument.rs` (`XLU US`, OCC options), `exchange.rs`
  (New York time and sessions), `money.rs` (exact decimals), beside the MISO model.
- `crates/mt-nws`: National Weather Service source, the template for non-MISO sources.
- `crates/mt-eia`: EIA Henry Hub gas spot, a second example of one.
- `crates/mt-alpaca`: Alpaca stocks and ETFs: snapshots, bars, assets, clock,
  calendar, company news (`queries.rs`), live prices and news (`stream.rs`, with
  the free plan's 30 trade and quote subscriptions: trades first), snapshot + stream merging (`board.rs`) and the
  `[markets]` config and built-in lists (`config.rs`). `mt-ui/src/market.rs`
  holds what the securities panels share (status, board, formats, picker);
  `functions/security_chart.rs` is GP for a security.
- `crates/mt-news`: RSS/Atom headlines, the built-in feeds and NI topics
  (`config.rs`), merging and the on-disk archive. `mt-ui/src/news.rs` combines
  feeds for panels and holds the headline browser TOP, NEWS and NI share.
- Example names must be unique across the workspace (they share
  `target/debug/examples/`; duplicates race at link time on Windows).

## Conventions

- Crate boundaries are load-bearing. `mt-core`, `mt-data`, `mt-miso`,
  `mt-nws`, `mt-eia`, `mt-news` and `mt-theme` must not depend on egui; CI
  tests them on Linux.
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
- The docs site is generated from the Markdown (`tools/build_docs.py`, styles in
  `tools/docs_style.css`): edit the Markdown, never the HTML. README's `## TODO`
  section becomes the site's roadmap page.
- Built-in themes are `themes/*.toml` (listed in `mt_theme::BUILTIN_SOURCES`);
  downloadable ones are `themes/gallery/*.toml` (named after their id, previewed
  in `docs/screenshots/themes/<id>.webp`, published by the docs site and
  installable from THEME). After changing the gallery, run
  `uv run tools/build_docs.py site --update-fixture` (a test checks the recorded
  index matches).
  `themes/gallery/everforge-*.toml` are generated: change the Everforge tokens
  and re-sync.
- Respect MISO's once-a-minute polling guidance. Real-time queries use
  `REALTIME_REFRESH`, and `FetchCtx` enforces a polite interval.
- API keys only through the secret store: `ctx.secret(name)` in a query, sent
  with `Request::secret_header`, entered in SET (`CREDENTIALS` in
  `functions/settings.rs`). Never in `config.toml`, saved state, logs, test
  output or fixtures. Per-minute API limits are a `Budget` in `FetchCtxOptions`.
- WebSocket feeds implement `mt_data::Stream`; panels call
  `cx.hub.watch_stream(&s, &topics)` every frame, as with `watch`. Never open
  a connection from a panel.
- Securities are written `XLU US` (`mt_core::instrument`); a bare token is a
  node or a code, never a ticker. Functions that accept securities set
  `takes_security`. Exchange times use `mt_core::exchange` (New York, with
  daylight saving), never `mt_core::time`; charts of securities use
  `widgets::chart::exchange_plot`. Order and position amounts are
  `Decimal` (`mt_core::money`), never `f64`; quotes and charts may use `f64`.
- Alpaca: panels check `cx.alpaca.is_ready()` (`market::needs_keys`) before
  watching anything, so a user without keys sees a prompt, not failing feeds.
  Every price shows its feed's label. Fixtures under `fixtures/*.alpaca.markets`
  hold synthetic prices and sample stories (`capture_alpaca` without
  `--verbatim`); never commit a verbatim Alpaca recording. No keys exist on the
  main dev machine: the drift workflow (run by hand with `alpaca_fixtures`)
  records fresh fixtures with the CI paper account and uploads them as an artifact.
- The rolling five-minute feed can take most of a minute to download late in
  the day; today's store is saved to the disk cache and restored at launch.
- News: headlines and summaries only, attributed and linked; never fetch or
  store article text, and never commit publishers' text (news fixtures are
  sample copies from `capture_news`; snapshots render those). Articles open through `AppCommand::OpenHeadline`, which
  only accepts http(s) links. Check a feed's robots.txt before adding it.
- Python helpers run through `uv` (inline script metadata), never global pip.
- Disk on the main dev machine is tight; `cargo clean` reclaims stale
  artifacts after profile changes.
