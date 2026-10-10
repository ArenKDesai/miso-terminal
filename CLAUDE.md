# MISO Terminal: notes for coding agents

A Bloomberg-style terminal for MISO market data. It is read-only for MISO;
trading goes only through Alpaca, on the paper account, through confirmed tickets. It is a
Rust workspace with an egui UI, and Windows is the primary platform. Read
`docs/ARCHITECTURE.md` before structural changes and `docs/EXTENDING.md` for
recipes. `docs/RELEASES.md` is the plan for releases (`v0.2.0` is out; ask
before tagging, since a `v*` tag publishes one). `docs/MARKETS-PLAN.md` is the
plan for news, Alpaca market data, trading and portfolio tracking: phases 0
to 5 are built, Phase 6 (live trading) is `0.4.0`. `docs/ANALYTICS-PLAN.md` is
the plan for `0.3.0`: the terminal's own
price history (built), studies and BETA, forecasts (FCST, crate
`mt-forecast`) and the Claude assistant (ASK, crate `mt-ai`, read-only tools
only); its part 2, forward fill for security prices, was dropped. Keep the
docs lean: the README's roadmap lists only open work (the changelog records
what is built), and a finished plan step shrinks to a short summary.
`docs/SECURITY-REVIEW.md` lists what the order path review found and what is
still open before live trading (Phase 6); update its status column when a
finding is fixed.

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
cargo run -p mt-alpaca --example check_paper_orders  # the order desk against the live paper API (needs paper keys; one order, cancelled)
cargo run -p mt-alpaca --example check_option_orders # an option order and a spread against the live paper API (paper keys; both cancelled)
uv run tools/sample_account.py         # rewrite the sample paper account and its orders (after re-recording Alpaca)
uv run tools/sample_options.py         # then the sample option chains (OMON) around the account's contracts
cargo run -p mt-theme --example sync_everforge -- ..\everforge
cargo run -p mt-ui --example render -- out.png --run "MAP MCC" --zoom   # offscreen PNG, fixtures, no window
$env:UPDATE_SNAPSHOTS=force; cargo test -p mt-ui --test snapshots           # accept intended visual changes
uv run tools/screenshot.py out.png --run "GP MINN.HUB"   # PrintWindow capture of the app window only (live data)
uv run tools/screenshot.py out.png --home some\dir --offline   # with a prepared config, no network
uv run tools/build_map_asset.py        # rebuild assets/map/miso_map.json from ../3D-MISO-Map
uv run tools/build_docs.py site        # the GitHub Pages docs site, from README.md, docs/ and themes/README.md
cargo about generate --locked -c packaging/about.toml -o notices.html packaging/about.hbs   # third-party notices (cargo-about 0.9, --features cli)
uv run tools/release_notes.py 0.2.0 --zip x.zip   # a release's notes from CHANGELOG.md (the release workflow runs it)
gh workflow run release.yml            # the release workflow as a dry run on main: zip, checksums and notes as an artifact, nothing published
```

`tests/snapshots.rs` compares renders with `crates/mt-ui/tests/snapshots/*.png`.
A visual change fails it on purpose: look at the `.new.png` / `.diff.png` it
leaves, and accept with `UPDATE_SNAPSHOTS=force` if the change is intended.
Renders freeze the clock (`mt_core::time::freeze_clock`) at the fixtures'
recording time; route any new "now" through `mt_core::time::now_utc`.

Prefer the `render` example for reviewing UI changes: it drives the real app
offscreen (egui_kittest + wgpu), so it never touches the desktop and can reach
states like a zoomed panel, a theme or an open menu (`--click "Studies · 2"`
clicks the widget with that label first). Use the screenshot tool for live data.
The screenshot tool runs the app in portable mode under a throwaway home (or
`--home`), so your real config and layout are untouched. It closes the app
with WM_CLOSE, which lets it save.

## How changes land

Never push to `main`. Every change goes through a pull request
(`docs/RELEASES.md`, "How changes land"): a branch from the latest `main`,
a pull request whose title and description become the squash commit (prose,
ending with the `Co-Authored-By:` line), and a `CHANGELOG.md` line under
*Unreleased* for a user-visible change (`CONTRIBUTING.md` has the checklist).
Aren has given standing permission to merge routine pull requests: open them
with auto-merge (`gh pr merge --auto --squash`), which GitHub carries out once
the required checks pass, and report the result. Ask first, and leave the
pull request open, for releases and tags (never push a tag), changes to
workflows, rulesets or repository settings, anything touching secrets or
keys, and anything Aren has asked to see.

## Where things live

- `crates/mt-ui/src/functions/`: one file per function (panel). `functions/mod.rs` lists them.
- `crates/mt-ui/src/series.rs`: node price series (today / N-day history /
  spreads / stats / percentiles / on-peak). Reuse it rather than re-fetching.
  History for several nodes goes through `nodes_history`, so the price
  history is read once for all of them; never watch whole day reports for
  long windows (about 1 MB a day in memory).
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
  `functions/security_chart.rs` is GP for a security (with its studies, whose
  maths and command-line words are in `mt-core/src/studies.rs`),
  `functions/cross_chart.rs` is CMP with securities (stocks above node
  prices, daily correlations) and `functions/beta.rs` is BETA (maths in
  `mt-core/src/beta.rs`; benchmarks in `mt-alpaca/src/config.rs`; total
  returns from `BarsQuery::adjusted(Adjustment::All)`).
- The paper account: `mt-core/src/account.rs` (types, marking, net delta),
  `mt-alpaca/src/account.rs` (account, positions, history, activities, option
  greeks), `mt-alpaca/src/trades.rs` (order events, binary frames),
  `mt-ui/src/portfolio.rs` (live marks, re-sync, the PAPER band) and the
  functions `port.rs`, `acct.rs`, `pnl.rs`, `act.rs`.
- Paper trading: `mt-core/src/order.rs` (orders, requests, day value, day
  trades), `mt-core/src/guard.rs` (every guardrail, and `Limits`, the
  `[trading]` config), `mt-alpaca/src/orders.rs` (order JSON, the order list),
  `mt-alpaca/src/desk.rs` (the order desk: send, look up, cancel, replace, kill
  switch; a fake broker in its tests), `mt-alpaca/src/audit.rs` (the audit log),
  `mt-ui/src/trading.rs` (what tickets and ORD share) and the functions
  `ticket.rs` (BUY and SELL) and `ord.rs`.
- Options: `mt-core/src/options.rs` (contract lists, chains by strike, price
  steps, expiry rules, position intents, legs, strategy names, net prices,
  payoffs and Alpaca's spread margin), `mt-core/src/guard/options.rs` and
  `guard/spread.rs` (the single-contract and spread guardrails),
  `mt-alpaca/src/options.rs` (the contract list and chain queries; order bodies
  and `mleg` parsing are in `orders.rs`), `mt-ui/src/options.rs` (formats and
  expiries the option functions share), `trading::OptionMarket` (what option
  tickets are checked against) and the functions `omon.rs`, `ticket.rs` (for a
  contract) and `mleg.rs`.
- FCST: `functions/fcst.rs` (the panel) and `mt-ui/src/forecast.rs` (its
  data: a node's inputs from the price history, gas and the forecasts kept
  as issued, complete days only; the forecasts as hub queries trained on a
  blocking thread through `FetchCtx::blocking`, keyed by a fingerprint of
  their inputs and cached under `local://models/` with `MODEL_VERSION`,
  which a change to models or features bumps).
- `crates/mt-forecast`: FCST's maths, no I/O: `models.rs` (a day's 24
  hours: naive, profile, smoothing), `evaluate.rs` (backtests from what was
  known at the time, accuracy, conformal bands), `securities.rs` (a close's
  distribution: random walk, drift, volatility cone, GARCH), `calendar.rs`
  (NERC holidays) and `hourly.rs` (series with gaps). A model never sees data
  from after its forecast was made: the backtest cuts the series off.
- `crates/mt-news`: RSS/Atom headlines, the built-in feeds and NI topics
  (`config.rs`), merging and the on-disk archive. `mt-ui/src/news.rs` combines
  feeds for panels and holds the headline browser TOP, NEWS and NI share.
- Example names must be unique across the workspace (they share
  `target/debug/examples/`; duplicates race at link time on Windows).

## Conventions

- Crate boundaries are load-bearing. `mt-core`, `mt-data`, `mt-miso`,
  `mt-nws`, `mt-eia`, `mt-news`, `mt-alpaca`, `mt-forecast` and `mt-theme`
  must not depend on egui; CI tests them on Linux.
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
  section becomes the site's roadmap page. The build fails on a broken link,
  anchor or repository path (CI's required *Docs* check), so run it after
  renaming a heading or a file.
- `docs/TUTORIALS.md` walks users through the features, naming buttons by their
  labels. When a panel's controls, a command's arguments or a config key change,
  update the tutorial that mentions them. Its screenshots
  (`docs/screenshots/tutorials/*.webp`) are `render` output against the
  fixtures at 1280x720, zoomed, saved as lossless WebP.
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
  `takes_security`, and those that accept option contracts (OCC symbols)
  `takes_option`. Exchange times use `mt_core::exchange` (New York, with
  daylight saving), never `mt_core::time`; charts of securities use
  `widgets::chart::exchange_plot`. Order and position amounts are
  `Decimal` (`mt_core::money`), never `f64`; quotes and charts may use `f64`.
- Alpaca: panels check `cx.alpaca.is_ready()` (`market::needs_keys`) before
  watching anything, so a user without keys sees a prompt, not failing feeds.
  Every price shows its feed's label. Fixtures under `fixtures/*.alpaca.markets`
  hold synthetic prices and sample stories (`capture_alpaca` without
  `--verbatim`); never commit a verbatim Alpaca recording. The drift workflow
  (run by hand with `alpaca_fixtures`) records fresh fixtures with the CI paper
  account and uploads them as an artifact. The main dev machine may hold the
  maintainer's own paper keys (entered in SET): reading with them is fine, but
  ask before anything that places or cancels orders (`check_paper_orders`,
  `check_option_orders`), and never commit what that account returns.
  The account's fixtures are a made-up portfolio from `tools/sample_account.py`
  (never an account's own), and the option chains are priced around it by
  `tools/sample_options.py`; rerun both after new Alpaca fixtures land.
- Orders: only `mt_alpaca::OrderDesk` sends them, and only when a ticket's
  *Confirm*, ORD's *Cancel*/*Confirm replace* or the kill switch is clicked.
  Commands (typed, `--run`, forwarded, hotkeys) open tickets (BUY, SELL,
  MLEG) and never send;
  nothing trades automatically (`commands_from_outside_the_window_only_open_tickets`
  checks it). Every ticket runs `mt_core::guard::review` (a contract:
  `review_option`; a spread: `review_spread`) first; a new rule goes there with
  a test. A ticket keeps one `client_order_id` until its order is
  placed, so a resend after a lost answer cannot duplicate it. Tickets are not
  restored after a restart. Order amounts are `Decimal`. The audit log never
  holds keys (they are headers) and its path (which names the Windows user)
  shows only on hover.
- The PAPER band never shows a balance and ACCT masks the account number: keep
  account figures out of anything that is always on screen.
- The rolling five-minute feed can take most of a minute to download late in
  the day; today's store is saved to the disk cache and restored at launch.
- Daily DA ex-post and RT reports pass through the day store
  (`local://archive/report/<da|rt>/<date>`, ARCHITECTURE's *The day store*):
  read old days with `mt_miso::read_day_store` or the report queries, never
  by caching the CSVs. Anything the app keeps on purpose in the cache goes in
  `mt_ui::kept_cache_dirs`, which the size cap and *Clear cache* skip.
  `mt_miso::history` holds the store to `[price_history]`'s window and fills
  it (`Backfill`, a worker the app configures; SET, LOG and the status bar
  read its status through `mt_ui::history`). It fetches through
  `fetch_report` like the panels, so the store stays the only record of what
  is there; keep it that way rather than tracking progress elsewhere.
- Forecasts kept as issued (`mt_data::issued`): one gzipped TSV per kind
  per day, `local://archive/issued/<kind>/<date>`, each value with when it
  was issued, stored only when it changes. A `Collector` (the app's, beside
  the backfill) polls `Source`s from `mt_miso::issued` (MTLF reports with a
  year's backfill, wind and solar, outages) and `mt_nws::issued`
  (temperatures), only while live. A model may only use a version issued
  before its forecast's moment (`Issued::as_of` in `mt-forecast`); MTLF
  reports count as issued at 06:00 on their date (MISO posts them at about
  01:20). A new kind or source updates `docs/PRIVACY.md`.
- News: headlines and summaries only, attributed and linked; never fetch or
  store article text, and never commit publishers' text (news fixtures are
  sample copies from `capture_news`; snapshots render those). Articles open through `AppCommand::OpenHeadline`, which
  only accepts http(s) links. Check a feed's robots.txt before adding it.
- `config.toml` keys may be added, never renamed or retyped in a way older
  files cannot load: `fixtures/compat/` holds the files earlier versions
  wrote, and `configs_from_earlier_versions_load` parses each strictly. Never
  edit those files; a release adds its own (`default_config` example).
- The version the app shows is `mt_ui::version()`: the binary's build script
  sets `MT_VERSION` (the commit added unless `MT_RELEASE=1`), and `main`
  passes it on. Tests and examples see the plain crate version, so renders
  stay stable.
- `docs/PRIVACY.md` is a public commitment: a change that contacts a new
  service or sends something new updates its list in the same pull request.
- Everything in the executable stays under OSI-approved licences
  (`packaging/about.toml` for crates; bundled fonts in `assets/fonts/` with
  their licences; egui's default fonts stay off), which keeps free
  open-source code signing possible (RELEASES.md's *Signing*: possible, not
  planned; never apply to SignPath Foundation without Aren).
- Python helpers run through `uv` (inline script metadata), never global pip.
- `rust-toolchain.toml` pins the compiler for local builds, CI and releases;
  every CI build is `--locked`. Routine maintenance moves the pin to the
  latest stable (in its own pull request: new lints show up there), merges
  Dependabot's weekly pull requests (lockfile-only for Cargo), and checks
  `cargo update --dry-run --verbose` for upgrades held back past
  `Cargo.toml`'s ranges.
- Third-party notices for the release zip come from cargo-about
  (`packaging/about.toml`, `packaging/about.hbs`); CI generates them on every
  pull request, so a dependency under an unaccepted licence fails there.
- Disk on the main dev machine is tight; `cargo clean` reclaims stale
  artifacts after profile changes.
