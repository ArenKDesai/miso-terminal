# Markets plan: news, market data, trading and portfolio

A plan: headlines from the Financial Times, Bloomberg and the Washington Post;
US stock, ETF and options data from Alpaca; paper trading through Alpaca, with
live trading later; and account and portfolio tracking. Phase 0 (foundations),
Phase 1 (news) and Phase 2 (market data) are built (2026-10-04); the later phases
are not yet.
Facts about the sources were checked on 2026-10-04. The README's roadmap tracks
progress against the phases below.

## Decisions

| | Decision | Why |
|---|---|---|
| News sources | Public RSS headline feeds from FT, Bloomberg and the Washington Post, plus Alpaca's ticker-tagged news (Benzinga) | None of the three offers an individual-subscriber API; their RSS feeds work today |
| Subscriber articles | Open in the default browser, where the reader is signed in | The terminal never handles news passwords, and it stays within each publisher's terms |
| Alpaca client | Our own direct HTTP and WebSocket client, a new `mt-alpaca` crate on `mt-data` | See below |
| Data plan | Alpaca's free plan, with the feed (`iex`/`sip`) a setting | IEX real-time, consolidated SIP 15 minutes delayed, 200 requests a minute, 30 streamed symbols |
| Assets | US stocks and ETFs, then options | Options are their own phase so they cannot delay stocks |
| Live orders | Every order confirmed, with notional caps and a kill switch | Paper uses the same confirm flow, so habits carry over |
| Instrument syntax | Ticker plus market code, Bloomberg-style: `XLU US` | Never confused with a MISO node such as `AECI` or `TVA` |
| CI checks | A dedicated throwaway paper account, its keys in the repository secrets `ALPACA_PAPER_KEY_ID` and `ALPACA_PAPER_SECRET_KEY` | Alpaca needs keys even for market data; the account holds only paper money and is nobody's real account |

Why our own Alpaca client:

- **Python SDK** (`alpaca-py`, 0.44): would need a Python sidecar process and
  inter-process plumbing in a single-exe Windows app.
- **`apca`** (Rust, 0.31, maintained): good coverage, but it brings its own
  hyper client, bypassing `FetchCtx`: fixtures and offline replay, the event
  log, polite pacing. It is a useful reference for endpoint shapes and edge
  cases.
- **Direct HTTP**: Alpaca's REST API is JSON with two auth headers, and about 25
  endpoints are needed. It is the `mt-miso`/`mt-eia` pattern, tested the same
  way.

## Phase 0: Foundations (built)

Built on 2026-10-04; [the architecture notes](ARCHITECTURE.md#requests-budgets-and-secrets)
describe each piece and [EXTENDING](EXTENDING.md#add-a-streaming-source) the recipes.

- **`mt-data` grows up.** `Request` carries a method, headers and a body;
  secret header values print as `***` in the event log. `Budget`s cap requests
  per group of hosts (Alpaca's will be about 180 of 200 a minute, shared by
  every panel), wait for a free slot rather than fail, and close for a `429`'s
  `Retry-After`. GETs are conditional (ETag, If-Modified-Since) whenever the
  server sends validators. `FetchCtx::send` returns every status with its body,
  for the order desk. **Streams** (`mt_data::Stream`): WebSocket connections
  owned by the hub, one per endpoint and shared by all panels, subscribed to the
  union of their topics, with reconnect, resubscribe, keep-alive pings, and no
  retries after a refused login until the keys change. LOG lists streams and
  budgets.
- **Secrets** live in Windows Credential Manager, through keyring-core and its
  Windows store (the `keyring` crate's own advice for applications), one entry
  per key: `miso-terminal/alpaca/paper/key-id` and `…/secret-key`. Live keys
  will get their own `alpaca/live/…` entries, and SET will only show them once
  live trading exists. SET stores and removes the paper keys; nothing goes in
  `config.toml`, saved state or logs.
- **Instruments** (`mt_core::instrument`). Securities are a ticker plus `US`,
  Bloomberg-style (`XLU US`, also `XLU US Equity`); options are OCC symbols. The
  command line joins a security into one argument (`XLU US GP 30` is `GP` with
  `XLU US` and `30`; `MINN.HUB GP 14` now works the same way for nodes) and
  refuses securities for functions that take only nodes. Completion from
  Alpaca's asset list moved to Phase 2, since it needs the Alpaca client.
- **Exchange time** (`mt_core::exchange`): America/New_York via `chrono-tz`,
  clock changes, and pre-market, regular and after-hours sessions for a day's
  hours, kept apart from `mt_core::time`.
- **Money** (`mt_core::money`): `rust_decimal` with parsing (no exponents),
  `$1,234.50` formatting, quantities and tick rounding. Broker JSON (strings or
  numbers) deserialises exactly.
- **Wording.** The README and CLAUDE.md now say read-only for MISO, with
  trading (when it comes) only through Alpaca and paper by default.

## Phase 1: News (built)

Built on 2026-10-04. It needs no Alpaca; company news (`CN XLU US`, from
Alpaca's news API) moved to Phase 2 with the rest of the Alpaca client.

| Source | Feeds (13) | Items (2026-10-04) |
|---|---|---|
| Financial Times | `ft.com/rss/home/international` (`/rss/home` redirects there), and `?format=rss` on `markets`, `energy`, `global-economy` | 10 to 25 each |
| Bloomberg | `www.bloomberg.com/feeds/<section>/news.rss` for markets, economics, industries, politics, technology (the older `feeds.bloomberg.com` addresses redirect there) | 3 to 20 each (`green` returns 404, `wealth` is empty) |
| Washington Post | `feeds.washingtonpost.com/rss/<section>` for business, business/economy, politics, national | 3 to 14 each; the all-stories feed timed out |

- **`mt-news`** parses RSS 2.0, RSS 1.0 and Atom (`quick-xml`) into
  `mt_core::news::Headline`s (title, summary as plain text, link, author,
  time, source, sections), and never keeps article bodies (`content:encoded`,
  Atom `content`). A story's identity is the publisher's id scoped to its site,
  or its link with tracking parameters removed (FT adds `?syn-…`); the same
  story in two feeds shows once with both sections. Each feed is a `FeedQuery`
  (so LOG lists each one's health), refreshed every 5 minutes or the feed's
  own `ttl` if longer (FT asks for 15), and merged into what it already had.
  Headlines are kept for `news.keep_days` (21) in the disk cache under
  `local://news/<feed id>`, shown at launch before the first fetch, and
  searchable across restarts. A request budget spaces FT requests a second
  apart, as its robots.txt asks; every feed path is allowed by its site's
  robots.txt.
- **Functions:** `TOP` (the top-stories feeds merged, newest first); `NEWS
  [source] [words]` (every headline kept; `NEWS FT`, `NEWS natural gas`);
  `NI <topic>` from keyword rules (ENERGY, POWER, GRID, GAS, OIL, UTILITIES,
  POLICY, CLIMATE, MACRO; `NI` alone lists them with counts). A keyword in
  capitals matches capitals only (`MISO` is not the soup) and `utilit*` matches
  any ending. All three share one browser: publisher and search filters,
  unread only, ↑/↓ and Enter, a preview with the summary, *Open in browser*,
  *Copy link* and read marks (kept in the cache, `local://news-read/ids`).
  Only `http`/`https` links are opened. HOME has a top-stories section, and
  ALRT a *Headline mentions* rule: it fires for each new matching headline
  published in the last hour.
- **Configuration:** the feed and topic lists are built in, so a release can
  fix a feed that moved. `[news]` in config.toml turns feeds off
  (`disabled_feeds`, also checkboxes in SET), adds or replaces feeds
  (`[[news.feeds]]`: id, source, section, url, top) and topics
  (`[[news.topics]]`: name, keywords), and sets `keep_days`.
- **Health.** Every feed shows in LOG; panels show how many feeds are failing
  and which. `cargo run -p mt-news --example capture_news` records them for the
  repository with every story's words replaced by sample text (the structure
  stays: CDATA, ids, dates), so the repository and release zips never
  republish headlines. The weekly drift job records them live (`--verbatim`),
  runs the parser tests on them (every feed parses; every publisher has
  headlines) and keeps nothing. Feeds do disappear: the
  Washington Post's homepage feed stopped in July 2026.
- **Bandwidth.** Phase 0's conditional GETs apply automatically: Bloomberg's
  feeds send ETags and answer `304` when unchanged; FT's send ETags but
  answered `200` again (checked 2026-10-04); the Washington Post's send none.
- **Terms.** Headlines and summaries only, always attributed and linked; no
  stored or scraped article text.

## Phase 2: Market data (stocks and ETFs) (built)

Built on 2026-10-04, in a new crate, `mt-alpaca` (our own client on `mt-data`,
as decided above). Paths and message formats were taken from Alpaca's own
`alpaca-py` client and are checked weekly against live answers by the drift job.

| Dataset | Endpoint | Refresh |
|---|---|---|
| Snapshots (latest trade, quote, minute bar, today's and the previous daily bar) | `data.alpaca.markets/v2/stocks/snapshots`, 100 symbols a request | a minute in any session, ten minutes when closed |
| Bars (1 and 15 minutes, daily), many symbols a request, every page followed | `data.alpaca.markets/v2/stocks/bars` | a minute while trading; daily bars every 30 minutes |
| Asset list (active US stocks and ETFs) | `paper-api.alpaca.markets/v2/assets` | daily, kept in the cache (`local://alpaca/assets`) |
| Clock and calendar | `paper-api.alpaca.markets/v2/clock`, `/v2/calendar` | 5 minutes; 12 hours |
| Company news (Benzinga) | `data.alpaca.markets/v1beta1/news`, never the article text | 5 minutes |
| Live trades, quotes and minute bars | `wss://stream.data.alpaca.markets/v2/{iex,delayed_sip,sip}` | streamed |
| Live news | `wss://stream.data.alpaca.markets/v1beta1/news` | streamed |

- **Feeds.** `[markets] feed` (and SET): `iex` (the default: real time, IEX
  alone), `delayed_sip` (every exchange, 15 minutes late, free) or `sip` (paid).
  Daily history always comes from every exchange: the free plan serves the
  consolidated tape up to 15 minutes ago, so daily bars ask for it with an end
  16 minutes back. Every figure carries its feed's label.
- **The free plan's limits.** One budget of 180 requests a minute covers every
  Alpaca host. The stream may hold 30 trade and quote subscriptions in all: a
  first live run subscribed both for the 23 symbols of the default list and was
  refused with `405 symbol limit exceeded`, so the limit counts a symbol's
  trades and quotes separately. `MarketStream` subscribes every symbol's trades
  first, then quotes while room remains (`[markets] stream_limit`, 30), and
  minute bars for all; what does not fit is held back and admitted when room
  frees up, and a `405` anyway halves the limit and reconnects. Bid and ask
  still refresh each minute from the snapshots. One connection per stream
  endpoint and account, shared by every panel (a second program on the same
  account gets `406`).
- **Rows.** `mt_alpaca::board::row` brings each minute-old snapshot up to date
  with what the stream has delivered since: the newest trade or bar sets the last
  price, later prices widen the day's range, and the change is measured from the
  close before the last trade's session (Monday's first trade against Friday's
  close, not Thursday's).
- **Functions:** `Q` (quote monitor: last, change, bid and ask, volume, the day's
  range, today's 15-minute chart, sortable, `Q POWER`, `Q WL`, `Q XEL US AEE US`);
  `GP XLU US` (the latest session minute by minute against the previous close,
  `5` days at 15 minutes, or daily closes for up to ten years, with volume); `DES`
  (the asset record, today's trading, the 52-week range and returns); `CN`
  (company news in the Phase 1 headline browser, REST plus the news stream); and
  securities in `WL` beside nodes. Charts of securities are in New York time
  (`widgets::chart::exchange_plot`), never MISO's EST. Panels without keys explain
  how to add them; the status bar shows the US market's session (from the
  calendar, overruled by Alpaca's clock when they disagree).
- **Completion.** The command line offers tickers and company names from the
  asset list for functions that take securities, `GP XLU US` for a bare ticker,
  and the functions that take a security after one (`XLU US D…`). A bare
  security opens `GP`, as a bare node does; options are refused until `OMON`.
- **The energy angle.** Built-in lists for `Q`: `POWER` (all of the below, the
  default), `UTILITIES` (AEE, XEL, LNT, WEC, DTE, CMS, ETR, CNP, MGEE, NI, OTTR),
  `GENERATORS` (VST, NRG, CEG, TLN), `ETFS` (XLU, XLE, UNG) and `GAS` (UNG, EQT, AR,
  RRC, CTRA, EXE); `[[markets.lists]]` adds or replaces lists. Still to come: a
  stock against a MISO hub price on one chart.
- **Recordings.** `cargo run -p mt-alpaca --example capture_alpaca` records every
  dataset through the queries themselves, plus short stream sessions (the live
  feed with the default list, Alpaca's always-on test feed, news). By default
  every price, size and volume is replaced with a synthetic one (a smooth function
  of symbol and time, consistent across files) and every story's words with
  sample text, so the repository holds no licensed market data or articles;
  `--verbatim` is for the drift job. A recording can be picked by a query value
  (`bars@1Day.json`), a `FixtureTransport` feature added for this.

## Phase 3: Account and portfolio

Read-only Alpaca access to a paper account, before any trading, so the read
side and reconciliation are proven first.

- **`PORT`:** positions (quantity, average cost, market value, unrealized and
  day P&L), cash, buying power and equity; options grouped by underlying with
  net delta.
- **`ACCT`:** status, pattern-day-trader flag and day-trade count, margin,
  options level.
- **`PNL`:** the equity curve from portfolio history (1 day, 1 month, 1 year).
- **`ACT`:** activities: fills, dividends, fees, exercises, assignments,
  expiries.
- Trade updates and the quote stream keep figures live, with a full re-sync
  every minute and on reconnect. An equity and day-P&L tile on HOME.
- A **PAPER** band across the status bar whenever Alpaca is connected; live
  gets a different, unmistakable band.

## Phase 4: Paper trading (stocks)

- **A separate order desk**, never the data hub: no caching and no blind
  retries. Every order gets a `client_order_id` generated before it is sent;
  after a timeout it is looked up by that id before anything is resent, so an
  order can never go in twice.
- **Order states** come from the `trade_updates` stream
  (`wss://paper-api.alpaca.markets/stream`), reconciled with the order list on
  every reconnect.
- **The ticket.** `BUY XLU US 10 LMT 82.50 DAY` opens a pre-filled ticket
  showing the estimated cost, buying power afterwards and day-trade warnings.
  Only its **Confirm** places the order. `ORD` is the blotter (open, filled,
  cancelled; cancel and replace).
- **A fixed rule:** commands that arrive from outside the window (`--run`,
  commands forwarded by a second launch, alerts) can only open a ticket, never
  place an order. No automated trading.
- **Guardrails**, all settings with tests: per-order and daily notional caps,
  maximum position per symbol, a price collar (limit within a percentage of the
  last price), no market orders outside regular hours, a fat-finger quantity
  check, a **restricted list** of symbols that cannot be traded, and a **kill
  switch** that cancels every open order and optionally flattens positions.
- **Audit log:** every order request and response, appended to a file in local
  app data.

## Phase 5: Options

- **Data:** chains and snapshots. On the free plan Alpaca's options prices come
  from its indicative feed, not OPRA, and are labelled so. `OMON` shows a chain
  by expiry with bid/ask, implied volatility and greeks.
- **Paper trading:** single-leg first, then multi-leg spreads (Alpaca's level
  3, enabled in paper). The ticket enforces Alpaca's rules: whole contracts, day
  or GTC only, no extended hours, stop orders single-leg only.
- In-the-money contracts are exercised automatically at expiry: positions
  expiring today carry a warning.

## Phase 6: Live trading

Only after paper trading has run cleanly for a few weeks.

- Live keys are stored separately; live is off by default, and turning it on
  takes a typed confirmation phrase in SET and a restart.
- One account trades per session, and the status band turns red.
- Every guardrail stays on, and every order needs its confirm.
- A security review of the order path before this ships.

## Testing

- Fixtures for every endpoint (account numbers redacted) and recorded stream
  sessions, so offline mode and the tests replay them.
- A fake broker in the tests: duplicate prevention, reconnect reconciliation,
  partial fills and rejections, every guardrail.
- The weekly drift job records live Alpaca responses with the CI paper
  account's keys (repository secrets `ALPACA_PAPER_KEY_ID`,
  `ALPACA_PAPER_SECRET_KEY`) and runs the parsers on them, as it does for
  MISO. A manually run job places and cancels paper orders on that account;
  never live. Installed copies of the terminal never see these keys: each user
  adds their own.
- Visual snapshots for each new panel.

## Risks

- **Feeds change or vanish:** feed health in LOG, the drift job, an editable
  feed list.
- **Free-plan limits:** IEX's thin volume, indicative options prices and 200
  requests a minute; met with labels, the shared request budget and streams.
- **Options scope:** its own phase.
- **Compliance:** anyone working in energy or financial markets should check
  their employer's personal-trading policy before trading live; some require
  pre-clearance or forbid certain names. The restricted list is there for that.

## Sources

- Alpaca: [Market Data API](https://docs.alpaca.markets/us/docs/about-market-data-api),
  [streaming market data](https://docs.alpaca.markets/us/docs/streaming-market-data),
  [trade updates](https://docs.alpaca.markets/us/docs/websocket-streaming),
  [paper trading](https://docs.alpaca.markets/us/docs/paper-trading),
  [options trading](https://docs.alpaca.markets/us/docs/options-trading),
  [multi-leg options in paper](https://docs.alpaca.markets/us/v1.1/changelog/multi-leg-level-3-options-trading-in-paper),
  [News API](https://alpaca.markets/blog/introducing-news-api-for-real-time-fiancial-news/)
- [The `apca` crate](https://lib.rs/crates/apca)
- RSS feed lists: [Washington Post](https://feeder.co/knowledge-base/rss-content/washington-post-rss-feeds/),
  [Financial Times](https://rss.feedspot.com/financial_times_rss_feeds/),
  [Bloomberg](https://rss.feedspot.com/bloomberg_rss_feeds/)
