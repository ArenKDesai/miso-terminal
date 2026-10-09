# Markets plan: news, market data, trading and portfolio

Headlines from the Financial Times, Bloomberg and the Washington Post; US
stock, ETF and option data from Alpaca; paper trading through Alpaca, with
live trading later; and account and portfolio tracking. Phases 0 to 5 were
built on 2026-10-04 and 2026-10-05 and shipped in v0.2.0. Phase 6, live
trading, is still to come (`0.4.0`). [ARCHITECTURE](ARCHITECTURE.md) explains
how the built pieces work.

## Decisions

| | Decision | Why |
|---|---|---|
| News sources | Public RSS headline feeds from FT, Bloomberg and the Washington Post, plus Alpaca's ticker-tagged news (Benzinga) | None of the three offers an individual-subscriber API |
| Subscriber articles | Open in the default browser, where the reader is signed in | The terminal never handles news passwords, and stays within each publisher's terms |
| Alpaca client | Our own HTTP and WebSocket client, `mt-alpaca`, on `mt-data` | `alpaca-py` would need a Python sidecar; `apca` brings its own HTTP client, bypassing `FetchCtx` (fixtures, replay, the event log, pacing). Alpaca's REST API is JSON with two auth headers |
| Data plan | Alpaca's free plan, the feed a setting | IEX real time or every exchange 15 minutes late, 200 requests a minute |
| Live orders | Every order confirmed, with caps and a kill switch | Paper uses the same flow, so habits carry over |
| Instrument syntax | Ticker plus market code, Bloomberg-style: `XLU US` | Never confused with a MISO node such as `AECI` |
| CI checks | A throwaway paper account, its keys in the repository secrets `ALPACA_PAPER_KEY_ID` and `ALPACA_PAPER_SECRET_KEY` | Alpaca needs keys even for market data; the account holds only paper money |

## What was built

| Phase | What it added |
|---|---|
| 0, foundations | Requests with any method and redacted secret headers, per-host request budgets, conditional GETs and hub-owned WebSocket streams in `mt-data` ([details](ARCHITECTURE.md#requests-budgets-and-secrets), [recipe](EXTENDING.md#add-a-streaming-source)); keys in Credential Manager; `XLU US` and OCC symbols on the command line; New York exchange time; exact decimals for money |
| 1, news | `mt-news`: 13 feeds, kept three weeks for search; `TOP`, `NEWS`, `NI`; headline alerts |
| 2, market data | Snapshots, bars, the asset list, clock, calendar, company news and live streams; `Q`, `GP` and `DES` for securities, `CN`, securities in WL and CMP |
| 3, account | The paper account, read only: `PORT`, `ACCT`, `PNL`, `ACT` and the PAPER band |
| 4, paper trading | The order desk, the guardrails, `BUY` and `SELL` tickets, `ORD`, the kill switch and the audit log |
| 5, options | `OMON` chains, single-contract tickets, `MLEG` spreads and their guardrails |

Facts about the sources that shaped the design (checked live, 2026-10-04
and 2026-10-05):

- **Alpaca's free plan** allows 30 trade and quote subscriptions on the stream
  in all, counting a symbol's trades and quotes separately (`405` beyond), and
  one stream connection per account (`406` for a second). The terminal
  subscribes trades first and quotes while room remains.
- Daily bars from every exchange need an end at least 15 minutes back on the
  free plan; option prices come from Alpaca's indicative feed (quotes derived
  from OPRA's, trades 15 minutes late), and options stream only in
  MessagePack, so chains are re-read each minute. The contract list stops at
  the next weekend unless given an end date.
- **Alpaca's order rules:** a `client_order_id` is accepted once, which is
  what lets the desk resend safely after a lost answer; no uncovered options;
  spreads need options level 3 and every sold leg covered in the same order
  and expiry; no orders for contracts expiring that day after 15:15 New York
  time (15:30 for SPY and QQQ).
- **News feeds:** FT's robots.txt asks for one request a second; Bloomberg's
  feeds answer `304` when unchanged; feeds do disappear (the Washington Post's
  homepage feed stopped in July 2026), so the feed list is configurable and
  the weekly drift job checks every one.

## Phase 6: Live trading

Only after paper trading has run cleanly for a few weeks.

- Live keys are stored separately; live is off by default, and turning it on
  takes a typed confirmation phrase in SET and a restart.
- One account trades per session, and the status band turns red.
- Every guardrail stays on, and every order needs its confirm.
- A security review of the order path before this ships: the first pass,
  with what is still open, is in [SECURITY-REVIEW.md](SECURITY-REVIEW.md).

## Testing

- Fixtures for every endpoint: a made-up account and option chains
  (`tools/sample_account.py`, `tools/sample_options.py`), synthetic prices and
  sample stories, never a verbatim recording.
- A fake broker in the order desk's tests: duplicates, lost answers,
  rejections, cancels, replaces and the kill switch.
- The weekly drift job runs every parser against live answers from the CI
  paper account. A job run by hand (`paper_orders`) places, replaces and
  cancels one order that cannot fill, on paper only. Installed copies of the
  terminal never see these keys: each user adds their own.

## Risks

- **Feeds change or vanish:** feed health in LOG, the drift job, an editable
  feed list.
- **Free-plan limits:** IEX's thin volume, indicative option prices and 200
  requests a minute; met with labels, a shared request budget and streams.
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
