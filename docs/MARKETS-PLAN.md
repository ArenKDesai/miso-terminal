# Markets plan: news, market data, trading and portfolio

A plan: headlines from the Financial Times, Bloomberg and the Washington Post;
US stock, ETF and options data from Alpaca; paper trading through Alpaca, with
live trading later; and account and portfolio tracking. Phase 0 (foundations),
Phase 1 (news) and Phase 2 (market data) are built (2026-10-04), and Phase 3
(account and portfolio) and Phase 4 (paper trading) on 2026-10-05; the later
phases are not yet.
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
  first live run subscribed both for the 23 symbols then in the default list and was
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
  RRC, EXE; Coterra stopped trading in May 2026); `[[markets.lists]]` adds or replaces lists.
- **Stocks against hub prices.** `CMP XEL US MINN.HUB 30` puts up to four
  securities (price, or % change for several) above up to four nodes' hourly RT
  or DA, on linked charts spanning the same time in market time (EST), with
  each node's daily average drawn over its hours on daily views. A grid gives
  the correlation of each security's daily return with the day-to-day change in
  each node's daily average price, over the trading days both have (five at
  least). CMP with nodes offers a security picker that opens this view.
- **Recordings.** `cargo run -p mt-alpaca --example capture_alpaca` records every
  dataset through the queries themselves, plus short stream sessions (the live
  feed with the default list, Alpaca's always-on test feed, news). By default
  every price, size and volume is replaced with a synthetic one (a smooth function
  of symbol and time, consistent across files) and every story's words with
  sample text, so the repository holds no licensed market data or articles;
  `--verbatim` is for the drift job. A recording can be picked by a query value
  (`bars@1Day.json`), a `FixtureTransport` feature added for this.

## Phase 3: Account and portfolio (built)

Built on 2026-10-05: read-only access to the paper account, before any
trading, so the read side and reconciliation are proven first. Nothing in this
phase can place an order.

| Dataset | Endpoint | Refresh |
|---|---|---|
| Account: balances, buying power, margin, flags | `paper-api.alpaca.markets/v2/account` | a minute in any session, five when closed, and at once after an order event |
| Positions | `paper-api.alpaca.markets/v2/positions` | the same |
| Equity curve | `/v2/account/portfolio/history`: `1D` at 5 minutes, `1W` hourly (regular hours, P&L from the start of the period), `1M`, `3M`, `1A` daily | a minute (1D); 15 minutes (1W); hourly |
| Activities | `/v2/account/activities`, newest first, 100 a page: three pages at first, then the newest page merged into what is kept (up to 1,000) | two minutes, and after an order event |
| Option greeks for held contracts | `data.alpaca.markets/v1beta1/options/snapshots?feed=indicative` | a minute |
| Order events | `wss://paper-api.alpaca.markets/stream`, `trade_updates` (single JSON objects in **binary** frames, unlike the market-data streams) | streamed |

- **Types** (`mt_core::account`): `Account`, `Position`, `PortfolioHistory`,
  `Activity` (with a category per Alpaca code: fills, dividends, interest, fees,
  transfers, option events, corporate actions) and `OrderEvent`. Every amount
  is a `Decimal` parsed from Alpaca's strings; only the equity curve is `f64`.
- **Live figures.** Alpaca's valuation is the truth and is re-read every
  minute. In between, a stock position whose price has traded since that
  re-read is marked at the newer price: market value and both P&Ls move by the
  change times the quantity, which keeps Alpaca's own reference for the day
  (the previous close, or the entry price for a position opened today). Equity
  and the day's P&L move by the same amounts. Options keep Alpaca's price.
- **Re-sync.** The order-event stream counts logins and events; the app
  watches that token and refreshes the account, positions, activities and
  today's curve as soon as it changes (a reconnect may have missed events), on
  top of the minute's refresh. Only queries something has asked for are
  refreshed.
- **Functions:** `PORT` (equity, the day's and unrealized P&L, cash, buying
  power, long and short value; sortable positions; options grouped by
  underlying with net delta in shares: the shares held plus contracts × 100 ×
  delta, from Alpaca's indicative greeks, with expiry warnings), `ACCT`
  (standing and blocks, balances, buying powers, margin and excess equity,
  the pattern-day-trader flag and day trades used against the three allowed
  under $25,000, options levels), `PNL [1D|1W|1M|3M|1Y]` (the equity curve
  against its starting value, with change, high, low and the deepest drawdown)
  and `ACT [FILLS|DIV|FEES|TRANSFERS|OPTIONS]` (the history, filtered by kind
  or symbol, under the latest order events). HOME has a paper-equity tile.
- **The band.** A strip above the status bar whenever keys are stored: `PAPER`
  on the theme's info colour (`LIVE` will be on its negative colour), what the
  account is, and whether it answers (connected, keys refused, retrying, any
  blocks). It never shows a balance, and ACCT masks the account number, so
  screenshots carry neither. Clicking `PAPER` opens ACCT.
- **Fixtures.** The repository's account is a made-up portfolio written by
  `tools/sample_account.py` from the synthetic prices already in the fixtures:
  six stocks (one short), two options, dividends, an expired option and a
  partly filled order on the recording day, all reconciling (equity is cash
  plus positions; the day's P&L is the positions'). The drift job records the
  CI account's real answers with `capture_alpaca --verbatim` and runs the
  parsers on them; sample-copy recordings leave the account out.

## Phase 4: Paper trading (stocks) (built)

Built on 2026-10-05, in two parts: the order desk and guardrails without UI,
then the tickets, the blotter and the kill switch.

| Request | Endpoint | When |
|---|---|---|
| Place | `POST /v2/orders` (amounts as strings, a `client_order_id` always) | a ticket's *Confirm* |
| Look up | `GET /v2/orders:by_client_order_id` | after a lost answer, a `5xx` or a duplicate-id `422` |
| Replace | `PATCH /v2/orders/{id}` (a new order with its own client id) | ORD's *Confirm replace* |
| Cancel | `DELETE /v2/orders/{id}` | ORD's or a ticket's *Cancel* |
| Kill switch | `DELETE /v2/orders`, then optionally `DELETE /v2/positions?cancel_orders=true` | ORD's kill switch |
| Order list | `GET /v2/orders?status=all&limit=500` | a minute in any session, five when closed, and after every order event, reconnect or desk action |

- **A separate order desk** (`mt_alpaca::OrderDesk`), never the data hub: it
  sends through `FetchCtx::send` (the shared request budget and event log, no
  caches) on the hub's runtime, with no timers and no blind retries. A ticket
  makes its `client_order_id` when it opens and keeps it until the order is
  placed. After a ten-second timeout, a dropped connection or a `5xx`, the desk
  asks for the order by that id (three times over about ten seconds): found, it
  was placed; not found, the ticket may send it again under the same id, which
  Alpaca refuses to accept twice (and a duplicate-id `422` sends the desk to look
  it up instead). While an order's fate is unknown the ticket cannot send.
  Offline replays send nothing.
- **Order states** come from the `trade_updates` stream: it now keeps each order
  as its latest event left it, merged over the order list by last change, and
  the list is re-read after every event and reconnect, as the account is.
- **The ticket.** `BUY XLU US 10 LMT 82.50 DAY` (also `MKT`, `STP`, `STPLMT`,
  `@price`, `GTC`, `IOC`, `FOK`, `OPG`, `CLS`, `EXT`) opens a filled-in ticket with
  the latest prices, the cost or proceeds, buying power and the position
  afterwards, the day-trade count, and every guardrail's verdict. Warnings must
  be ticked off; blocks stop it. Only its **Confirm** places the order, and it
  then follows the order's fills. A limit with no price starts at the last
  trade. `SELL` is the same; PORT's right-click menu opens tickets to buy, sell
  or close a position. Tickets are not restored after a restart.
- **`ORD`** is the blotter: open, filled, cancelled (with expired, rejected and
  replaced) or all orders, filtered by symbol, with *Cancel* and *Replace…*
  (quantity and prices, through the same guardrails) on open ones.
- **A fixed rule:** commands that arrive from outside the window (`--run`,
  commands forwarded by a second launch, hotkeys) can only open a ticket, never
  place an order; a test runs them against fixtures posing as the live network
  and checks that nothing but GETs goes out. Alerts run no commands. No
  automated trading.
- **Guardrails** (`mt_core::guard`), settings under `[trading]` and in SET, each
  tested: per-order, daily (what filled today plus what is open) and
  per-position caps in dollars, a price collar (limit and stop within a
  percentage of the last trade), a fat-finger check (a warning above a share of
  equity, a block above a number of shares), and a **restricted list**. Fixed:
  no market orders outside the regular session (auction orders excepted),
  buying power for what the order opens, no order that turns a long short (or a
  short long) in one go, no selling shares held by open orders, shorts only
  where the account and asset allow, and the pattern-day-trader count below
  $25,000.
- **The kill switch** (ORD) cancels every open order, optionally closes every
  position at the market, and turns trading off (`[trading] enabled`, so it
  holds across restarts) until it is turned back on in ORD or SET. The band
  says when trading is off.
- **Audit log:** every order request body and answer (or the lack of one),
  appended to `orders-YYYY-MM.jsonl` in `%LOCALAPPDATA%\MISO Terminal\audit`.
  Keys travel only in headers and are never written.
- **Fixtures:** the sample account gains an order list (`v2/orders.json` from
  `tools/sample_account.py`): every fill's order, the day's order the stream
  replays, a cancelled order and an open GTC sell that holds 100 of the 200 XLU
  shares (`qty_available`).

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
