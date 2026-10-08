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

- **A recent price for orders that trade at once.** When an order would trade
  as soon as it arrives, its price must be no older than `[trading]
  max_price_age_secs` (120 seconds; SET, *Oldest price to trade on*): a market
  order is refused past it, and any other order needs the warning ticked. This
  catches a stopped stream, an option chain that has not refreshed and the
  15-minute-delayed feed.

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
