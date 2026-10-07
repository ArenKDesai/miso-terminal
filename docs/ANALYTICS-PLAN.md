# Analytics plan: history, studies, forecasts and AI

The work that comes before MISO Terminal applies for code signing: the
terminal keeps its own price history instead of borrowing one, securities get
the studies a trader expects (moving averages, Bollinger bands, momentum,
alpha and beta), MISO products and securities get forecasts, and an assistant
built on Claude answers questions about the market with the terminal's own data.
Gaps in security prices are held flat instead of drawn as slopes. A plan, not
yet built (2026-10-07).

It ships as **`0.3.0`**. The application to SignPath Foundation follows that
release, and live trading (markets Phase 6) moves to `0.4.0`, still only once
releases are signed ([release plan](RELEASES.md#versioning)).

## Decisions

- **Order.** History first, then gaps in security prices, then studies, then
  forecasts, then the assistant. Each part leans on the ones before it: the
  forecasts train on the archive, and the assistant's best tools are the
  archive, the studies and the forecasts. Each part is several pull requests,
  and as in the markets phases, a function is registered only in the pull
  request that makes it usable.
- **Information only, still.** Nothing in this plan places, stages or cancels
  an order. Forecasts and answers never feed the order desk, the guardrails or
  the alerts, and the assistant has no tool that writes anything.
- **Local first.** History, studies and forecasts are computed on the user's
  machine from data the terminal already fetches. The assistant is the one
  part that sends data to a third party (Anthropic), so it is off until the
  user turns it on, enters their own key and asks a question.
- **Pure crates for the maths.** Studies go in `mt-core`; forecasting is a new
  crate, `mt-forecast`; the Claude client and its tools are a new crate,
  `mt-ai`. None of them depends on egui, so CI tests them on Linux like the
  other source crates.
- **Pure Rust where it matters.** No C++ build enters the terminal (the reason
  the DuckDB lived in a script). Forecasting libraries are chosen with that in
  mind.

## 1. The terminal's own history

**Now.** `tools/export_history.py` copies hourly DA and RT prices for chosen
nodes out of the
[Energy-Pricing-Journalist](https://github.com/ArenKDesai/Energy-Pricing-Journalist)
DuckDB into the cache (`local://archive/lmp/<node>`). Without that export,
`series::node_history` stops at 90 days (`DOWNLOAD_DAYS`) of daily report
downloads. The DuckDB is itself built from MISO's daily reports
(`<date>_da_expost_lmp.csv`, `<date>_rt_lmp_final.csv` and `_rt_lmp_prelim`
until the final lands), the same files `DayReportQuery` and `RtBestDayQuery`
already download, and MISO publishes them back to 2023-01-01. So the
dependency buys nothing the terminal cannot fetch itself; it only saves the
wait.

**Plan.**

- **A day store for every node.** Each daily report the terminal parses is
  kept whole: every CP node's hourly LMP, MCC and MLC for that day and market,
  in compact columns under `local://archive/report/<da|rt>/<date>`, exempt
  from the cache's size cap like the five-minute archive. Every report
  downloaded for any panel lands there, so the archive grows with use. For
  scale: the Energy-Pricing-Journalist keeps the raw reports gzipped at about
  220 KB a day per market, about 600 MB for both markets since 2023; a binary
  store should be no larger.
- **Backfill on request.** A *Price history* section in SET: how far back to
  keep (from a month to MISO's first day, 2023-01-01), the disk it will take,
  and *Download now*. The backfill runs on the hub at a polite pace (a
  `Budget` on `docs.misoenergy.org`; at one report every two seconds the whole
  span takes about an hour and a half), shows its progress in LOG and the
  status bar, can be paused, resumes where it stopped, and replaces a
  preliminary RT day when its final report appears. Nothing downloads until
  the user starts it, as the principles ask.
- **Readers.** `node_history` reads any node from the day store, so the 90-day
  cap applies only to days not archived, and GP, SPRD, CMP and HUBS reach back
  as far as the archive does for every node, not only the exported ones. The
  forecasts (part 4) and the assistant's scans across nodes (part 5) need
  exactly that.
- **Retiring the export.** `LmpArchiveQuery`, `tools/export_history.py` and
  their docs (the tutorial's *Reach back years*, ARCHITECTURE's *Long history*,
  the README's roadmap item, `CLAUDE.md`) go. Files an earlier version
  exported are discarded at first launch, and the changelog says so; the
  backfill rebuilds the same history from MISO.
- **Checked once against the old source.** Before the export goes, a one-off
  comparison of the day store with the DuckDB for a sample of nodes and days,
  so the switch changes where the numbers come from, not what they are.

Unchanged: MISO publishes five-minute prices only for today and yesterday, so
the five-minute archive still covers only days the terminal ran (the README's
backfill item stays open).

## 2. Gaps in security prices: forward fill

**Now.** No code interpolates, but the charts draw it: Alpaca returns a bar
only for an interval with at least one trade, and on the free plan's IEX feed
(a few per cent of volume) a thinly traded stock has many empty minutes. GP
for a security joins each bar's close to the next with a straight line, so an
empty stretch reads as a steady drift, and in the five-day view the line runs
across nights. The sparklines in Q and WL place closes evenly, ignoring the
time between them. CMP already breaks intraday lines at gaps of two hours.

**Plan.** Forward fill wherever a chart shows a price through time:

- Within a session, a price holds at the last close until the next bar (a
  step, as node prices are drawn), because that was the price: nothing traded
  in between.
- Lines break at the session's end. Across a night, a weekend, a holiday or a
  halt there is no price to hold, and a flat line across 17 hours would
  misstate it as much as a slope does.
- Sparklines place points by time.
- A filled interval is marked as filled. Its volume is zero, and its VWAP and
  trade count stay empty.

**Where forward fill does not apply**, for reasons that hold up:

- **Return statistics.** Volatility, beta, alpha, correlation and the
  momentum studies use the bars that exist, aligned on the intervals both
  series have. A filled bar adds a return of exactly zero, which drags
  volatility and beta toward zero and weakens correlations: the classic
  stale-price bias. Daily bars, which those statistics use, are rarely
  missing anyway.
- **Freshness.** A filled price is never a fresh price. The tiles' times,
  the stale-price checks of the guardrails
  ([finding 4](SECURITY-REVIEW.md)) and anything else that asks "how old is
  this price" use the last actual trade.

One helper in `mt_core::equity` (bars, timeframe and the exchange calendar in;
bars with a `filled` flag out) and one chart helper beside
`chart::hourly_steps`, used by GP, CMP, Q, WL and the HOME tiles. The snapshot
tests change on purpose and are re-accepted.

## 3. Studies and BETA

**Studies on GP for a security.** A *Studies* menu over the price chart:

- **Overlays:** simple moving averages (20, 50 and 200 by default), an
  exponential moving average, and Bollinger bands (20 periods, two standard
  deviations, as Bollinger defines them).
- **Lower panes,** between the price and the volume: momentum (the change over
  N periods) and rate of change, with RSI (14) and MACD (12, 26, 9), which
  cost little once the averages exist.
- Each study's periods can be edited; the chosen set is the panel's (saved
  with the layout) and new panels start from `[markets.studies]` in
  `config.toml`.
- Enough extra bars are fetched before the window to warm the studies up, so
  a 200-day average is defined from the first day shown.
- Daily studies count trading days. Intraday studies run on the filled
  session grid (part 2), so "20 periods" on one-minute bars means twenty
  minutes, not twenty trades.
- The maths lives in `mt_core::studies`, each study tested against values
  worked by hand.

**`BETA XLU US`**, a new function (after Bloomberg's): how a security moves
with a benchmark.

- Beta, alpha (annualised), R², correlation, the standard error of beta and
  the number of observations, against **the S&P 500** and against **its
  industry**, side by side. Alpaca carries ETFs, not indexes, so the S&P 500
  is `SPY US`.
- A scatter of the security's returns against the benchmark's with the fitted
  line, and a rolling beta through time.
- Windows of one, two and five years; daily or weekly returns (weekly is less
  troubled by trades that do not line up in time, and is the default for two
  years and more); the raw beta and the adjusted one (two thirds raw plus one
  third of 1).
- **Total returns.** Utilities pay large dividends, so returns come from bars
  adjusted for dividends as well as splits (Alpaca's `adjustment=all`; the
  bars query asks for `split` today).
- **Excess returns** over a risk-free rate if a clean public source for daily
  Treasury bill rates passes the usual checks (terms, robots.txt); until then
  alpha is over zero, and the panel says so.
- **The industry benchmark:** one of three, shown with the choice made: a
  benchmark set in `config.toml` (`[markets.benchmarks]`, `VST US = "XLU US"`);
  an equal-weighted basket of a list (`BETA VST US GENERATORS`, built from the
  bars already fetched for the list's members); or an industry ETF chosen from
  the company's SIC code in SEC EDGAR, through a small table of SIC ranges to
  ETFs. The built-in lists come with their benchmarks set.
- `BETA XLU US XLE US` names a benchmark directly. DES gains a beta tile that
  opens BETA.

## 4. Forecasts (`FCST`)

`FCST <node> [DA|RT]` and `FCST XLU US`: a fan chart of the forecast (median,
with 50% and 80% bands) after the recent history, an hourly table, the model
used, and a *Skill* tab with how each model has done on past data. GP gains a
*Forecast* overlay, and DAM shows tomorrow's forecast strip until MISO posts
the results, then results against the forecast.

**Targets.** DA LMP for tomorrow's 24 hours; RT LMP, hourly, for the next 24
to 48 hours; the DART spread; daily closes of a security one to twenty
trading days out. Any node the archive holds (part 1), with the hubs and load
zones first.

**Statistical models,** always computed, and always shown as the bar to beat:

- Seasonal naive (the same hour yesterday, the same hour last week) and an
  hour-by-weekday profile over recent weeks.
- Exponential smoothing and MSTL (daily and weekly seasonality) from
  [augurs](https://docs.rs/augurs), Grafana's Rust time-series library, whose
  ETS follows statsforecast.
- For securities: a random walk with drift, with a volatility cone from
  realised volatility, and GARCH(1,1) volatility (small enough to write here).

**Machine learning:** gradient-boosted trees on features, with
[Perpetual](https://docs.rs/perpetual) (a pure-Rust booster tuned by a single
budget, which suits a desktop app that cannot run a hyperparameter search) as
the first candidate and a ridge regression from `linfa` as the fallback.

- **Features for prices:** hour, weekday and holidays; recent DA and RT at the
  node and its congestion component; Henry Hub gas (lagged); MISO's load
  forecast; and, once enough history of them builds up, the wind and solar
  forecasts, the NWS temperature forecast by zone and scheduled outages.
- **Only what was known at the time.** A model trained on the actual load
  where a forecast would have been known learns to be right for the wrong
  reason. So every forecast the terminal shows (MTLF, wind and solar, the
  NWS forecasts, the outage schedule) is kept as issued, in the same per-day
  stores as the five-minute prices, and a feature joins the model only once
  enough of its history has accumulated, or once a MISO report of past
  forecasts is found for it.
- **Intervals** from the spread of past errors (split conformal prediction
  over the backtest), so an 80% band holds about 80% of outcomes by
  construction rather than by assumption.

**Evaluation.** Rolling-origin backtests over recent months: MAE, RMSE,
pinball loss and the bands' coverage, each against seasonal naive. A model
that does not beat naive says so on the *Skill* tab, and the assistant's
forecast tool reports the same figures. For securities the expectation is
honest: daily returns are close to unpredictable, so FCST for a security
shows a distribution rather than a direction, and machine learning is offered
there only if its backtests beat the random walk.

**Running it.** Training happens on the user's machine, on its own thread so
it never holds up the hub's workers, when new days arrive or on request (a
gradient-boosted model over a few years of hourly data trains in seconds).
Models are cached under `local://models/` with the data range and the version
that built them, and a format change discards them cleanly.

## 5. The assistant (`ASK`)

`ASK` opens a panel for questions; `ASK why did RT spike at INDIANA.HUB
yesterday` typed on the command line opens it with the question asked. The
answer streams in, the lookups it made show as they run ("RT five-minute
prices at INDIANA.HUB, 2026-10-06"), its figures come from those lookups, and
it links the panels that show them (`GP INDIANA.HUB 2 5MIN`, `BCH
2026-10-06`) for the user to click.

**Claude, over the Messages API.** There is no official Anthropic SDK for
Rust, so `mt-ai` speaks HTTP through `mt-data` like every other source:

- **The loop:** send the question with the tool definitions; while Claude
  answers with tool calls, run them (in parallel where independent) and send
  all their results back in one message; stop at the answer, or at a cap on
  rounds (12 to start) and on tokens per question.
- **Streaming** over server-sent events, which `mt-data` does not read yet
  (its streams are WebSockets): a reader for a streamed POST response, with
  the answer's text appearing as it comes.
- **Model:** Claude Opus 5.5 (`claude-opus-5-5`) by default, with Claude
  Sonnet 5.5 and Claude Haiku 4.5 offered in SET as cheaper choices, and an
  effort setting (Opus 5.5 always thinks; effort sets how hard). Model ids
  and prices change, so they are configuration with defaults, not constants,
  and SET can list the models the key can use.
- **Details the API asks for:** tools defined strictly (`strict: true`) and
  their inputs validated before running; `tool_choice` left on `auto` (Opus
  5.5 does not accept forced tool use); a `refusal` stop reason shown as such,
  with the server-side fallback to another model turned on; `max_tokens` and
  `pause_turn` handled; 429 and overload answers retried with backoff.
- **Prompt caching:** the tool list (in a fixed order) and the system prompt
  (with its MISO reference text) never change between questions, so they are
  cached; the time, the question and anything else that varies come after
  them. LOG shows the cached and uncached tokens, so a broken cache shows.
- **Other providers** are not part of `0.3.0`. The client sits behind a small
  trait so one could be added later, a local model included.

**Tools,** all read-only, all answered from the DataHub, so they share its
cache, its request budgets (MISO's once-a-minute guidance included) and LOG.
Each returns compact JSON: summaries and the notable rows, not raw dumps,
with units and market-time timestamps labelled.

| Tool | What it answers |
|---|---|
| `find_nodes` | Nodes by name, type, state or place: "Indiana" finds `INDIANA.HUB`, the load zone and nearby CP nodes (from the map's positions) |
| `lmp_series` | A node's DA (ex-ante, ex-post), RT (five-minute, hourly, preliminary or final) prices and their components over a window, with statistics and extremes |
| `price_extremes` | Where and when prices crossed a threshold across many nodes, the scan a spike question starts with (needs part 1's day store) |
| `binding_constraints` | DA and RT binding constraints and shadow prices for a day or an interval (`CONS`, `BCH`) |
| `reserves` | Ancillary service MCPs by product and zone, where scarcity pricing shows (`ASM`) |
| `system_conditions` | Load against its forecast, fuel mix, wind and solar against forecast, interchange, ACE, regional transfer and headroom (`LOAD`, `FUEL`, `RENEW`, `NSI`, `ACE`, `RDT`, `CAP`) |
| `outages` | Planned and forced outages (`OUT`) |
| `weather` | NWS forecasts by MISO zone (`WX`) |
| `gas` | Henry Hub spot (`GAS`) |
| `news` | The local headline archive, by words and dates (headlines and links, never article text) |
| `forecast` | Part 4's forecast for a node or security, with its bands, inputs and backtest skill |
| `securities` | Quotes, bars, studies and beta (parts 2 and 3) |
| `paper_account` | Positions and P&L, only if *Let ASK see the paper account* is on (off by default) |

The system prompt carries a short, cited reference on how MISO prices work:
LMP as energy plus congestion plus losses, DA against RT, ex-ante against
ex-post, interval-beginning five-minute intervals, market time (EST, no
daylight saving), when final RT reports land, offer caps, and how shortage
and constraint-violation pricing can drive prices far above any offer. Its
figures are taken from MISO's tariff and business practice manuals when it
is written, dated and linked, and reviewed when those change, so the
assistant does not have to remember them.

**What the two example questions need.**

- *"Why did the RT energy market hit $10,000/MWh for a 5-minute interval
  yesterday?"* `price_extremes` finds the interval and where it happened;
  `lmp_series` splits the price into energy, congestion and losses, which
  says whether the whole system was short (the energy part) or a constraint
  bound (congestion); then `binding_constraints`, `reserves` and
  `system_conditions` at that interval show which. Two gaps to close in this
  part: yesterday's five-minute prices exist (MISO's previous-day feed), but
  the operating feeds (load, fuel mix, interchange, ACE, transfers, reserves,
  constraints) show only today. They get per-day stores like the five-minute
  prices, so yesterday stays answerable, together with MISO's historical
  reports wherever one exists for a dataset (the binding constraint reports
  BCH reads already do).
- *"What can I expect DA LMPs to be around tomorrow in Indiana?"*
  `find_nodes` maps Indiana to the hub and load zone; once MISO posts
  tomorrow's results, `lmp_series` has them; before that, `forecast` gives
  the hourly forecast with its bands and how well that model has done, and
  `system_conditions`, `weather` and `gas` give the reasons.

Questions the terminal has no source for (MISO's operational notifications,
for one, which have no clean public feed) can be answered with Anthropic's web
search tool, off by default and cited when on.

**Keys, privacy and cost.**

- The key lives in Credential Manager (`miso-terminal/anthropic/api-key`),
  entered in SET, sent only to `api.anthropic.com` over TLS, never through a
  redirect (the rules the Alpaca keys got from findings 2 and 5).
- Off by default. The first use says what leaves the machine: the question,
  the tools' results (public market data) and, only with its own switch, the
  paper account. Never keys, never the audit log, never files.
- Each answer shows its tokens and cost, LOG lists each request, and SET holds
  a monthly spending cap the panel enforces from the usage the API reports.
- Conversations stay in memory unless the user saves one.

**Safety.**

- **No writes, by construction.** The tools get a read-only handle on the
  hub, with no `AppCommand` sender and no order desk. A test checks that no
  tool can reach either, as `commands_from_outside_the_window_only_open_tickets`
  does for commands. Links in answers open only non-trading functions (never
  BUY, SELL, MLEG or ORD) and only when clicked; web links go through
  `AppCommand::OpenHeadline`'s http(s) check.
- **Prompt injection.** Headlines and web results are other people's text. They
  are passed as data, and with no tool that writes, the worst a hostile
  headline can do is spoil an answer, which the answer's cited lookups make
  visible.

**Testing it.** CI runs the loop against a fake server, as the order desk's
tests use a fake broker: streaming, tool rounds, parallel calls, refusals,
`max_tokens`, 429s and cancelling. An evaluation set of questions about past
events, with the tools' results recorded so it can be replayed, grades
answers on facts that match the data, causes the data supports, and saying
"I can't tell" when it cannot. It spends real money, so it runs by hand
before a release and after prompt or tool changes, not in CI.

## Testing

- The studies, forward fill, beta, the forecasting models' plumbing and the
  backtest metrics are pure functions in the non-UI crates, tested on Windows
  and Linux against values worked by hand.
- The day store gets a fixture day per market and a test that a recorded
  report round-trips; the backfill is tested against a fake server for
  pacing, resuming and the preliminary-to-final swap.
- New panels (BETA, FCST, ASK) get snapshot tests against fixtures; ASK's
  snapshot renders a recorded conversation, not a live one.
- The drift job keeps checking the MISO report formats the day store parses,
  and adds the EDGAR lookup and any rate source BETA uses.

## Risks

- **Disk.** The full history is a few hundred MB, and per-day stores of
  operating data add more. Both are shown in SET before anything downloads,
  and both have limits there.
- **Forecasts mistaken for advice.** Every forecast shows its bands and its
  record against naive, and the assistant is told to say how uncertain it is.
- **Assistant cost and errors.** Caps per question and per month, costs shown
  per answer, and an evaluation set that runs before releases.
- **Library churn.** augurs and Perpetual are young; each sits behind
  `mt-forecast`'s own trait, so either can be replaced without touching the
  panels.
- **Scope.** This is a large release. If it runs long, parts 1 and 2 can ship
  as a `0.2.x` release on their own; the signing application waits for
  `0.3.0` either way.

## Open questions

- **How far back the backfill offers by default:** one year (about 160 MB) or
  everything since 2023 (about 600 MB).
- **EDGAR's contact line.** The SEC asks every client for a User-Agent with a
  contact address. Either the project's address, or the user's, entered in
  SET; or skip EDGAR and rely on configured benchmarks and list baskets.
- **Which forecasts to keep as issued** beyond MTLF and wind and solar, and how
  long to keep them.
- **Default effort and spending cap** for the assistant, once the evaluation
  set shows what each costs per answer.

## Sources

- MISO market reports (daily LMP, binding constraint and other reports):
  [docs.misoenergy.org/marketreports](https://www.misoenergy.org/markets-and-operations/real-time--market-data/market-reports/)
- Alpaca stock bars (timeframes, feeds, `adjustment`):
  [docs.alpaca.markets](https://docs.alpaca.markets/reference/stockbars)
- [augurs](https://docs.rs/augurs) (ETS, MSTL) and
  [Perpetual](https://docs.rs/perpetual) (gradient boosting), both on docs.rs
- SEC EDGAR APIs (submissions with SIC codes, the ticker to CIK map):
  [sec.gov](https://www.sec.gov/search-filings/edgar-application-programming-interfaces)
- Claude: [models](https://platform.claude.com/docs/en/about-claude/models/overview),
  [pricing](https://platform.claude.com/docs/en/about-claude/pricing),
  [tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview),
  [streaming](https://platform.claude.com/docs/en/build-with-claude/streaming),
  [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
