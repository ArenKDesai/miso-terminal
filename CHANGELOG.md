# Changelog

What changes in each release of MISO Terminal, in user terms: functions,
settings and sources. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html), staying below 1.0
while formats settle (see the [release plan](docs/RELEASES.md#versioning)).

Every pull request with a user-visible change adds its line under
*Unreleased*. A release turns that section into its own, and the release's
notes are that section.

## [Unreleased]

### Added

- **Forecasts in GP and DAM.** GP's *Today* view has *+ forecast*: tomorrow's
  forecast with its 80% band after today's prices (DA until MISO posts it,
  then RT). Until MISO posts tomorrow's day-ahead results, `DAM TOMORROW`
  shows each hub's forecast in the same strip, marked as such.

- **`FCST`: forecasts.** `FCST MINN.HUB` forecasts tomorrow's DA price at a
  node, `FCST MINN.HUB RT` tomorrow's RT, and `DART` the spread between them
  once tomorrow's DA is posted: a fan chart after the last week (the median
  with 50% and 80% bands), tiles, an hourly table, and *Skill*, where every
  model's record over the last eight weeks sits beside repeating a recent
  day. Six models are tried: the same hour on the last day known and a week
  earlier, an hour-by-day profile, exponential smoothing with daily and
  weekly seasonality, gradient-boosted trees and ridge regression (on recent
  prices, congestion, Henry Hub gas, the calendar and, once sixty days are
  kept, the forecasts kept as issued). The one with the lowest error is
  shown, and FCST says when none beats repeating a recent day. Every model
  is backtested only on what was known when its forecast would have been
  made, and the bands come from its past errors, so an 80% band holds about
  80%. `FCST XLU US` shows the range a security's close may take over the
  next twenty trading days (random walk, with drift, the volatility cone,
  GARCH, and a learned model only where it beats the random walk). Models
  train on a background thread when new days arrive, and results are cached
  for the day.

- **Forecasts kept as issued.** For the forecasts coming in `FCST`, the
  terminal keeps MISO's load forecast by zone (its daily report, with the past
  year filled in a report every two seconds), its wind and solar forecasts
  and outage schedule (hourly), and the NWS's temperature forecasts for a
  city in each zone (every three hours), each version with when it was
  issued and stored only when it changes: models can then learn from what
  was known at the time. SET's new *Forecasts kept as issued* section turns
  it off and chooses how long to keep (two years by default, a few tens of
  megabytes); LOG shows each source. On by default, and idle in an offline
  replay.

- **`BETA`: how a security moves with the market and its industry.**
  `BETA XEL US` sets beta, adjusted beta (two thirds beta plus one third of
  1), alpha a year, R², correlation, beta's standard error and the number of
  returns against the S&P 500 (SPY) beside the same against the security's
  industry, with a scatter of returns and the fitted line, and a rolling beta
  against both. *1Y*, *2Y* and *5Y* windows, *Daily* or *Weekly* returns
  (weekly from two years). Returns are total returns, from bars adjusted for
  dividends, paired on the days both trade; alpha is over zero, not over a
  risk-free rate. The built-in lists bring their industry benchmarks (XLU for
  utilities and independent producers, XLE for gas producers); name another on
  the command line (`BETA XEL US XLU US 2Y`), or a list's name for an
  equal-weighted basket of it without the security (`BETA VST US
  GENERATORS`). `[markets.benchmarks]` sets one per security
  (`"VST US" = "GENERATORS"`), and a list under `[[markets.lists]]` can carry
  its own `benchmark`.
- **DES shows a year's beta** against the S&P 500; click it for `BETA`.

- **Studies on GP for securities.** *Studies* over a security's chart adds
  simple and exponential moving averages and Bollinger bands over the price,
  and momentum, rate of change, RSI and MACD in panes between the price and
  the volume. Periods can be changed in the menu, the line above the chart
  shows each study's latest value, and *Copy CSV* includes them. Typed, they
  follow the security: `GP XLU US 365 SMA50 SMA200 RSI14`. A chart keeps its
  studies with the layout; *Use for new charts* saves them as `[markets]
  studies` for every new chart. Enough earlier bars are fetched for a study to
  have a value from the first day shown; intraday studies count bars, not
  minutes.

- **Full screen.** `F11`, `Alt+Enter` or *Full screen* (top right) puts the
  terminal in full screen, over the taskbar, and back. A popped-out window has
  its own *Full screen* button and the same keys, so a second monitor can show
  one panel edge to edge. Windows reopen in full screen if left so. A command
  bound to `F11` under `[ui.hotkeys]` keeps the key; `Alt+Enter` still works.

- **A recent price for orders that trade at once.** When an order would trade
  as soon as it arrives, its price must be no older than `[trading]
  max_price_age_secs` (120 seconds; SET, *Oldest price to trade on*): a market
  order is refused past it, and any other order needs the warning ticked. This
  catches a stopped stream, an option chain that has not refreshed and the
  15-minute-delayed feed.
- **The terminal keeps its own price history.** Every daily DA and RT report
  it downloads is kept for every node (about 170 KB a day per market, outside
  the cache's size cap), so a day is downloaded once and charts reopen
  without waiting. An RT day keeps its preliminary prices until the final
  report replaces them.
- **Download the price history.** SET's new *Price history* section chooses
  how far back to keep (three months by default, up to everything since
  2023-01-01, MISO's first day) and shows what is stored. *Download now*
  fetches the missing days in the background, newest first, one report every
  two seconds (three months is about 230 MB to download and 30 MB on disk);
  *Pause* stops it, and it carries on where it stopped, after a restart too.
  Once done it keeps the window filled, replacing preliminary RT days with
  final ones. Progress shows in the status bar and LOG. Nothing downloads
  until you click *Download now*, and days before the window are removed
  (`[price_history]` in config.toml).
- **Charts reach back as far as the price history.** GP, SPRD, CMP and HUBS
  read stored days from disk for any window, a year and more, without the
  Energy-Pricing-Journalist export; only days missing from the last 90 are
  downloaded. When older days are missing, the chart says how many, with a
  *Price history…* button that opens SET. Stored days take far less memory
  than downloaded ones: a chart keeps only its nodes, not every node of every
  day.

### Changed

- LOG's *Clear cache* clears downloaded reports and responses only; the
  five-minute archive, the price history and saved headlines stay.
- The terminal no longer carries egui's default fonts. Text a theme's own font
  cannot draw falls back to IBM Plex Sans (it was Ubuntu Light), and emoji
  still come from Noto Emoji. Everything in the program is now under
  OSI-approved licences; the font licences are in the zip's `licenses`
  folder.

### Removed

- `tools/export_history.py` and reading what it exported: the price history
  replaces the Energy-Pricing-Journalist export (compared first: they agree
  on every settled price). Files it exported are deleted when the terminal
  starts; *Download now* in SET (*Price history*) fetches the same days from
  MISO.

### Fixed

- After the kill switch, saving other changes in SET no longer turns trading
  back on.
- The order desk itself refuses orders that did not pass their checks, and
  refuses new orders and replacements while trading is off, rather than
  relying on the order tickets.

## [0.2.0] - 2026-10-06

The first release: everything built before it, with markets phases 0 to 5.

### Added

- **MISO prices.** `LMP` (about 300 key nodes, or every CP node with
  `LMP ALL`), `HUBS`, `DAM`, `MAP` (node prices over an interpolated price
  surface and the transmission backbone), `GP` (today at five minutes against
  the DA staircase, N days hourly, heatmaps, duration curves and five-minute
  history), `SPRD`, `CMP`, `SEAM` and the `WL` watchlist, from MISO's real-time
  data API and daily market reports.
- **The grid.** `LOAD`, `CAP`, `FUEL`, `RENEW`, `NSI`, `RDT`, `ACE`, `ASM`,
  `CONS`, `BCH` and `OUT`; `GAS` (Henry Hub spot from the EIA, with each hub's
  heat rate and spark spread) and `WX` (National Weather Service forecasts by
  MISO zone); `HOME` as the launchpad.
- **History.** Daily reports cached on disk; a five-minute archive of every
  day the terminal runs (`GP <node> 7 5MIN`); years of hourly history from a
  local archive (`tools/export_history.py`).
- **Alerts** (`ALRT`) on prices, spreads, constraints, the North-South
  transfer, load against forecast, ACE and headlines, with a taskbar flash and
  Windows notifications.
- **News.** `TOP`, `NEWS` and `NI` from the Financial Times', Bloomberg's and
  the Washington Post's RSS feeds: headlines and summaries kept three weeks
  for search, read marks and keyword topics, with articles opening in your
  own browser.
- **Stocks and ETFs**, with your own Alpaca account's keys: `Q`, `GP XLU US`,
  `DES`, `CN` (Benzinga's company news), securities in `WL`, stocks above node
  prices in `CMP`, ticker completion and the market's session in the status
  bar.
- **The paper account:** `PORT`, `ACCT`, `PNL` and `ACT`, an equity tile on
  `HOME` and a PAPER band that never shows a balance.
- **Paper trading:** `BUY` and `SELL` tickets that only their *Confirm*
  button can send, the `ORD` blotter, guardrails under `[trading]` (caps, a
  price collar, a fat-finger check, a restricted list, sessions, buying power
  and day trades), a kill switch and an order audit log.
- **Options:** `OMON` chains with greeks from Alpaca's indicative feed,
  single-contract tickets (covered calls and cash-secured puts, opening and
  closing) and multi-leg spreads in `MLEG`, with the payoff at expiry and the
  option guardrails.
- **The workspace:** a command line with completion and usage hints; tiled,
  tabbed panes that zoom and pop out onto other monitors; configurable
  function keys; tables as CSV and panels as images; `SET` for settings and
  API keys (kept in Windows Credential Manager), `LOG` for feeds and files,
  and `HELP`; both show the version, with the commit in builds that are not
  releases.
- **Themes:** Default (the trading-desk look), Default Light and High
  Contrast built in; a gallery (Catppuccin Mocha, Everforge Dark and Light,
  Gruvbox Dark, Monokai, Rosé Pine, Tokyo Night) to install from `THEME`; your
  own TOML themes, reloaded as you save; following Windows light and dark.
- **Windows:** one window per user (a second launch hands its `--run`
  commands over), `--offline` replay of the recorded data, portable mode
  (`--home`), `--reset-layout`, `--reset-config` and a per-user install
  script, which installs the licences with the program.
- **The download:** a zip with its SHA-256 checksum, a build provenance
  attestation to verify it came from this repository, and the third-party
  licence notices for the Rust crates inside.
- **Documentation:** the [tutorials](docs/TUTORIALS.md) and the
  [docs site](https://arenkdesai.github.io/miso-terminal/).
