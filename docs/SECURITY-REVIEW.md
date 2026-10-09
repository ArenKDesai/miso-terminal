# Security review of the order path

The [markets plan](MARKETS-PLAN.md#phase-6-live-trading) asks for a review
of the order path before live trading (Phase 6) ships. This is the first
pass, made on 2026-10-06 against `main` at the end of markets phase 5, before
the first release. A second pass reviews the Phase 6 changes themselves
before `0.4.0`, the release that carries them.

## What was read

- The order desk: `mt-alpaca/src/desk.rs` (send, look up, cancel, replace,
  the kill switch), `orders.rs` (order bodies and answers), `audit.rs` and
  `trades.rs` (the order stream).
- The guardrails: `mt-core/src/guard.rs`, `guard/options.rs`,
  `guard/spread.rs`, and the order types in `mt-core/src/order.rs`.
- The tickets and the blotter: `mt-ui/src/trading.rs` and the functions
  `ticket.rs`, `mleg.rs` and `ord.rs`, from what they feed the guardrails to
  the *Confirm* button.
- Keys and the network: `mt-data/src/secret.rs`, `request.rs` (redaction),
  `transport.rs` (the HTTP and WebSocket clients) and `authed` in
  `mt-alpaca/src/lib.rs`.
- Commands from outside the window: `miso-terminal/src/instance.rs`.

## What holds

- **Only the desk sends orders**, and only when a ticket's *Confirm*, ORD's
  *Cancel* or *Confirm replace*, or the kill switch is clicked. Typed,
  forwarded and `--run` commands open tickets and nothing else
  (`commands_from_outside_the_window_only_open_tickets`).
- **An order cannot go in twice.** A ticket keeps one `client_order_id`
  until its order is placed. After a lost answer the desk asks Alpaca for
  the order by that id before the ticket can be sent again, and a resend
  with the same id is refused by Alpaca and found by the lookup. The fake
  broker in `desk.rs` tests lost answers, dropped connections and proxy
  errors.
- **Keys stay out of logs and files.** They are `Secret` header values,
  shown as `***` by the event log and `Debug`, and never written to the
  audit log, which records bodies only. They live in Credential Manager.
- **Commands from another launch** arrive on a loopback port and need the
  token in `instance.port`, which sits in the user's own data folder.
- **Amounts are exact decimals**, and limit and stop prices off the
  exchange's price step are blocked before they reach Alpaca.

## Findings

| # | Severity | Finding | Status |
|---|---|---|---|
| 1 | Medium | The guardrails reviewed orders before the position and order lists had loaded. An empty list counted no position and no orders today, so the daily cap and the day-trade check passed, the position cap was measured from zero, and selling shares held looked like opening a short. | Fixed for v0.2.0 ([#19](https://github.com/ArenKDesai/miso-terminal/pull/19)): every review blocks until both lists are in. |
| 2 | Medium | Keys could follow a redirect to another host. reqwest drops `Authorization` on such a redirect, but keeps custom headers such as `APCA-API-KEY-ID`. | Fixed for v0.2.0 ([#20](https://github.com/ArenKDesai/miso-terminal/pull/20)): keyed requests never follow redirects and go only over TLS. |
| 3 | Medium, for Phase 6 | The guardrails are enforced by the tickets alone. `OrderDesk::submit` checks that a request is well formed, not that it passed its review, and does not check the kill switch's `trading.enabled`. | Fixed for `0.3.0` ([#24](https://github.com/ArenKDesai/miso-terminal/pull/24)): `submit` and `replace` take a `guard::Approved`, which only a passing review makes and which carries the reviewed request (a replacement must match it); the desk keeps its own copy of the switch, cleared by the kill switch before its cancels, and refuses new orders and replacements while it is off. An unreviewed approval exists only behind a dev-only feature, and a release build with it does not compile. |
| 4 | Low to medium, for Phase 6 | Prices are not checked for age. The collar and the caps use the last trade and quote however old they are, and a market order is valued at the ask. | Fixed for `0.3.0` ([#25](https://github.com/ArenKDesai/miso-terminal/pull/25)): every review (stock, option, spread) has a `price_age` rule. When the order would trade at once, the newest price must be within `[trading] max_price_age_secs` (120 s by default; a spread counts its oldest leg, and a price with no known time counts as old): past it a market order is blocked and any other order needs acknowledging. Left for Phase 6: whether a live limit order should block rather than warn, and marketable limit orders in place of market orders. |
| 5 | Low, for Phase 6 | `authed` attaches the keys to whatever URL it is given. The URLs are fixed in code today. | Open. With live keys: each kind of key goes only to its own host (paper keys to `paper-api.alpaca.markets` and the data API, live keys to `api.alpaca.markets`), checked where the headers are added. |
| 6 | Low | The desk treats every 4xx other than 422 (a reused id) and 429 as a refusal, so the ticket may be sent again. A 408 or 409 does not prove nothing was placed. A failed audit log write does not stop an order. | Open. Look ambiguous statuses up like a 5xx; for live, refuse to send while the audit log cannot be written. |
| 7 | Low | The instance token comes from the standard library's randomly keyed hasher rather than the operating system's random number generator, and is compared in ordinary time. | Open. Adequate for a loopback port only the same user can find; cheap to switch. |
| 8 | Low | ORD reviews a replacement with `day_trade: false`, so a replacement that changes the quantity skips the day-trade check (Alpaca still enforces its own). | Open. |
| 9 | Low | Found while fixing #3. SET drafts the whole configuration, so a draft opened before the kill switch still held `enabled = true`, and saving any other edit from it turned trading back on. | Fixed for `0.3.0` ([#24](https://github.com/ArenKDesai/miso-terminal/pull/24)): an open draft follows the switch when it changes. |

## Before Phase 6 ships

- Findings 3 to 6 fixed, each with a test.
- Live keys in their own credential entries, with a typed confirmation
  phrase and a restart to turn live on (the markets plan).
- Releases stay unsigned ([release plan](RELEASES.md#signing)), so the
  release notes say, beside live trading, to check the download's checksum
  and attestation before entering live keys.
- A second pass of this review over the Phase 6 changes, added here.
