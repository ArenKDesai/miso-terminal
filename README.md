# MISO Terminal

A Bloomberg-style information terminal for the Midcontinent ISO, written in Rust.
It covers prices, load, generation, interchange, constraints, the seams, gas and
weather in one keyboard-driven, tiled workspace. It is read-only: it shows MISO's
public data (plus EIA gas prices and NWS forecasts) and never submits anything to MISO.

![MISO Terminal, Everforge Dark](docs/screenshots/home-everforge-dark.png)

- **Command line first.** Type `LMP`, `GP MINN.HUB`, `MINN.HUB GP 14`, or just a
  node name, then press Enter. Completion covers every function and every pricing node.
- **Tiled, tabbed workspace.** Drag tabs to split panes, zoom one to the whole
  window (`Ctrl+M`), or pop it out onto a second monitor. Your layout is saved
  between sessions.
- **Live.** Real-time feeds refresh once a minute (MISO's limit). Daily market
  reports are cached on disk, so history loads instantly the second time, and an
  optional local archive reaches back to 2023. Alerts arrive as Windows notifications.
- **Themed.** Ships with **Everforge Dark** (the default), Everforge Light,
  Amber Terminal and High Contrast. Drop a TOML file into the themes folder to add your own; it
  hot-reloads while you edit it.
- **Built for Windows.** A single `.exe` with a native window, Windows certificate
  store TLS (corporate proxies work), a per-user or portable data layout and an
  embedded icon. The data, MISO and theme crates also build and test on Linux.

| | |
|---|---|
| ![GP history](docs/screenshots/gp-history.png) | ![Amber Terminal](docs/screenshots/amber-terminal.png) |

![MAP: real-time congestion across the footprint](docs/screenshots/map.png)

## Install (Windows)

Download `miso-terminal-<version>-windows-x64.zip` from the Releases page, unzip
it, and run:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1           # Start Menu shortcut
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Desktop  # plus a desktop shortcut
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall
```

It installs for the current user only (no administrator rights needed) to
`%LOCALAPPDATA%\Programs\MISO Terminal`. Or skip the script and run
`miso-terminal.exe` straight from the folder; it is a single portable file.

## Build from source (Windows)

```powershell
# Rust stable (https://rustup.rs) and the MSVC build tools are required.
git clone https://github.com/ArenKDesai/miso-terminal
cd miso-terminal
cargo run --release
```

Useful flags:

| Flag | What it does |
|---|---|
| `--offline [DIR]` | Replay the recorded responses in `fixtures/` instead of calling MISO. Use it for demos, UI work, or when MISO is down. |
| `--home DIR` | Portable mode: keep everything in `DIR`. Also enabled by `MISO_TERMINAL_HOME`, or a file named `portable` next to the exe. |
| `--run "CMD"` | Run a command at startup (repeatable), e.g. a desktop shortcut with `--run "GP ALTE.ALTE"`. If the terminal is already open, the command runs in that window instead. |
| `--reset-layout` | Start from the default layout. |
| `--new-instance` | Open a second window anyway. Normally there is one live window per home, so MISO is polled once. |

Files live in `%APPDATA%\MISO Terminal\config` (config, themes, fonts: these roam)
and `%LOCALAPPDATA%\MISO Terminal` (report cache, logs, window and layout state).
The `LOG` function shows the exact paths and has buttons to open them.

## Functions

| Code | Name | What it shows |
|---|---|---|
| `HOME` | Launchpad | Demand, marginal energy cost, interchange, generation, hub prices (RT, next-interval ex-ante, DA) with 5-min sparklines, fuel mix, top constraints |
| `LMP` | LMP monitor | ~300 key nodes with RT 5-min, RT hourly, DA ex-ante and ex-post, DART, MCC and MLC. Sortable, and filterable by name, region and node type (hub, load zone, interface, generator). `LMP ALL` lists all ~2,600 CP nodes with this hour's DA and RT − DA |
| `HUBS` | Hub statistics | All eight trading hubs over N days: DA, RT and DART averages, on-peak and off-peak blocks, RT volatility, extremes and how often RT beat DA |
| `DAM` | Day-ahead strip | Hourly DA prices at the eight hubs for one day (tomorrow once posted, else today), with on-peak, off-peak and all-hours averages, by component, or as the change from the day before (`DAM TOMORROW`) |
| `MAP` | Price map | Every node MISO plots, over an interpolated price surface of the footprint and the 230 kV-and-up transmission backbone, coloured by RT LMP, congestion, loss, DA or RT − DA. Hover for the breakdown, click to graph |
| `GP` | Graph price | One node: today's 5-min RT vs the DA staircase (plus tomorrow's DA once posted), N days of hourly DA vs RT with stats, an hour × day heatmap (`GP MINN.HUB 14 HEAT`), price duration curves (`… DUR`), or five-minute RT over past days from the local archive (`… 5MIN`). Switch between LMP, energy, congestion and loss |
| `SPRD` | Node spread | A − B between any two nodes: today at 5 minutes, hourly DA and RT spreads over N days with stats, or five-minute spreads from the archive. Use the congestion component for an FTR-style view |
| `CMP` | Compare nodes | Up to eight nodes on one chart: today's 5-minute RT, or hourly RT or DA over N days, with a latest/average/range row each (`CMP MINN.HUB MICHIGAN.HUB 7`) |
| `SEAM` | Seams & interfaces | PJM's CTS forecast at the PJM interface against MISO's price there (with the spread and which way it favours flows), and RT and DA prices at all 22 interface nodes (PJM, SPP, TVA, Ontario…) |
| `WL` | Watchlist | Your favourite nodes: RT vs DA, 5-min change and today's sparkline. Add from here, with `WL <node>`, or with ☆ in GP. Saved to config |
| `ASM` | Ancillary MCPs | Regulation, spinning, supplemental, short-term reserve and ramp MCPs by zone |
| `LOAD` | System load | 5-min actual vs MTLF forecast vs DA cleared, with forecast error |
| `CAP` | Capacity & headroom | Committed capacity vs demand, forecasts, available capacity, real-time RSG commitments and tomorrow's short-term reserve requirement |
| `FUEL` | Fuel mix | Generation by fuel now, plus a stacked chart of the day |
| `GAS` | Natural gas | Henry Hub spot (EIA, no key needed) with its recent change and range, and for each hub the market heat rate and spark spread its DA on-peak price implies (`HH`) |
| `RENEW` | Wind & solar | Hourly forecast vs actual for today and tomorrow, with forecast bias |
| `NSI` | Interchange | Net scheduled interchange by neighbour, actual (metered) interchange and the inadvertent gap, plus 5-min history |
| `RDT` | Regional transfer | North-South regional directional transfer over the last day against its limits, with utilisation |
| `ACE` | Area control error | 30-second ACE over the last two hours: how far generation is from balancing load |
| `WX` | Weather | Now, today's and tomorrow's high/low, dew point, wind and the 48-hour trend for a city in each MISO zone (National Weather Service); hourly chart for all or one |
| `CONS` | Binding constraints | RT binding constraints, shadow prices and how long each has bound; reserve and sub-regional constraints |
| `BCH` | Constraint history | A day's binding constraints in DA and RT side by side, matched by MISO's constraint ID: hours bound, cost ($/MW) and peak shadow price, sortable and filterable, with the selected constraint's DA and RT shadow prices through the day (`BCH 2026-09-30`) |
| `OUT` | Generation outages | Planned, unplanned, forced and derated MW for ±5 days |
| `ALRT` | Alerts | Alerts on RT price (any node), spreads, constraints, N–S transfer vs limit, load vs forecast and ACE: add rules, see which hold, and what fired. A firing rule flashes the taskbar, shows a ⚠ badge, and (when the terminal is in the background) a Windows notification; a burst becomes one summary |
| `LOG` | Data feeds & log | Every feed's freshness and errors, fetch activity, cache and file locations |
| `SET` | Settings | Zoom, price highlighting thresholds, history length, cache cap, request limits and MISO endpoints, saved to config.toml |
| `THEME` | Themes | Switch, preview and contrast-check themes, or copy one to edit |
| `HELP` | Help | Functions, keyboard shortcuts, data notes |

Double-click a tab title (or press `Ctrl+M`) to zoom that panel to the whole
window; `Esc` or `Ctrl+M` brings the layout back. Right-click a tab title to
open the panel in its own window (for a second monitor; close it or press
*Dock* to put it back, and it reopens where you left it), or to copy the panel
as an image or save it as a PNG (to `Pictures\MISO Terminal`).

Keyboard: `Ctrl+K` or `Esc` focuses the command line, `Enter` runs it, `Tab`/`↑`/`↓`
pick a suggestion, `F1` opens help, `F5` refreshes every open feed,
`Ctrl+Tab` / `Ctrl+Shift+Tab` cycle tabs, `Ctrl+W` closes one, and
`Ctrl+Shift+L` resets the layout. Function keys open functions: F2 HOME, F3 LMP,
F4 MAP, F6 WL, F7 HUBS, F8 WX, F9 ALRT and F10 LOG by default. Remap them, or
bind any command (`F11 = "GP ALTE.ALTE 14"`), under `[ui.hotkeys]` in config.toml.

## Data

Everything comes from MISO's public sources:

- The **real-time data API** at <https://public-api.misoenergy.org/>, which
  replaced the old `MISORTWDDataBroker` feeds on 2025-12-12. MISO asks that each
  link be polled at most once a minute; the terminal refreshes every 60 s and
  enforces a per-URL minimum interval on top.
- The **daily market reports** at `docs.misoenergy.org/marketreports`
  (`<yyyymmdd>_da_expost_lmp.csv`, `_rt_lmp_final.csv`, `_rt_lmp_prelim.csv`).
  The final RT report trails by about a week; until it lands, GP uses the
  preliminary one and says so.

All times are **market time, EST all year** (UTC-5, no daylight saving).
Five-minute intervals are stamped by their start (00:00 through 23:55).

> For information only. This is not an official MISO product and is not meant
> for operational or settlement decisions.

## Themes

Themes are TOML files with semantic slots: background, surface, text, accent,
positive, negative, chart series, fuel colours, fonts and spacing. Panels only
use those slots, so any theme works with any panel. See [`themes/README.md`](themes/README.md)
for the format.

- **Built-in:** `everforge-dark` (default), `everforge-light`, `amber-terminal`, `high-contrast`.
- **Yours:** run `THEME`, click *Copy to edit*, then edit the file in the themes
  folder. It reloads every time you save. A theme whose `id` matches a built-in
  replaces it. Validation flags unreadable contrast.
- **Fonts:** the three Everforge typefaces (IBM Plex Sans, JetBrains Mono, Space
  Grotesk; all SIL OFL) are bundled. Themes can name any installed font, or a
  font file dropped into the fonts folder.
- **Everforge stays in sync with its tokens.** The two Everforge themes are
  generated from the Everforge design tokens:
  `cargo run -p mt-theme --example sync_everforge -- ..\everforge`

## Development

```powershell
cargo test --workspace            # unit, parser-fixture, UI smoke and visual regression tests
cargo clippy --workspace --all-targets
cargo fmt --all
cargo run -- --offline            # UI work without touching MISO
cargo run -p mt-miso --example capture_fixtures   # re-record fixtures from live MISO
# Render the app offscreen against the fixtures (docs screenshots, reviews):
cargo run -p mt-ui --example render -- docs/screenshots/x.png --run "GP MINN.HUB 7" --zoom
# After an intended visual change, accept the new reference images:
$env:UPDATE_SNAPSHOTS=force; cargo test -p mt-ui --test snapshots
```

The workspace is layered so that each part can change without touching the others:

```
crates/
  mt-core       domain model: prices, load, fuel, constraints, market time (no I/O)
  mt-data       source-agnostic data hub: queries, cache, refresh, transports
  mt-miso       MISO endpoints, response parsers (fixture-tested), queries
  mt-nws        National Weather Service forecasts (the template for non-MISO sources)
  mt-theme      theme model, TOML loading, validation, built-ins (no UI toolkit)
  mt-ui         egui front end: shell, command line, workspace, functions
  miso-terminal the binary: paths, logging, runtime, window
```

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for how data flows and why,
and [`docs/EXTENDING.md`](docs/EXTENDING.md) for step-by-step recipes: adding a
function, a dataset, a non-MISO source, or a theme. CI runs on Windows (fmt,
clippy, tests, release build). A weekly job re-records MISO's feeds and runs the
parsers against them. Pushing a `v*` tag builds a Windows zip release.

## TODO

Roughly in priority order within each area. The architecture docs explain where
each piece plugs in.

### Data
- [x] Yesterday's five-minute RT alongside today's in GP (MISO's `Previous` feed, on demand).
- [x] Keep today's five-minute prices across restarts (sparklines in seconds instead
      of waiting up to a minute for MISO's rolling feed).
- [x] A five-minute archive over many days (`GP <node> 7 5MIN`), built from the daily stores
      saved while the app runs, with yesterday completed from MISO's previous-day feed.
- [ ] Backfill the five-minute archive for days the app was not running (MISO only
      publishes today and yesterday at five minutes; older days would need another source).
- [x] **Long history from a local archive:** `uv run tools/export_history.py` exports
      hourly DA and RT (since 2023-01-01) for the hubs, load zones and interfaces, or any
      `--nodes`, from the Energy-Pricing-Journalist DuckDB into the terminal's cache.
      GP, SPRD, CMP and HUBS then reach back up to about four years (`GP MINN.HUB 365`),
      downloading only the days after the archive ends.
- [x] Hub **ex-ante LMPs** (next interval) on HOME.
- [x] New MISO feeds: `ACE` and `RDT` (regional directional transfer vs limits).
- [x] Net actual interchange (NSI) and reserve / sub-regional constraints (CONS).
- [x] More MISO feeds: RSG commitments and the next-day STR requirement (CAP), CTS (SEAM).
- [x] DA/RT binding-constraint history (`BCH`, from MISO's daily `.xls` reports).
- [ ] More market reports: DA ex-ante LMPs, MCP history, load-zone summaries.
- [x] Node types for every CP node (hub, load zone, interface, generator), from the DA report, in LMP.
- [ ] More node metadata: zone and LBA for every CP node (MISO publishes no feed for it;
      `geo.rs` carries positions for the 317 mapped ones).
- [x] Weather by MISO zone (`WX`, National Weather Service): the first non-MISO source (`mt-nws`).
- [x] Prices at the seams: every interface node, and PJM's CTS forecast at the PJM interface (`SEAM`).
- [x] Gas prices: Henry Hub daily spot from EIA's public workbook (`GAS`, `mt-eia`).
- [ ] Neighbouring ISOs' own prices (SPP's public marketplace files; PJM Data Miner
      needs a key), and delivered gas at MISO hubs (Chicago, MichCon) if a free source exists.
- [x] Disk-cache size cap with least-recently-used pruning (`data.cache_max_mb`); the
      five-minute archive is exempt and kept for `data.archive_days` (default 90).
- [ ] An in-app switch between live data and offline replay.
- [ ] MISO's operational notifications (Max Gen, capacity advisories, conservative
      operations). Not in the public API, and misoenergy.org serves them behind a
      browser challenge, so there is no clean source today.

### Functions and UI
- [x] `MAP`: node price map (LMP, congestion, loss, DA, DART) built from MISO's own node positions; click to GP.
- [x] MAP: an interpolated price surface over the footprint.
- [x] MAP: the transmission backbone (HIFLD, 230 kV and up, simplified to ~1.5 km).
- [x] `SPRD A B`: node-to-node spreads (today at 5 minutes, hourly history, by component).
- [x] Hour × day heatmap for a node or a spread (`GP <node> 14 HEAT`, `SPRD A B 14 HEAT`).
- [x] Price duration curves for a node or spread (`GP <node> 30 DUR`).
- [x] **Alerts** (`ALRT`): per-node price thresholds and constraint conditions,
      edge-triggered, with a taskbar flash and an in-app badge.
- [x] More alert kinds: spreads, RDT near its limit, load above forecast, ACE.
- [x] Windows toast notifications for alerts (toggle and a test button in ALRT).
- [x] `WL` watchlist of favourite nodes, editable in-app (☆ in GP) and saved to config.
- [x] Pop a tab out into its own OS window for multi-monitor desks (saved with the
      layout, position included).
- [x] Copy tables as CSV (LMP, WL, GP and SPRD history).
- [x] Copy a panel to the clipboard as an image, or save it as PNG (right-click its tab).
- [x] `SET`: edit `config.toml` values in-app.
- [x] Command line: usage hints, fuzzy matching, and configurable function keys.
- [x] Contain panel panics to their tab (logged, with a *Reload panel* button).

### Themes
- [x] Follow the Windows light/dark setting with a chosen light/dark pair (`THEME`).
- [x] Everforge's chamfered corners (`cut-md`) on hero tiles (a theme `chamfer` value).
- [ ] The chamfer on the active tab (needs custom tab painting in egui_dock).
- [x] A high-contrast accessibility theme (`high-contrast`).
- [ ] Optionally make `miso-terminal` a target in Everforge's own `build.py`
      (per its port policy) instead of the sync example here.

### Windows distribution
- [x] Per-user install script in the release zip (`packaging/install.ps1`).
- [ ] A proper installer (MSI via `cargo-wix`, or MSIX for winget).
- [ ] **Code signing.** Unsigned executables trip SmartScreen and Smart App Control.
- [x] Single instance: a second launch hands its `--run` commands to the open window.
- [ ] Jump-list entries for favourite functions (taskbar right-click).
- [ ] An update check against GitHub releases.

### Engineering
- [x] Offscreen rendering of the real app to PNG (`cargo run -p mt-ui --example render`),
      with egui_kittest and wgpu: no window, real input and rendering.
- [x] Visual regression tests (`tests/snapshots.rs`): the layout in every theme plus
      zoomed MAP, DAM, SEAM and GP, rendered with the clock frozen at the fixtures'
      recording time and compared with committed images.
- [x] A weekly CI job (`drift.yml`) that records live responses and runs every
      parser against them, to catch MISO format changes early.
- [x] A rolling-feed benchmark (`cargo run --release -p mt-miso --example bench_rolling`): a full
      day (29 MB, 530k rows) parses in ~0.4 s and builds in ~50 ms; the download dominates.
- [x] GitHub repository (private) with CI on every push.
- [ ] Make the repository public (the owner's call).
- [ ] Choose a licence (the bundled fonts are OFL; see `assets/fonts/`).
