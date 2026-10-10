# MISO Terminal

A Bloomberg-style information terminal for the Midcontinent ISO, written in
Rust: prices, load, generation, interchange, constraints, the seams, gas,
weather, the news, and the energy stocks and options beside them, in one
keyboard-driven, tiled workspace. It is read-only for MISO. Trading goes only
through Alpaca, on a paper account for now, and every order is checked against
your limits and confirmed on a ticket.

![MISO Terminal in its default theme](docs/screenshots/home-default.png)

- **Command line first.** Type `LMP`, `GP MINN.HUB` or `MINN.HUB GP 14`, or
  just a node name, and press Enter. Completion covers every function, node and
  ticker.
- **Tiled, tabbed workspace.** Split panes, zoom one panel (`Ctrl+M`), pop one
  out onto a second monitor, or go full screen (`F11`). The layout is saved.
- **Live, with history.** Real-time feeds refresh once a minute (MISO's limit).
  Daily reports are kept on disk, and SET can fill the price history back to
  2023. Alerts arrive as Windows notifications.
- **Themed.** Default (the trading-desk look), Default Light and High Contrast
  built in, more in one click from the [gallery](themes/README.md#gallery), and
  your own as a TOML file that reloads as you edit it.
- **Built for Windows.** One `.exe`, TLS through the Windows certificate store
  (corporate proxies work), per-user or portable data. The non-UI crates also
  build and test on Linux.

| | |
|---|---|
| ![GP history](docs/screenshots/gp-history.png) | ![Default Light](docs/screenshots/default-light.png) |

![MAP: real-time congestion across the footprint](docs/screenshots/map.png)

## Principles

MISO Terminal is a community project, and these hold for every change:

- **Your data stays yours.** No telemetry, no accounts with us, nothing sent
  anywhere but the sources you ask for. Keys live in Windows Credential
  Manager, never in files or logs. The [privacy policy](docs/PRIVACY.md)
  lists every service the terminal contacts.
- **Good citizens of the web.** Every source's robots.txt, terms and rate
  limits are respected (MISO asks for at most one request a minute per feed,
  and gets it). No scraping around paywalls or limits.
- **Free and open, and kept that way.** The AGPL-3.0 licence makes anyone who
  distributes or serves a modified version share their source too.

## Install (Windows)

Download `miso-terminal-<version>-windows-x64.zip` from the
[Releases page](https://github.com/ArenKDesai/miso-terminal/releases); its
notes give the SHA-256 and how to verify that this repository built it. Unzip
it, and in that folder (*Open in Terminal* from its right-click menu) run:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1           # Start Menu shortcut
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Desktop  # plus a desktop shortcut
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall
```

The script is not signed yet, so run it this way: where PowerShell runs only
signed scripts, double-clicking it fails. It installs for the current user (no
administrator rights) to `%LOCALAPPDATA%\Programs\MISO Terminal`, and
uninstalling keeps your settings and data. Or run `miso-terminal.exe` from the
folder; an empty file named `portable` beside it keeps everything in a `data`
folder there.

Releases are not code-signed ([signing](docs/RELEASES.md#signing)), so
SmartScreen may warn on first launch (*More info*, then *Run anyway*), and
Smart App Control blocks unsigned programs outright.

## Build from source (Windows)

```powershell
# Needs Rust (https://rustup.rs) and the MSVC build tools; rustup fetches the
# compiler version rust-toolchain.toml names.
git clone https://github.com/ArenKDesai/miso-terminal
cd miso-terminal
cargo run --release
```

| Flag | What it does |
|---|---|
| `--offline [DIR]` | Replay the recorded responses in `fixtures/` instead of calling MISO: demos, UI work, or MISO down |
| `--home DIR` | Portable mode: everything in `DIR` (also `MISO_TERMINAL_HOME`, or a `portable` file beside the exe) |
| `--run "CMD"` | Run a command at startup (repeatable), e.g. from a desktop shortcut; if the terminal is open, it runs there |
| `--reset-layout` | Start from the default layout |
| `--reset-config` | Start from the default settings; the old `config.toml` is kept as `config.toml.bak` |
| `--new-instance` | Open a second window anyway (normally one per home, so MISO is polled once) |

Settings, themes and fonts live in `%APPDATA%\MISO Terminal\config`; the cache,
logs, order audit log and layout in `%LOCALAPPDATA%\MISO Terminal`. `LOG` shows
the paths and opens them.

## Functions

New to the terminal? The [tutorials](docs/TUTORIALS.md) walk through it.

| Code | Name | What it shows |
|---|---|---|
| `HOME` | Launchpad | Demand, marginal energy, interchange, generation, hub prices with sparklines, fuel mix, constraints, weather, top stories and the paper account's equity |
| `LMP` | LMP monitor | ~300 key nodes (`LMP ALL`: all ~2,600) with RT, DA, RT − DA, congestion and loss; sort and filter |
| `HUBS` | Hub statistics | The eight trading hubs over N days: averages, on- and off-peak, volatility, extremes |
| `DAM` | Day-ahead strip | Hourly DA at the hubs for a day, with block averages, or the change from the day before |
| `MAP` | Price map | Every mapped node over a price surface and the transmission backbone, by LMP, congestion, loss, DA or RT − DA |
| `GP` | Graph price | A node: today at five minutes against DA, N days hourly, a heatmap (`HEAT`), duration curves (`DUR`) or past five-minute days (`5MIN`), by component. A security: today, a few days or up to ten years, with studies (moving averages, Bollinger bands, momentum, RSI, MACD) |
| `FCST` | Forecast | Tomorrow's DA or RT price at a node (or RT − DA) as a fan chart with 50% and 80% bands, an hourly table and each model's record against repeating a recent day; for a security, the range its close may take over twenty trading days (`FCST MINN.HUB RT`, `FCST XLU US`) |
| `SPRD` | Node spread | A − B between two nodes, with GP's views; congestion gives an FTR-style view |
| `CMP` | Compare | Up to eight nodes on one chart; with securities, their prices above the nodes and how they move together (`CMP XEL US MINN.HUB 30`) |
| `SEAM` | Seams & interfaces | PJM's CTS forecast against MISO's price, and every interface node |
| `WL` | Watchlist | Your nodes and securities, with today's sparklines |
| `ASM` | Ancillary MCPs | Regulation and reserve MCPs by zone |
| `LOAD` | System load | Actual against MISO's forecast and DA cleared |
| `CAP` | Capacity & headroom | Committed capacity against demand, RSG commitments, tomorrow's reserve requirement |
| `FUEL` | Fuel mix | Generation by fuel, now and through the day |
| `GAS` | Natural gas | Henry Hub spot (EIA), and each hub's implied heat rate and spark spread |
| `RENEW` | Wind & solar | Forecast against actual, today and tomorrow |
| `NSI` | Interchange | Scheduled and actual interchange by neighbour |
| `RDT` | Regional transfer | North-South transfer against its limits |
| `ACE` | Area control error | ACE over the last two hours |
| `WX` | Weather | National Weather Service forecasts for a city in each MISO zone |
| `CONS` | Binding constraints | RT binding constraints and shadow prices; reserve and sub-regional ones |
| `BCH` | Constraint history | A day's DA and RT binding constraints side by side (`BCH 2026-09-30`) |
| `OUT` | Generation outages | Planned and forced outages, ±5 days |
| `Q` | Quote monitor | Live stock and ETF prices for a list (`Q`, `Q UTILITIES`, `Q XEL US AEE US`) |
| `DES` | Security description | A security's profile, today's trading, 52-week range and returns |
| `BETA` | Beta | How a security moves with the S&P 500 and its industry: beta, alpha, R², a scatter of returns and a rolling beta, from total returns over one to five years (`BETA VST US`, `BETA XEL US XLU US 2Y`) |
| `OMON` | Option monitor | An option chain by expiry, with greeks; click a price to trade, right-click to build a spread |
| `TOP` | Top stories | The Financial Times', Bloomberg's and the Washington Post's top stories |
| `NEWS` | News search | Every headline, kept three weeks, by publisher or words |
| `NI` | News by topic | Headlines by keyword topic (`NI POWER`); add your own in config |
| `CN` | Company news | Benzinga's stories on a security, as they arrive |
| `PORT` | Portfolio | The paper account's positions and P&L; options by underlying with net delta |
| `ACCT` | Account | Balances, buying power, margin, day trades and options level |
| `PNL` | Profit and loss | The equity curve, from a day to a year |
| `ACT` | Account activity | Fills, dividends, fees and option events |
| `BUY` | Buy ticket | A stock or option order filled in from the command (`BUY XLU US 10 LMT 44.50`); only its *Confirm* sends it |
| `SELL` | Sell ticket | The same to sell or sell short, or to write a covered call or cash-secured put |
| `MLEG` | Spread ticket | An option spread of two to four legs as one order, with its payoff at expiry |
| `ORD` | Orders | The paper account's orders: cancel, replace, and the kill switch |
| `ALRT` | Alerts | Rules on prices, spreads, constraints, transfers, load, ACE and headlines, with Windows notifications |
| `LOG` | Data feeds & log | Every feed's health, streams, request budgets, the price history's download and file locations |
| `SET` | Settings | Display, data, the price history, news feeds, market data, trading limits, endpoints and API keys |
| `THEME` | Themes | Switch, install, preview and copy themes |
| `HELP` | Help | Functions and keyboard shortcuts |

**Keys.** `Ctrl+K` or `Esc` goes to the command line, `F1` is help, `F5`
refreshes, `Ctrl+Tab` and `Ctrl+W` move between and close tabs, `Ctrl+M`
zooms a panel, `F11` (or `Alt+Enter`) goes full screen, and `F2` to `F10` open
functions; rebind any under `[ui.hotkeys]` in config.toml. Right-click a tab
to open it in its own window, copy it as an image or save it as a PNG. HELP
lists every shortcut.

**Securities** are written ticker then market code (`XLU US`) and options by
OCC symbol (`XLU261218C00082500`), so they never clash with node names such as
`AECI`. Type one alone to chart it (an option opens its chain).

**Orders.** `BUY`, `SELL` and `MLEG` only open a ticket, whoever asks (the
command line, `--run`, a hotkey); only a click on its *Confirm* sends an
order, after it passes Alpaca's rules and your limits under `[trading]` (also
in SET). `ORD`'s kill switch cancels every order and turns trading off, and
every request and answer goes to an audit log. [Trade on
paper](docs/TUTORIALS.md#trade-on-paper) walks through it.

## Data

- **MISO:** the [real-time data API](https://public-api.misoenergy.org/),
  polled at most once a minute per feed, and the daily market reports at
  `docs.misoenergy.org/marketreports`, kept in the price history. RT final
  reports trail by about a week; until then charts show the preliminary ones
  and say so.
- **News:** the public RSS feeds of the Financial Times, Bloomberg and the
  Washington Post. Headlines and summaries only, attributed and linked;
  articles open in your browser, where your subscriptions apply.
- **Alpaca,** with your own free account's keys: stock and ETF prices (real
  time from IEX, or every exchange fifteen minutes late), option chains
  (Alpaca's indicative feed), company news (Benzinga) and the paper account.
  Every price says which feed it came from.
- **Also:** Henry Hub gas from the EIA and weather from the National Weather
  Service.

MISO times are market time, EST all year; securities are shown in New York
time.

> For information only. This is not an official MISO product and is not meant
> for operational or settlement decisions.

## Themes

Themes are TOML files of semantic slots (background, accent, positive, chart
series, fonts and so on), so any theme works with any panel. Three are built
in; the [gallery](themes/README.md#gallery) has Catppuccin, Everforge,
Gruvbox, Monokai, Rosé Pine and Tokyo Night, installed from `THEME`. *Copy to
edit* there starts your own. The [theme guide](themes/README.md) describes
the format.

## Development

```powershell
cargo test --workspace            # unit, parser-fixture, UI smoke and visual regression tests
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- --offline            # UI work without touching MISO
cargo run -p mt-ui --example render -- out.png --run "GP MINN.HUB 7" --zoom   # offscreen render
```

```
crates/
  mt-core       domain model: prices, grid data, market time, securities, money,
                accounts, orders, options and the guardrails (no I/O)
  mt-data       source-agnostic data hub: queries, streams, caches, transports
  mt-miso       MISO endpoints, parsers, queries, the price history
  mt-nws        National Weather Service forecasts
  mt-eia        EIA Henry Hub gas prices
  mt-news       news headlines: RSS and Atom, topics, an archive
  mt-alpaca     Alpaca: market data, streams, the paper account, the order desk
  mt-theme      themes: model, loading, validation (no UI toolkit)
  mt-ui         egui front end: shell, command line, workspace, functions
  miso-terminal the binary: paths, logging, runtime, window
```

[ARCHITECTURE](docs/ARCHITECTURE.md) explains how data flows and why, and
[EXTENDING](docs/EXTENDING.md) has recipes for adding a function, a dataset, a
source, a guardrail or a theme. Every change lands through a pull request
([CONTRIBUTING](CONTRIBUTING.md)); CI runs the tests on Windows and Linux and
builds the docs, and a weekly job checks every parser against live sources.
The [release plan](docs/RELEASES.md) covers versions and releases. The
[documentation site](https://arenkdesai.github.io/miso-terminal/) is built from
this README, `docs/` and `themes/README.md`.

## TODO

What is built is in the [changelog](CHANGELOG.md); this is what is still to
come, roughly in order within each area.

### 0.3.0: analytics
The [analytics plan](docs/ANALYTICS-PLAN.md) has the design.
- [x] **The terminal's own price history:** every daily report kept for every
      node, filled back as far as you like from SET, read by GP, SPRD, CMP and
      HUBS for any window.
- [ ] **`FCST`:** forecasts for DA and RT prices and for securities, with
      bands and a record against naive baselines.
- [ ] **`ASK`:** a Claude assistant with read-only tools over the terminal's
      data, through Claude Desktop or Claude Code (the terminal as an MCP
      server) or in the terminal with an API key.
- [ ] An update check, off by default (a link, never a download).
- [ ] The docs site deploys from published releases, not from `main`.

### After 0.3.0
- [ ] **Live trading (0.4.0),** markets Phase 6: off by default, behind a typed
      confirmation, after the [security review](docs/SECURITY-REVIEW.md)'s
      open findings.

### Data
- [ ] Five-minute history for days the terminal was not running (MISO
      publishes only today and yesterday at five minutes).
- [ ] More market reports: DA ex-ante LMPs, MCP history, load-zone summaries.
- [ ] Zone and LBA for every node (MISO publishes no feed for it).
- [ ] Neighbouring ISOs' prices (SPP's public files; PJM needs a key), and
      delivered gas at MISO hubs if a free source exists.
- [ ] MISO's operational notifications (Max Gen, capacity advisories), which
      have no clean public source today.
- [ ] An in-app switch between live data and offline replay.

### UI and themes
- [ ] Light gallery themes (Catppuccin Latte, Gruvbox Light, Rosé Pine Dawn,
      Tokyo Night Day).
- [ ] The chamfer on the active tab (needs custom tab painting in egui_dock).
- [ ] Jump-list entries for favourite functions (taskbar right-click).
- [ ] Make `miso-terminal` a target in Everforge's own `build.py` instead of
      the sync example here.

## Licence

Copyright (C) 2026 Aren Desai.

MISO Terminal is free software: you can redistribute it and/or modify it under
the terms of the GNU Affero General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option) any
later version. It is distributed in the hope that it will be useful, but
WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
FITNESS FOR A PARTICULAR PURPOSE. See [`LICENSE`](LICENSE) for the full text.

The bundled fonts (IBM Plex Sans, JetBrains Mono, Space Grotesk, Noto Emoji)
are under the SIL Open Font License 1.1, and emoji-icon-font under the MIT
licence; their licences are in [`assets/fonts/`](assets/fonts/).
Market data comes from MISO, the National Weather Service and the EIA, and is
subject to their terms. Stock and ETF prices come from Alpaca under each user's
own account and its terms; the repository's recordings of them hold synthetic
prices. Headlines belong to their publishers (the Financial Times, Bloomberg,
the Washington Post, Benzinga), are shown with their names and links, and are
subject to their terms.
