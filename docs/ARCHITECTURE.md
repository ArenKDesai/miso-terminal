# Architecture

The design goal is that the terminal can keep growing (new panels, new
datasets, new sources, new themes) without anyone having to rework what is
already here. Every extension point below is an *addition*: a new file plus one
line in a list.

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
 data       │ mt-data        │◄────│ mt-miso        │     │ mt-theme       │
            │ DataHub, Query │     │ endpoints      │     │ TOML themes    │
            │ FetchCtx,      │     │ parsers        │     │ validation     │
            │ Transport,     │     │ queries        │     │ registry       │
            │ DiskCache      │     └───────┬────────┘     └────────────────┘
            └────────────────┘             │
            ┌──────────────────────────────▼───────────────────────────────┐
 domain     │ mt-core   prices · grid · market time · lenient numbers      │
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
User themes are read from the themes folder and hot-reloaded by polling a
cheap directory fingerprint every two seconds.

## Time

MISO publishes everything in EST year-round. `mt_core::time` is the only place
that knows this. Every `NaiveDateTime` in the codebase is market time. Chart x
values are Unix seconds (`chart_x`), and axes are labelled back in market time
with a grid that snaps to market midnight.

## Testing strategy

| Layer | Tests |
|---|---|
| `mt-core` | Time parsing for every MISO spelling, EST invariants, intraday store merging |
| `mt-data` | Hub dedupe, refresh, `prev` threading, error backoff, pause, GC, notify; transports; disk cache |
| `mt-miso` | Every parser against a recorded response in `fixtures/` (structure and sanity, not exact values, so re-recording keeps them green) |
| `mt-theme` | Built-ins parse, validate and round-trip; user overrides; contrast maths |
| `mt-ui` | Command parsing and completion; **headless smoke test**: every function × every theme, with no data and with all fixtures loaded, rendering *and tessellating* real frames; the app shell running startup commands |

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
