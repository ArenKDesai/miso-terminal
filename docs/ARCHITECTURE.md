# Architecture

The design goal is that the terminal can keep growing (new panels, new
datasets, new sources, new themes) without anyone having to rework what is
already here. Every extension point below is an *addition*: a new file plus one
line in a list.

It is also a community project, and the [principles](../README.md#principles)
are part of the design: user data stays private and secure (no telemetry,
secrets in Windows Credential Manager), every source's robots.txt, terms and
rate limits are respected, and everything stays free and open source under the
AGPL-3.0.

## Layers

```
            ┌──────────────────────────────────────────────────────────────┐
 binary     │ miso-terminal   paths · logging · tokio runtime · window     │
            └───────────────┬──────────────────────────────────────────────┘
            ┌───────────────▼──────────────────────────────────────────────┐
 UI         │ mt-ui   shell (command line, ticker, status) · workspace     │
            │         function registry · panels · widgets · skin/fonts    │
            └───────┬──────────────────────┬──────────────────────┬────────┘
                    │ hub.watch(query)     │ miso.lmp_board() …   │ Theme
            ┌───────▼────────┐     ┌───────▼────────┐     ┌───────▼────────┐
 data       │ mt-data        │◄────│ mt-miso, mt-nws│     │ mt-theme       │
            │ DataHub, Query │     │ endpoints      │     │ TOML themes    │
            │ FetchCtx,      │     │ parsers        │     │ validation     │
            │ Transport,     │     │ queries        │     │ registry       │
            │ DiskCache      │     └───────┬────────┘     └────────────────┘
            └────────────────┘             │
            ┌──────────────────────────────▼───────────────────────────────┐
 domain     │ mt-core   prices · grid · weather · time · geometry · numbers│
            └──────────────────────────────────────────────────────────────┘
```

Dependencies point downward only. `mt-core`, `mt-data`, `mt-miso` and
`mt-theme` know nothing about egui, and CI tests them on Linux to keep them
portable. A second front end (a TUI, a web view, a CLI exporter) could reuse all
four unchanged.

## Data flow

1. A panel's `ui()` runs every frame. It asks for data with
   `cx.hub.watch(&cx.miso.fuel_mix())` and gets a `Snapshot<FuelMix>` back
   immediately: the cached value (possibly stale) plus `loading`, `error`,
   `stale` and `updated`.
2. If the value is missing or due (per the query's `Freshness`), the hub spawns
   a fetch on the tokio runtime. Requests for the same key are deduplicated: ten
   panels watching the LMP board make one request.
3. The query's `fetch()` goes through `FetchCtx`, which adds:
   a concurrency cap; a **polite interval** (a repeat request for the same URL
   inside the window gets the previous body, enforcing MISO's once-a-minute
   rule even when a user hammers F5); an **on-disk gzip cache** for immutable
   files (settled market reports); and an event log.
4. `FetchCtx` calls a `Transport`: `HttpTransport` (reqwest with native-tls, so
   SChannel and the Windows certificate store) or `FixtureTransport` (recorded
   files, used by `--offline` and the tests).
5. The response is parsed by a pure function in `mt-miso::parse` into `mt-core`
   types. On success the hub stores it and calls the notify hook, which the app
   wires to `request_repaint()`. On failure the last good value is kept, the
   error is attached, and retries back off exponentially (5 s up to 5 min).
6. Nothing refreshes unless something is watching it. Unwatched entries are
   garbage-collected after 15 minutes.

Panels therefore never block, never own threads, and never handle HTTP. Loading
and error states look the same everywhere because they all go through
`widgets::with_data`.

### Restoring at launch

Some feeds are slow to fill: MISO's rolling five-minute feed is tens of MB
late in the day. The shell saves today's five-minute store to the disk cache
(every 5 minutes and on exit) and, at launch, puts it back with
`DataHub::seed_stale`. That shows the value immediately, reports its real
age (so freshness labels stay honest), and makes it due at once. The first
refresh then builds on it as `prev`.

### The five-minute archive

Each day's store stays in the cache under `local://intraday/<day>`, so the
saved days accumulate into a five-minute archive. `RtArchiveQuery` reads one
day back (no network), and `RtPreviousDayQuery` writes yesterday's complete
day into it whenever MISO's previous-day feed is fetched, replacing a partial
day. Queries reach the store through `FetchCtx::local_get` / `local_put`, which
keep the disk work off the async workers; any source can keep its own data the
same way. `series::node_five_minute` stitches archive days, yesterday and today
together for GP and SPRD (`… 5MIN`). The archive is exempt from the cache's size cap and
keeps `data.archive_days` days instead.

### Long history

`tools/export_history.py` exports hourly DA and RT prices per node from the
Energy-Pricing-Journalist DuckDB into the cache (`local://archive/lmp/<node>`,
exempt from the size cap). `LmpArchiveQuery` reads a node's file once per run,
and `series::node_history` takes every day the archive covers from it, so only
the days after it ends are downloaded as daily reports. Without an archive,
windows are capped at 90 days of downloads. The DuckDB is linked by the script,
not the app, which keeps a large C++ build out of the terminal.

### Feeds with memory

`Query::fetch` receives the previous value. `RtIntradayQuery` uses that to seed
once from MISO's rolling feed (~7 MB gzipped, every CP node for the whole day so
far) and then append only the current interval (~25 KB) each minute. It
re-seeds automatically on a new market day or after a gap (sleep, network
outage). `Freshness` can also depend on the value: a preliminary RT report
refreshes hourly until the final report replaces it, then never refreshes again.

## Functions, routes and the workspace

A **function** is a `FunctionSpec` (code, aliases, name, category, usage,
description, `open` fn) plus a `Panel` implementation. `functions::all()` is the
single list. The command line, the Functions menu, HELP, the layout and the
smoke tests all read it.

Open tabs are persisted as **routes** (`GP MINN.HUB 14`), not as panel state.
On startup each route is reopened through the registry. Panels can therefore
hold anything (no `Serialize` needed), and a function that is renamed or removed
later opens a harmless placeholder instead of breaking a saved layout. A panel
whose state changes (a new node, a new day count) reports that through
`route()`, so the layout always reopens what the user was looking at. Bump
`workspace::LAYOUT_VERSION` if the persisted format ever changes incompatibly.

Opening a route focuses an existing tab with the same route. A panel can also
*absorb* a non-identical route of its own code (`Panel::absorb`). `WL` does this,
so `WL ALTE.ALTE` adds to the open watchlist instead of opening a second one.

Shared panel machinery lives outside the functions so panels stay small:
`series` assembles a node's prices from whichever feeds cover each span
(five-minute today, daily reports before, yesterday's full five-minute day on
demand), and computes spreads, stats, percentiles and the on-peak block.
`alerts` is a pure, edge-triggered rule engine the shell evaluates every
frame, watching only the feeds active rules need. `capture` turns a tab's
area into a clipboard image or PNG via egui's window screenshot.

Panels talk back to the shell only through `AppCommand`s (open a route, set a
theme, refresh, reveal a folder). The shell applies them after the frame, so
panels never hold `&mut App`.

## Themes

`mt-theme` defines a `Theme` of semantic slots and validates WCAG contrast for
the pairings the UI uses. `mt-ui::skin` maps a theme onto egui's `Style` and
`Visuals` (widgets, selection, shadows, spacing, text styles) and converts the
palette to `Color32`s for panels. `mt-ui::fonts` resolves font families in this
order: bundled faces (with variable-font weights through the `wght` axis), then
the user fonts folder, then system fonts (scanned lazily). JetBrains Mono is
always the symbol fallback.

Built-in themes are TOML files in `themes/`, compiled in with `include_str!`.
More themes live in `themes/gallery/`: not compiled in, but validated by the
tests and published by the docs build, as files and as one index
(`themes/index.json`, every file verbatim). THEME reads that index through the
hub like any feed (`mt_ui::gallery::GalleryQuery`, refreshed every six hours)
and installs a theme by writing its file into the themes folder
(`mt_theme::gallery::install`, which refuses ids that are not plain file
names); the app then reloads the registry at once rather than at the next poll.
User themes are read from the themes folder and hot-reloaded by polling a
cheap directory fingerprint every two seconds. A configured theme that is no
longer installed falls back to the default with a notice pointing at the
gallery.

## Time

MISO publishes everything in EST year-round. `mt_core::time` is the only place
that knows this. Every `NaiveDateTime` in the codebase is market time. Chart x
values are Unix seconds (`chart_x`), and axes are labelled back in market time
with a grid that snaps to market midnight.

## Testing strategy

| Layer | Tests |
|---|---|
| `mt-core` | Time parsing for every MISO spelling, EST invariants, intraday store merging and round-trips, map masks and surfaces |
| `mt-data` | Hub dedupe, refresh, `prev` threading, error backoff, pause, GC, notify; transports; disk cache |
| `mt-miso` | Every parser against a recorded response in `fixtures/` (structure and sanity, not exact values, so re-recording keeps them green); the previous-day feed filling the archive |
| `mt-nws` | Weather parsers against recordings for every city; the same `MT_FIXTURES` override |
| `mt-theme` | Built-ins parse, validate and round-trip; user overrides; contrast maths |
| `mt-ui` | Command parsing, completion and hints; alert engine; series maths; **headless smoke test**: every function × every theme, with no data and with all fixtures loaded, rendering *and tessellating* real frames; the app shell running startup commands, alerts firing and tab shortcuts; a panicking panel contained; today's prices restored after a restart |
| visual | `mt-ui/tests/snapshots.rs`: the real app rendered offscreen (egui_kittest + wgpu) against the fixtures with the clock frozen at their recording time, compared with committed images: every theme's layout and several zoomed panels |

The smoke test iterates the registry, so a new function gets coverage without
writing a test. Separately, the weekly `drift.yml` workflow records live MISO
responses and runs the parser tests against them (`MT_FIXTURES`), so MISO format
changes surface in CI rather than as a blank panel.

At runtime, each tab's `ui()` runs inside `catch_unwind`. A panicking panel is
logged, its state is dropped, and the tab shows the error with a *Reload panel*
button. The rest of the terminal keeps running. It also checks that every feed any panel requests loads from the
fixtures without error, which is what makes `--offline` trustworthy.

## Decisions

- **egui + egui_dock + egui_plot.** Immediate mode keeps a panel to one file
  with no retained widget tree to synchronise. egui_dock provides tabs, splits
  and drag-to-dock. egui_plot provides zoomable, hoverable time series. Rendering
  is wgpu (DX12 on Windows, with a WARP software fallback for VMs and RDP).
- **native-tls**, not rustls. On Windows it is SChannel, which trusts the system
  certificate store, so TLS-inspecting corporate proxies work without setup.
- **No async runtime in the UI thread.** tokio has two worker threads owned by
  the binary. The UI only ever does non-blocking hub lookups.
- **Fixtures over mocks.** Real recorded responses catch real format drift.
  `capture_fixtures` re-records them in one command, trimming the large feeds to
  a few nodes.
- **TOML for config and themes.** Hand-editable, comment-friendly, and every
  field is optional with a default, so old files keep working.
- **One live window per home.** The binary locks `instance.lock` next to the
  window state and listens on a loopback port (`instance.rs`); a second launch
  sends its `--run` commands there with a token from `instance.port`, and the
  UI receives them through `mt_ui::remote`. Shortcuts then open functions in
  the running terminal, and MISO is never polled by two copies. Offline replay
  and `--new-instance` opt out. Standard library only (`File::try_lock`), no
  named pipes or `unsafe`.
