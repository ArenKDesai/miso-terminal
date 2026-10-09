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
            │ DataHub, Query │     │ mt-eia, mt-news│     │ TOML themes    │
            │ Stream, Request│     │ mt-alpaca      │     │ validation     │
            │ FetchCtx,      │     │ endpoints      │     │ registry       │
            │ budgets,secrets│     │ parsers        │     └────────────────┘
            │ Transport,     │     │ queries        │
            │ DiskCache      │     └───────┬────────┘
            └────────────────┘             │
            ┌──────────────────────────────▼───────────────────────────────┐
 domain     │ mt-core   prices · grid · weather · time · geometry · numbers│
            │           instruments · quotes, bars · exchange time · money │
            └──────────────────────────────────────────────────────────────┘
```

Dependencies point downward only. `mt-core`, `mt-data`, the sources (`mt-miso`,
`mt-nws`, `mt-eia`, `mt-news`, `mt-alpaca`), `mt-forecast` (forecasting
maths on `mt-core`: models, backtests and bands, with no I/O) and `mt-theme`
know nothing about egui, and CI tests them on Linux to keep them portable. A second front end (a
TUI, a web view, a CLI exporter) could reuse them unchanged.

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
   rule even when a user hammers F5); **conditional GETs** (a URL whose server
   sent an `ETag` or `Last-Modified` is asked "changed since?", so an unchanged
   feed costs a `304`); **request budgets** per host (below); an **on-disk gzip
   cache** for immutable files (the LMP reports go to the
   [day store](#the-day-store) instead); and an event log.
4. `FetchCtx` sends a `Request` (method, URL, headers, body) through a
   `Transport`: `HttpTransport` (reqwest with native-tls, so SChannel and the
   Windows certificate store) or `FixtureTransport` (recorded files, used by
   `--offline` and the tests).
5. The response is parsed by a pure function (`mt-miso::parse` for MISO; each
   source crate has its own) into `mt-core` types. On success the hub stores it and calls the notify hook, which the app
   wires to `request_repaint()`. On failure the last good value is kept, the
   error is attached, and retries back off exponentially (5 s up to 5 min).
6. Nothing refreshes unless something is watching it. Unwatched entries are
   garbage-collected after 15 minutes.

Panels therefore never block, never own threads, and never handle HTTP. Loading
and error states look the same everywhere because they all go through
`widgets::with_data`.

### Requests, budgets and secrets

Most sources need only `ctx.get(url)`. Sources with API keys build a
`Request` and put the keys in with `Request::secret_header`; the keys come from
`FetchCtx::secret(name)`, which reads the `SecretStore`: Windows Credential
Manager in the app (one generic credential per key, `miso-terminal/<name>`,
local to the machine), memory in tests and offline replay. A missing key is a
`FetchError::Auth` naming it. Keys never go in `config.toml`, saved state or
fixtures, and the event log shows every request through `Request::describe`,
which prints secret header values (and any header named like a key, token or
`Authorization`) as `***`. SET writes and removes keys directly in the store.

A `Budget` caps requests to a group of hosts per window (Alpaca allows 200 a
minute per key across its APIs). Every request through `FetchCtx` takes a slot
first, waiting for one when the window is full rather than failing; a `429`
closes the budget for the server's `Retry-After`. Budgets are passed in
`FetchCtxOptions` by whoever builds the context, and LOG shows their use.

`FetchCtx::send` is the raw path: any method, every HTTP status returned as a
`Response` (an API's rejection message is in the body), with budgets, the
concurrency cap and the log, but no caching and no retries. The order desk
sends with it. `get` and `get_immutable` turn statuses into errors:
`404` is `NotFound`, `401`/`403` `Auth`, `429` `RateLimited`.

### Streams

A `Stream` is the WebSocket counterpart of a `Query`: one implementation per
endpoint, with a key, the handshake `Request`, `hello` messages (a login, never
logged), subscribe and unsubscribe messages, and `apply`, which folds each
frame into a typed state. Panels call `cx.hub.watch_stream(&stream, &topics)`
every frame and get a `Snapshot` of that state, like `watch`.

The hub owns the connections: **one per endpoint**, shared by every panel and
subscribed to the union of their topics (the free Alpaca plan allows one
connection per feed). A topic nobody has asked for in 30 seconds is
unsubscribed, and a stream nobody has watched for a minute is closed, so tab
switches do not churn subscriptions. After a drop the hub reconnects with
backoff (1 s doubling to a minute), logs in again and resubscribes everything,
keeping the last state (shown as stale meanwhile). Pings every 30 seconds
detect a dead connection. A refused login (`FetchError::Auth`) stops the
reconnecting until `restart_streams()`, which SET calls when keys change and
LOG offers as a button. Repaint requests are throttled to ten a second however
busy the stream. While fetching is paused, streams close.

`HttpTransport` connects with tokio-tungstenite on native TLS (the same
SChannel trust as HTTP). `FixtureTransport` replays `<url path>.jsonl`, one
server frame per line, and then stays open and quiet, so offline mode and the
tests can drive a stream.

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

### The day store

Every DA ex-post and RT daily report the terminal parses is kept whole under
`local://archive/report/<da|rt>/<date>`: every node's hourly LMP, MCC and MLC
as whole cents in byte planes (`DayLmpReport::to_bytes`, about 170 KB a day
gzipped). `fetch_report` answers from it before downloading, so a settled day
is downloaded once; an RT day keeps its preliminary report until the final
one replaces it, never the other way round. Only live files naming the day
asked for are kept (a replay keeps nothing). The CSVs are not cached as well,
in memory or on disk (`FetchCtx::get_uncached`). The store is exempt from the
size cap, like everything in `mt_ui::kept_cache_dirs`.

Charts read the store a node at a time: a whole day of every node is about
1 MB in memory, so a year of days would be most of a gigabyte.
`StoredPricesQuery` (`Miso::stored_prices(nodes, first)`) reads only the
asked-for nodes' rows from each file (`DayLmpReport::rows_from_bytes`), all of
a panel's nodes at once (HUBS reads its eight hubs in one pass), and refreshes
every minute, re-reading only files whose size or modified time changed
(`DiskCache::read` does not touch the time). `series::nodes_history` takes
each day from the store, else from a download, which only the last 90 days get
(`DOWNLOAD_DAYS`); older days the store lacks are counted, and the chart
points to SET.

### The price history's backfill

`[price_history]` (`mt_miso::HistoryConfig`) sets the store's window
(`keep_days`: three months by default, 0 for everything since 2023-01-01) and
whether to `backfill` it. `mt_miso::Backfill` is a worker on the data
runtime, like the order desk: the app hands it the settings (`configure`) and
reads its `status` for SET, LOG and the status bar.

The store is its own record of progress. Each pass lists it (`Inventory`,
peeking at each RT file's header for final or preliminary), removes days
before the window, plans what is missing (`plan`: newest first, plus
preliminary RT days old enough for their final) and fetches each report
through `fetch_report`, the panels' own path, one at a time and two seconds
apart. A pause, a restart or a day a panel downloaded meanwhile therefore
costs nothing extra. A 404 is asked for again next pass, a report that will
not parse is skipped and shown in LOG, and anything else is retried after 10 s,
doubling to 10 minutes. A full window is looked at again hourly; the first
pass after launch waits 15 s for the panels' own fetches; a settings change
starts a new pass. The pacing is the backfill's own rather than a `Budget`,
which every panel request to the host would share.

The store replaced an export from the Energy-Pricing-Journalist DuckDB, after
a comparison over every value of 155 days (they agree wherever both hold
settled prices); `discard_exported_history` removes what the export left in
the cache.

### News

`mt-news` reads publishers' RSS and Atom feeds: one `FeedQuery` per feed, so
the hub, LOG, retries and conditional GETs treat each like any other source.
A feed's fetch builds on its previous value: new headlines merge in by
identity (the publisher's id scoped to its site, or the link without tracking
parameters), keep when they were first seen, drop out after `news.keep_days`,
and are written to `local://news/<feed id>`. At launch the shell seeds each
feed from that archive with `seed_stale`, as it does today's prices, so
headlines and search are there before the first fetch. A feed's own RSS `ttl`
can lengthen its 5-minute refresh, and a `Budget` spaces FT requests a second
apart for its robots.txt crawl delay.

`mt_ui::news::Combined` watches a set of feeds and merges them into one list,
rebuilt only when a feed's generation changes; TOP, NEWS, NI, HOME and the
alert engine each hold one. `mt_ui::news::Browser` is the list the three news
functions share (filters cached per list, keyboard focus on the list itself).
Opening an article is an `AppCommand::OpenHeadline`: the shell accepts only
`http`/`https` links, hands them to the system browser through egui, and
records the read mark (`local://news-read/ids`). The feed and topic lists are
built into `mt-news`, so a release can fix a feed that moved; `[news]` in
config.toml turns feeds off and adds or replaces feeds and topics by id.

Headline alerts are edge-triggered like the rest, with one addition: a rule
can return a token (the newest matching headline's id), and a new token fires
the rule again while it holds.

### Stocks and ETFs

`mt-alpaca` is a source like the others: queries for snapshots, bars, the
asset list, the clock and calendar and company news, and two `Stream`s (live
prices, live news), all through `FetchCtx` with the user's keys from the
secret store (`Request::secret_header`; a replay sends none). Its facade,
`Alpaca`, knows the configured feed and whether keys are stored
(`is_ready`), so panels show a prompt instead of failing requests without
them. One `Budget` covers every Alpaca host (180 of the 200 requests a minute
the key allows).

The free plan allows 30 trade and quote subscriptions in all (a symbol's
trades count one, its quotes another; more is refused with a `405`, as the
drift job found) and minute bars for any number. So `MarketStream` keeps
track of what it has sent: every symbol's trades first, then quotes while room
remains (a new symbol's trades displace a quote), bars for all, and the rest
held back until room frees up. Should the server refuse anyway, it halves its
limit and reconnects. The hub's subscription union stays unaware of the limit.
Panels watch a minute-old snapshot and the stream together and merge them per
security with `mt_alpaca::board::row` (`mt_ui::market::Board`).

Bars are fetched for many symbols at once and every page is followed (a
repeated page token, which a replay produces, ends it). Daily bars come from
the consolidated tape with an end 16 minutes back, which the free plan allows;
intraday bars follow the live feed. `FixtureTransport` serves
`bars@1Day.json` for `bars?timeframe=1Day&…`, so one endpoint can have a
recording per timeframe.

### The account

The same keys open the paper account, read-only (`mt_alpaca::account`):
queries for the account, positions, the equity curve (`history@1D.json` and
so on, by period), activities (newest page merged into what is kept) and
option snapshots for the greeks of held contracts, all parsed into
`mt_core::account` types with exact decimals. `TradeStream` is the account's
order-event stream; it speaks a different protocol from the market-data
streams (single objects tagged `stream`, a `listen` message, binary frames)
and counts logins and events in a sync token. The app watches that token
(`mt_ui::portfolio::resync`, with `peek_stream`, so it never opens the
stream itself) and refreshes the account's queries when it moves.

`mt_ui::portfolio::Book` is what the account panels share: the account,
positions, the order-event stream and a `market::Board` for the stock
positions. A position is marked at a newer price only when the board's last
trade is later than the positions' fetch, and then by the price change times
the quantity (`Position::mark`), so Alpaca's reference for the day is kept and
the next re-read replaces the estimate. `AccountMode` (paper, later live)
names the band above the status bar, the queries' keys and the stream's.

### Options data

`mt_alpaca::options` reads an underlying's contract list
(`/v2/options/contracts`, every page, out three years; refreshed hourly) into
an `mt_core::options::ContractList`, and one expiry's chain
(`/v1beta1/options/snapshots/{underlying}?expiration_date=…`, the indicative
feed) into an `OptionChain` of `OptionSnapshot`s: quote, latest trade, daily
bars, greeks and implied volatility. Alpaca streams options only in
MessagePack, so chains are re-read every minute instead. `chain_rows` pairs
calls and puts by strike from the contract list and the chain together;
adjusted contracts (another root, after a corporate action) are left out.
`FixtureTransport` picks `contracts@XLU.json` and `XLU@2026-12-18.json` by the
first query value. The repository's chains are made up by
`tools/sample_options.py` (Black-Scholes around the synthetic stock prices),
and only `capture_alpaca --verbatim` records a real one, for the drift job.

### The order desk

Orders go through `mt_alpaca::OrderDesk`, never the data hub: nothing is
cached, nothing refreshes on a timer and nothing is retried blindly. It sends
with `FetchCtx::send` (the shared request budget and event log, no caches) on
the hub's runtime, and reports each order's `Outcome` under the
`client_order_id` the ticket made before sending (`new_client_order_id`).
When an answer does not come (a ten-second timeout, a dropped connection, a
`5xx`), the desk asks Alpaca for the order by that id
(`/v2/orders:by_client_order_id`) a few times over several seconds: found, it
was placed; not found, it was not (`NotPlaced`), and the ticket may send it
again with the same id, which Alpaca refuses to accept twice (a `422` that the
desk answers with another lookup). While the outcome is unknown the id cannot
be sent. Cancels, replaces (a new order with its own client id) and the kill
switch (cancel every open order, then optionally `DELETE /v2/positions`) go
through the same path. Every request body and answer is appended to
`orders-YYYY-MM.jsonl` (`AuditLog`); keys travel only in headers, which are
never written.

A spread is one order (`OrderRequest::legs`, sent as Alpaca's `mleg` class);
its legs come back nested under it (`Order::legs`, read with `nested=true`).

Before the desk is asked, a ticket runs `mt_core::guard::review`
(`review_option` for a contract, `review_spread` for a spread, which also
returns the net prices, Alpaca's margin and the payoff): pure rules over the
request, the account, the position, the latest prices, the session, today's
order value and day trades, each a pass, a warning to acknowledge, or a
block. `Limits` (`[trading]`) holds the caps, the collar, the restricted list
and the switch.

The desk does not take the tickets' word for it. `submit` and `replace` take a
`guard::Approved`, which only `Review::approve` makes and which carries the
very request reviewed; a replacement must match its approval. The desk keeps
its own copy of the switch (`set_enabled`, cleared by `kill` before its
cancels go out) and sends nothing new while it is off. `Approved::unreviewed`
exists only with `mt-core`'s `unreviewed-orders` feature, for the desk's
tests and live checks; a release build with it does not compile. The order
list is re-read after every order event and reconnect, and the trade stream's
latest state of each order is merged over it (`order::merge_orders`).

In the UI, `mt_ui::trading` is what the tickets (`BUY`, `SELL`, `MLEG`) and
`ORD` share. A ticket's *Confirm* is the only call to `OrderDesk::submit`; the
app holds the one desk (`PanelCx::desk`) and re-reads the account whenever
the desk's generation moves. The kill switch also sets `[trading] enabled =
false`, so trading stays off across restarts. Tickets are not restored at
launch.

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
(five-minute today, the price history or daily reports before, the
five-minute archive on demand), and computes spreads, stats, percentiles and
the on-peak block.
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

US exchanges keep New York time, which observes daylight saving, so the 09:30
open is 08:30 MISO time from March to November. `mt_core::exchange` handles
equities (`chrono-tz`'s America/New_York): exchange clocks, the spring and
autumn clock changes, and the trading sessions (pre-market, regular, after
hours) for a day's hours, which the exchange calendar supplies. It follows the
same freezable clock. The two never mix: MISO data uses `time`, securities use
`exchange`, and charts of securities use `widgets::chart::exchange_plot`,
whose grid follows New York midnights through the clock changes.

## Instruments and money

MISO nodes and securities share the command line, and some node names (`AECI`,
`TVA`) look like tickers. Securities therefore always carry a market code,
Bloomberg style: `XLU US` (Bloomberg's trailing `Equity` is accepted); options
are OCC symbols (`XLU261218C00082500`). `mt_core::instrument` parses and prints
them. The command line joins a security's tokens into one argument (`XLU US GP
30` is `GP` with `XLU US` and `30`) and refuses securities for functions whose
`FunctionSpec::takes_security` is false, and options for those whose
`takes_option` is false. An option typed alone opens its chain in OMON.

Amounts that feed an order or a position (prices, fractional quantities,
notionals, cash, P&L) are exact `rust_decimal::Decimal`s, never `f64`;
`mt_core::money` parses, formats and rounds them to a tick. Brokers send them
as strings to keep them exact, and `Decimal` deserialises from those directly.

## Testing strategy

| Layer | Tests |
|---|---|
| `mt-core` | Market and New York time, intraday stores, the day store's format, map geometry, instruments, exact money, account figures, orders, and every guardrail for stocks, options and spreads, against values worked by hand |
| `mt-data` | The hub (dedupe, refresh, backoff, pause, GC), streams against a scripted server and a real local WebSocket, conditional GETs, budgets and `429`s, secrets kept out of the log, the disk cache |
| `mt-miso` | Every parser against a recording in `fixtures/` (structure and sanity, so re-recording keeps them green); the day store, the stored-prices reader and the backfill against a fake MISO |
| `mt-nws`, `mt-eia`, `mt-news` | Parsers against recordings (news as sample text in the feeds' real structure); merging, topics and read marks |
| `mt-alpaca` | Every dataset against recordings with synthetic prices and the made-up account; stream subscriptions within the plan's limit; the order desk against a fake broker (an order goes in once, lost answers looked up, rejections, cancels, replaces, the kill switch, nothing sent while trading is off) |
| `mt-forecast` | Holidays, gap filling and quantiles; each model against series worked by hand (naive days, profiles, smoothing a known shape, a random walk's bands, GARCH recovering simulated parameters); backtests that see only what was known, accuracy and conformal bands scored out of sample |
| `mt-theme` | Built-ins parse, validate and round-trip; contrast maths |
| `mt-ui` | Commands and completion; every earlier release's `config.toml` loading; the alert engine; series maths; a **headless smoke test** rendering and tessellating every function in every theme, with and without data; the shell's commands and shortcuts; commands from outside the window opening tickets with nothing but GETs sent |
| visual | `mt-ui/tests/snapshots.rs`: the real app rendered offscreen (egui_kittest + wgpu) against the fixtures with the clock frozen, compared with committed images |

The smoke test iterates the registry, so a new function gets coverage without
writing a test. It also checks that every feed any panel requests loads from
the fixtures without error, which is what makes `--offline` trustworthy.
Separately, the weekly `drift.yml` workflow records live MISO
responses (and the weather, gas, news and Alpaca sources, the last with a
throwaway paper account's keys from the repository secrets) and runs the parser
tests against them (`MT_FIXTURES`), so format changes surface in CI rather than
as a blank panel. Run by hand, it can also record a fresh set of Alpaca fixtures
(synthetic prices, sample text) as an artifact.

At runtime, each tab's `ui()` runs inside `catch_unwind`. A panicking panel is
logged, its state is dropped, and the tab shows the error with a *Reload panel*
button. The rest of the terminal keeps running.

## Decisions

- **egui + egui_dock + egui_plot.** Immediate mode keeps a panel to one file
  with no retained widget tree to synchronise. egui_dock provides tabs, splits
  and drag-to-dock. egui_plot provides zoomable, hoverable time series. Rendering
  is wgpu (DX12 on Windows, with a WARP software fallback for VMs and RDP).
- **native-tls**, not rustls. On Windows it is SChannel, which trusts the system
  certificate store, so TLS-inspecting corporate proxies work without setup.
  WebSockets (tokio-tungstenite) use it too. They do not go through an HTTP
  proxy, so a network that only allows proxied traffic will block streams.
- **Secrets in the OS store.** keyring-core with its Windows Credential Manager
  store (the `keyring` crate itself recommends that applications link these
  directly). Other platforms get an in-memory store for the session.
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
