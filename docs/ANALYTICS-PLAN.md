# Analytics plan: history, studies, forecasts and AI

The work shipping as **`0.3.0`**: the terminal's own price history (built),
studies and BETA for securities, forecasts (FCST), and an assistant built on
Claude (ASK). Code signing goes ahead alongside it, and live trading (markets
Phase 6) is `0.4.0`, only once releases are signed
([release plan](RELEASES.md#versioning)).

## Decisions

- **Order.** The history first, then studies and BETA, forecasts, and the
  assistant: forecasts train on the history, and the assistant's best tools
  are the history, the studies and the forecasts. Each part is several pull
  requests, and a function is registered only in the one that makes it usable.
- **Information only.** Nothing here places, stages or cancels an order.
  Forecasts and answers never feed the order desk, the guardrails or the
  alerts, and the assistant has no tool that writes anything.
- **Local first.** History, studies and forecasts are computed on the user's
  machine from data the terminal already fetches. Only the assistant sends
  data to a third party (Anthropic), through the user's own Claude app or
  their API key.
- **Pure crates for the maths.** Studies go in `mt-core`, forecasting in a new
  `mt-forecast`, the Claude client and its tools in a new `mt-ai`. None
  depends on egui, so CI tests them on Linux.
- **Pure Rust where it matters.** No C++ build enters the terminal, which
  steers the choice of forecasting libraries.

## 1. The terminal's own price history (built)

Built on 2026-10-07 and 2026-10-08 (#27, #29, #30, #31). Every daily DA
ex-post and RT report the terminal downloads is kept for every node in a
compact [day store](ARCHITECTURE.md#the-day-store), about 170 KB a day per
market. SET's *Price history* chooses how far back to keep it (three months
by default, about 30 MB; everything since 2023-01-01, MISO's first day, about
470 MB) and [backfills](ARCHITECTURE.md#the-price-historys-backfill) the
missing days in the background, one report every two seconds (MISO sends each
uncompressed, about 1.3 MB). GP, SPRD, CMP and HUBS read only their nodes from
it, for any window; only days missing from the last 90 are downloaded.

It replaced the export from the Energy-Pricing-Journalist DuckDB, after a
comparison over every node, hour and component of 78 DA and 77 RT days from
2023-01-01 to 2026-09-04 (28.9 million values): identical, apart from two RT
days the DuckDB held as preliminary, where the store holds MISO's final
prices.

Five-minute prices are unchanged: MISO publishes them only for today and
yesterday, so that archive still covers only days the terminal ran.

## 2. Gaps in security prices (dropped)

The plan was to hold a security's last price through minutes without trades
and break lines between sessions, instead of drawing slopes across them.
Dropped on 2026-10-08, to go straight to the studies. Charts still join one
bar to the next (CMP breaks intraday lines at gaps of two hours).

## 3. Studies and BETA

**Studies on GP for a security.** A *Studies* menu over the price chart:

- **Overlays:** simple moving averages (20, 50 and 200 by default), an
  exponential moving average, and Bollinger bands (20 periods, two standard
  deviations).
- **Lower panes,** between the price and the volume: momentum and rate of
  change, RSI (14) and MACD (12, 26, 9).
- Each study's periods can be edited; the chosen set is the panel's (saved
  with the layout), and new panels start from `[markets.studies]`.
- Enough extra bars are fetched before the window to warm the studies up, so
  a 200-day average is defined from the first day shown.
- Daily studies count trading days. Intraday studies count the bars there
  are: a minute with no trade has no bar on Alpaca's feed, so in a thin name
  "20 periods" can span more than twenty minutes, and the chart says so.
- The maths lives in `mt_core::studies`, each study tested against values
  worked by hand.

**`BETA XLU US`**, a new function: how a security moves with a benchmark.

- Beta, alpha (annualised), R², correlation, beta's standard error and the
  number of observations, against **the S&P 500** (`SPY US`: Alpaca carries
  ETFs, not indexes) and against **its industry**, side by side.
- A scatter of the security's returns against the benchmark's with the fitted
  line, and a rolling beta.
- Windows of one, two and five years; daily or weekly returns (weekly by
  default from two years); raw and adjusted beta (two thirds raw plus one
  third of 1).
- **Total returns,** from bars adjusted for dividends as well as splits
  (Alpaca's `adjustment=all`; the bars query asks for `split` today), since
  utilities pay large dividends. Returns use the bars that exist, aligned on
  the days both series have.
- **Excess returns** over a risk-free rate if a clean public source of daily
  Treasury bill rates passes the usual checks; until then alpha is over zero,
  and the panel says so.
- **The industry benchmark:** one set in config (`[markets.benchmarks]`,
  `VST US = "XLU US"`), or an equal-weighted basket of a list
  (`BETA VST US GENERATORS`). The built-in lists come with benchmarks; a
  security with neither shows the S&P 500 alone, with a hint. No industry
  lookup service (SEC EDGAR was considered and left out).
- `BETA XLU US XLE US` names a benchmark directly. DES gains a beta tile that
  opens BETA.

## 4. Forecasts (`FCST`)

`FCST <node> [DA|RT]` and `FCST XLU US`: a fan chart (median, 50% and 80%
bands) after the recent history, an hourly table, the model used, and a
*Skill* tab with how each model has done on past data. GP gains a *Forecast*
overlay, and DAM shows tomorrow's forecast strip until MISO posts the results.

**Targets.** DA LMP for tomorrow's 24 hours; RT LMP, hourly, 24 to 48 hours
out; the DART spread; a security's daily close one to twenty trading days
out. Any node the price history holds, the hubs and load zones first.

**Statistical models,** always computed and always shown as the bar to beat:
seasonal naive (the same hour yesterday and last week) and an hour-by-weekday
profile; exponential smoothing and MSTL (daily and weekly seasonality) from
[augurs](https://docs.rs/augurs); for securities, a random walk with drift, a
volatility cone and GARCH(1,1).

**Machine learning:** gradient-boosted trees, with
[Perpetual](https://docs.rs/perpetual) (pure Rust, tuned by a single budget,
so no hyperparameter search on a desktop) first and a `linfa` ridge regression
as the fallback.

- **Features:** hour, weekday and holidays; recent DA and RT at the node and
  its congestion; Henry Hub gas (lagged); MISO's load forecast; and, once
  enough history builds up, the wind and solar forecasts, NWS temperatures by
  zone and scheduled outages.
- **Only what was known at the time.** A model trained on actual load where a
  forecast would have been known is right for the wrong reason. So MISO's
  load forecast (MTLF), the wind and solar forecasts, the NWS forecasts by
  zone and the outage schedule are kept as issued: each new version stored
  with the time it was fetched, unchanged versions not stored again, only the
  fields the models use, one gzipped file per day, two years kept (tens of
  megabytes; SET shows the space). A feature joins the model once enough of
  its history exists.
- **Intervals** from past errors (split conformal prediction over the
  backtest), so an 80% band holds about 80% of outcomes by construction.

**Evaluation.** Rolling-origin backtests over recent months (MAE, RMSE,
pinball loss, band coverage), each against seasonal naive. A model that does
not beat naive says so. Daily stock returns are close to unpredictable, so
FCST for a security shows a distribution rather than a direction, and machine
learning is offered there only if it beats the random walk.

**Running it.** Training runs on the user's machine on its own thread, when
new days arrive or on request (seconds for a few years of hourly data).
Models are cached under `local://models/` with their data range and version,
and a format change discards them.

## 5. The assistant (`ASK`)

`ASK` opens a panel for questions; `ASK why did RT spike at INDIANA.HUB
yesterday` opens it with the question asked. The answer streams in, the
lookups it makes show as they run, its figures come from those lookups, and
it links the panels that show them (`GP INDIANA.HUB 2 5MIN`, `BCH
2026-10-06`).

**Two ways in, one set of tools.** Anthropic allows a Claude subscription to
be used only in Claude Code and Anthropic's own apps; other products must use
an API key ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)).
So:

- **Through your subscription: the terminal as an MCP server.**
  `miso-terminal --mcp` serves the read-only tools over the Model Context
  Protocol (stdio) to Claude Desktop or Claude Code, within the plan's own
  limits at no extra cost. SET shows the configuration snippet to paste in.
  The server answers from the running terminal when one is open (over the
  single-instance channel, with its token), else from a headless hub over the
  same cache. This is the route the defaults point to.
- **Inside the terminal: ASK with an API key,** paid per use. The monthly
  spending cap defaults to **$0**, so ASK sends nothing until the user enters
  a key and raises it; until then the panel explains both routes.

**The Messages API client** (`mt-ai`, HTTP through `mt-data`; there is no
official Rust SDK):

- **The loop:** the question with the tool definitions; while Claude calls
  tools, run them (in parallel where independent) and send all results back
  in one message; stop at the answer or at caps on rounds (12) and tokens.
- **Streaming** over server-sent events, a new reader in `mt-data`.
- **Model:** Claude Opus 5.5 (`claude-opus-5-5`) by default, Sonnet 5.5 and
  Haiku 4.5 as cheaper choices, effort **low** by default. Model ids and
  prices are configuration, not constants.
- **API details:** strict tool definitions with inputs validated before
  running; `tool_choice` on `auto`; refusals shown as such, with the
  server-side fallback on; `max_tokens` and `pause_turn` handled; 429s and
  overloads retried with backoff.
- **Prompt caching:** the tool list (in a fixed order) and the system prompt
  never change between questions, so they are cached; anything that varies
  comes after them. LOG shows cached and uncached tokens.
- Other providers are not part of `0.3.0`; the client sits behind a small
  trait so one could be added.

**Tools,** all read-only and answered from the DataHub (its cache, request
budgets and LOG), each returning compact JSON with units and market-time
timestamps:

| Tool | What it answers |
|---|---|
| `find_nodes` | Nodes by name, type, state or place ("Indiana" finds `INDIANA.HUB`, the load zone and nearby nodes) |
| `lmp_series` | A node's DA and RT prices and components over a window, with statistics and extremes |
| `price_extremes` | Where and when prices crossed a threshold across many nodes, from the price history |
| `binding_constraints` | DA and RT binding constraints and shadow prices (`CONS`, `BCH`) |
| `reserves` | Ancillary MCPs by product and zone (`ASM`) |
| `system_conditions` | Load against forecast, fuel mix, wind and solar, interchange, ACE, transfers and headroom |
| `outages` | Planned and forced outages (`OUT`) |
| `weather` | NWS forecasts by MISO zone (`WX`) |
| `gas` | Henry Hub spot (`GAS`) |
| `news` | The local headline archive by words and dates (headlines and links only) |
| `forecast` | Part 4's forecast for a node or security, with bands and backtest skill |
| `securities` | Quotes, bars, studies and beta |
| `paper_account` | Positions and P&L, only if *Let ASK see the paper account* is on (off by default) |

The system prompt carries a short, cited reference on how MISO prices work
(LMP components, DA against RT, ex-ante against ex-post, interval-beginning
times, EST, when final RT reports land, offer caps, shortage and
constraint-violation pricing), dated and linked to MISO's tariff and business
practice manuals.

**The two example questions.** *"Why did RT hit $10,000/MWh for a five-minute
interval yesterday?"*: `price_extremes` finds the interval and place,
`lmp_series` splits it into energy, congestion and losses (a system shortage
or a binding constraint), and `binding_constraints`, `reserves` and
`system_conditions` show which. The operating feeds (load, fuel mix,
interchange, ACE, transfers, reserves, constraints) show only today, so they
get per-day stores like the five-minute prices, plus MISO's historical
reports where one exists. *"What will DA be tomorrow in Indiana?"*:
`find_nodes` maps Indiana to the hub and load zone; `lmp_series` once MISO
posts the results, `forecast` before that, with `system_conditions`,
`weather` and `gas` for the reasons. Questions with no source in the terminal
can use Anthropic's web search tool, off by default and cited when on.

**Keys, privacy and cost.** The key lives in Credential Manager
(`miso-terminal/anthropic/api-key`), sent only to `api.anthropic.com` over
TLS and never through a redirect. The first use says what leaves the machine:
the question, the tools' results (public market data) and, only with its own
switch, the paper account; never keys, the audit log or files. The
[privacy policy](PRIVACY.md) gains Anthropic in the same pull request, and
that first use links it (SignPath Foundation asks for both). Each answer
shows its tokens and cost, LOG lists each request, and the monthly cap is
enforced from the usage the API reports. Conversations stay in memory unless
saved. Through MCP the terminal sends nothing to Anthropic itself; the paper
account switch governs the MCP tool too.

**Safety.** The tools get a read-only handle on the hub, with no `AppCommand`
sender and no order desk, and a test checks that no tool can reach either.
Links in answers open only non-trading functions, and only when clicked.
Headlines and web results are passed as data; with no tool that writes, a
hostile headline can at worst spoil an answer, which its cited lookups make
visible.

**Testing it.** CI runs the loop against a fake server (streaming, tool
rounds, parallel calls, refusals, `max_tokens`, 429s, cancelling) and the
MCP server against a scripted client. An evaluation set of questions about
past events, with recorded tool results, grades answers on facts, supported
causes and saying "I can't tell"; it costs money, so it runs by hand before a
release, not in CI.

## Testing

- Studies, beta, the forecasting plumbing and the backtest metrics are pure
  functions in the non-UI crates, tested on Windows and Linux against values
  worked by hand.
- BETA, FCST and ASK get snapshot tests against fixtures; ASK's renders a
  recorded conversation.
- The drift job adds any rate source BETA uses.

## Risks

- **Disk.** Per-day stores of operating data and issued forecasts add to the
  price history; SET shows each and sets their limits.
- **Forecasts mistaken for advice.** Every forecast shows its bands and its
  record against naive, and the assistant is told to say how uncertain it is.
- **Assistant cost and errors.** MCP costs nothing extra; the API route has
  caps per question and per month ($0 until raised) and an evaluation set.
- **Anthropic's terms change.** The two routes are separate, so one can go
  without the other.
- **Library churn.** augurs and Perpetual are young; each sits behind
  `mt-forecast`'s own trait.
- **Scope.** If `0.3.0` runs long, finished parts can ship in `0.2.x`
  releases.

## Sources

- MISO market reports:
  [docs.misoenergy.org/marketreports](https://www.misoenergy.org/markets-and-operations/real-time--market-data/market-reports/)
- Alpaca stock bars (timeframes, feeds, `adjustment`):
  [docs.alpaca.markets](https://docs.alpaca.markets/reference/stockbars)
- [augurs](https://docs.rs/augurs) (ETS, MSTL) and
  [Perpetual](https://docs.rs/perpetual) (gradient boosting)
- Anthropic on subscription sign-in in other products:
  [code.claude.com](https://code.claude.com/docs/en/legal-and-compliance)
- Claude: [models](https://platform.claude.com/docs/en/about-claude/models/overview),
  [pricing](https://platform.claude.com/docs/en/about-claude/pricing),
  [tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview),
  [streaming](https://platform.claude.com/docs/en/build-with-claude/streaming),
  [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
