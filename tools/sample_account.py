# /// script
# requires-python = ">=3.10"
# dependencies = ["tzdata"]
# ///
"""Write the sample paper account behind the repository's Alpaca fixtures.

    uv run tools/sample_account.py            # into fixtures/
    uv run tools/sample_account.py some\\dir

PORT, ACCT, PNL and ACT replay these offline and in the visual snapshots.
They are made up, not recorded: the CI paper account holds nothing worth
showing, and a real account's figures do not belong in a public repository.
The portfolio trades at the synthetic prices already in the fixtures
(`data.alpaca.markets/v2/stocks/*.json`, from `capture_alpaca`), so
everything reconciles: equity is cash plus positions, the day's P&L is the
positions' day P&L, and the equity curve ends at the account's equity. Run it
again after re-recording the Alpaca fixtures.

Writes, under paper-api.alpaca.markets/: v2/account.json, v2/positions.json,
v2/account/activities.json, v2/account/portfolio/history@{1D,1W,1M,3M,1A}.json
and stream.jsonl (a login and the day's order events); and
data.alpaca.markets/v1beta1/options/snapshots.json (greeks for the options).
"""

from __future__ import annotations

import hashlib
import json
import math
import sys
import uuid
from datetime import date, datetime, time, timedelta, timezone
from decimal import ROUND_HALF_UP, Decimal
from pathlib import Path
from zoneinfo import ZoneInfo

NY = ZoneInfo("America/New_York")
UTC = timezone.utc
ROOT = Path(__file__).resolve().parent.parent / "fixtures"

#: The session the fixtures were recorded after.
TODAY = date(2026, 10, 2)
CREATED = datetime(2026, 4, 1, 14, 2, 11, tzinfo=UTC)
START_CASH = Decimal("100000")
ACCOUNT_NUMBER = "PA00SAMPLE01"

#: (date, symbol, side, qty): stock fills at that day's close.
STOCK_TRADES = [
    ("2026-05-04", "XLU", "buy", 200),
    ("2026-05-18", "AEE", "buy", 300),
    ("2026-06-08", "XEL", "buy", 40),
    ("2026-07-13", "VST", "buy", 200),
    ("2026-08-24", "UNG", "sell_short", 50),
    ("2026-09-15", "VST", "sell", 50),
]
#: (date, OCC symbol, side, contracts, premium).
OPTION_TRADES = [
    ("2026-08-20", "XLU260918C00046000", "buy", 1, "0.40"),
    ("2026-09-08", "XLU261218C00045000", "buy", 2, "1.35"),
    ("2026-09-21", "VST261120P00035000", "sell", 1, "1.10"),
]
#: OCC symbol -> (previous close, now, delta, implied volatility).
OPTION_MARKS = {
    "XLU261218C00045000": ("1.28", "1.60", 0.52, 0.214),
    "VST261120P00035000": ("0.62", "0.88", -0.30, 0.468),
}
#: Expired worthless.
EXPIRED = {"XLU260918C00046000": "2026-09-18"}
#: (pay date, symbol, per share).
DIVIDENDS = [
    ("2026-06-22", "XLU", "0.19"),
    ("2026-06-30", "AEE", "0.71"),
    ("2026-07-20", "XEL", "0.57"),
    ("2026-09-22", "XLU", "0.19"),
    ("2026-09-30", "AEE", "0.71"),
]
#: Today's order: bought at 14:31 New York time, in two fills.
TODAY_ORDER = ("CEG", 60, [20, 40], time(14, 31))
EXCHANGES = {"XLU": "ARCA", "XEL": "NASDAQ", "VST": "NYSE", "CEG": "NASDAQ", "AEE": "NYSE", "UNG": "ARCA"}


def uid(*parts: object) -> str:
    return str(uuid.UUID(bytes=hashlib.sha1("/".join(map(str, parts)).encode()).digest()[:16], version=4))


def money(d: Decimal, places: str = "0.01") -> Decimal:
    return d.quantize(Decimal(places), rounding=ROUND_HALF_UP)


def s(d: Decimal | int) -> str:
    """Alpaca's string amounts: no exponent, no trailing zeros."""
    t = format(Decimal(d).normalize(), "f")
    return t if t != "-0" else "0"


def ny(d: date, t: time) -> datetime:
    return datetime.combine(d, t, NY).astimezone(UTC)


def z(t: datetime) -> str:
    return t.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"


def ny_date(stamp: str) -> date:
    return datetime.fromisoformat(stamp.replace("Z", "+00:00")).astimezone(NY).date()


def load(root: Path):
    stocks = root / "data.alpaca.markets" / "v2" / "stocks"
    snaps = json.loads((stocks / "snapshots.json").read_text())
    daily_raw = json.loads((stocks / "bars@1Day.json").read_text())["bars"]
    q15 = json.loads((stocks / "bars@15Min.json").read_text())["bars"]
    daily = {
        sym: {ny_date(b["t"]): Decimal(str(b["c"])) for b in bars} for sym, bars in daily_raw.items()
    }
    # A 15-minute bar's close is the price at its end.
    intraday = {
        sym: sorted(
            (datetime.fromisoformat(b["t"].replace("Z", "+00:00")) + timedelta(minutes=15), b["c"])
            for b in bars
        )
        for sym, bars in q15.items()
    }
    return snaps, daily, intraday


def trading_days(daily) -> list[date]:
    return sorted(daily["XLU"].keys())


def on_or_after(days: list[date], d: date) -> date:
    return next(x for x in days if x >= d)


def main() -> None:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT
    snaps, daily, intraday = load(root)
    days = trading_days(daily)
    prev_day = max(d for d in days if d < TODAY)
    current = {sym: Decimal(str(snaps[sym]["latestTrade"]["p"])) for sym in EXCHANGES}
    lastday = {sym: Decimal(str(snaps[sym]["prevDailyBar"]["c"])) for sym in EXCHANGES}
    assert all(lastday[sym] == daily[sym][prev_day] for sym in EXCHANGES), "snapshots and daily bars disagree"

    # ------------------------------------------------------------ the ledger
    # Every cash and position change, in order: (when, kind, details).
    events = []
    for d, sym, side, qty in STOCK_TRADES:
        day = on_or_after(days, date.fromisoformat(d))
        events.append((ny(day, time(15, 45)), "fill", dict(symbol=sym, side=side, qty=qty, price=daily[sym][day])))
    for d, occ, side, qty, prem in OPTION_TRADES:
        day = on_or_after(days, date.fromisoformat(d))
        events.append((ny(day, time(10, 5)), "fill", dict(symbol=occ, side=side, qty=qty, price=Decimal(prem))))
    for occ, d in EXPIRED.items():
        events.append((ny(date.fromisoformat(d), time(16, 0)), "expire", dict(symbol=occ)))
    for d, sym, per in DIVIDENDS:
        events.append((ny(date.fromisoformat(d), time(7, 0)), "div", dict(symbol=sym, per=Decimal(per))))
    sym, total, parts, at = TODAY_ORDER
    order_at = ny(TODAY, at)
    # The price at the fill: the 15-minute close nearest it.
    fill_price = money(Decimal(str(min(intraday[sym], key=lambda p: abs(p[0] - order_at))[1])))
    for i, q in enumerate(parts):
        events.append(
            (order_at + timedelta(seconds=2 * i), "fill", dict(symbol=sym, side="buy", qty=q, price=fill_price, part=i))
        )
    events.sort(key=lambda e: e[0])

    def mult(symbol: str) -> int:
        return 100 if len(symbol) > 10 else 1

    cash = START_CASH
    pos: dict[str, dict] = {}
    activities = []
    snapshots_of_book = []  # (when, cash, {symbol: qty}) after each event
    for when, kind, e in events:
        symbol = e["symbol"]
        if kind == "fill":
            sign = 1 if e["side"] == "buy" else -1
            qty, price = e["qty"], e["price"]
            cash -= sign * qty * price * mult(symbol)
            p = pos.setdefault(symbol, dict(qty=0, avg=Decimal(0)))
            new_qty = p["qty"] + sign * qty
            if p["qty"] == 0 or (p["qty"] > 0) == (sign > 0):
                # Opening or adding: average in.
                p["avg"] = (abs(p["qty"]) * p["avg"] + qty * price) / abs(new_qty)
            p["qty"] = new_qty
            if p["qty"] == 0:
                del pos[symbol]
            order_id = uid("order", symbol, when.date() if "part" not in e else "today")
            partial = "part" in e and e["part"] < len(parts) - 1
            cum = sum(parts[: e["part"] + 1]) if "part" in e else qty
            activities.append(
                dict(
                    activity_type="FILL",
                    cum_qty=str(cum),
                    id=when.strftime("%Y%m%d%H%M%S%f")[:-3] + "::" + uid("fill", symbol, when),
                    leaves_qty=str((total - cum) if "part" in e else 0),
                    price=s(price),
                    qty=str(qty),
                    side=e["side"],
                    symbol=symbol,
                    transaction_time=z(when),
                    order_id=order_id,
                    type="partial_fill" if partial else "fill",
                    order_status="partially_filled" if partial else "filled",
                )
            )
        elif kind == "expire":
            qty = pos.pop(symbol)["qty"]
            activities.append(
                dict(
                    activity_type="OPEXP",
                    id=when.strftime("%Y%m%d%H%M%S%f")[:-3] + "::" + uid("exp", symbol),
                    date=when.astimezone(NY).date().isoformat(),
                    net_amount="0",
                    symbol=symbol,
                    qty=str(-qty),
                    price="0",
                    description="Option Expiry",
                    status="executed",
                )
            )
        elif kind == "div":
            held = pos[symbol]["qty"]
            amount = money(held * e["per"])
            cash += amount
            activities.append(
                dict(
                    activity_type="DIV",
                    activity_sub_type="CDIV",
                    id=when.strftime("%Y%m%d%H%M%S%f")[:-3] + "::" + uid("div", symbol, when),
                    date=when.astimezone(NY).date().isoformat(),
                    net_amount=s(amount),
                    symbol=symbol,
                    qty=str(held),
                    per_share_amount=s(e["per"]),
                    description=f"Cash DIV @ {e['per']}, Pos {held} @ {when.astimezone(NY).date()}",
                    status="executed",
                )
            )
        snapshots_of_book.append((when, cash, {k: v["qty"] for k, v in pos.items()}))
    activities.sort(key=lambda a: a["id"], reverse=True)

    # ------------------------------------------------------- the positions
    def option_price(symbol: str, at: datetime, opened: datetime, premium: Decimal) -> Decimal:
        """Premium at entry, the previous close by then, today's mark now (or
        nothing at expiry, for one that expired)."""
        if symbol in EXPIRED:
            end = ny(date.fromisoformat(EXPIRED[symbol]), time(16, 0))
            frac = Decimal(min(1.0, max(0.0, (at - opened).total_seconds() / (end - opened).total_seconds())))
            return premium * (1 - frac)
        prev, now = (Decimal(x) for x in OPTION_MARKS[symbol][:2])
        prev_close = ny(prev_day, time(16, 0))
        if at >= prev_close:
            frac = Decimal(min(1.0, (at - prev_close).total_seconds() / (ny(TODAY, time(16, 0)) - prev_close).total_seconds()))
            return prev + (now - prev) * frac
        frac = Decimal(max(0.0, (at - opened).total_seconds() / (prev_close - opened).total_seconds()))
        return premium + (prev - premium) * frac

    opened = {}
    for when, kind, e in events:
        if kind == "fill" and e["symbol"] not in opened:
            opened[e["symbol"]] = (when, e["price"])

    positions = []
    long_mv = short_mv = Decimal(0)
    for symbol, p in pos.items():
        qty, avg, m = p["qty"], p["avg"], mult(symbol)
        is_option = m == 100
        cur = Decimal(OPTION_MARKS[symbol][1]) if is_option else current[symbol]
        prev = Decimal(OPTION_MARKS[symbol][0]) if is_option else lastday[symbol]
        opened_today = opened[symbol][0].astimezone(NY).date() == TODAY
        ref = avg if opened_today else prev
        mv = qty * cur * m
        cost = qty * avg * m
        upl = mv - cost
        ipl = qty * (cur - ref) * m
        if mv >= 0:
            long_mv += mv
        else:
            short_mv += mv
        positions.append(
            dict(
                asset_id=uid("asset", symbol),
                symbol=symbol,
                exchange="" if is_option else EXCHANGES[symbol],
                asset_class="us_option" if is_option else "us_equity",
                asset_marginable=not is_option,
                qty=s(qty),
                avg_entry_price=s(money(avg, "0.0001")),
                side="long" if qty > 0 else "short",
                market_value=s(money(mv)),
                cost_basis=s(money(cost)),
                unrealized_pl=s(money(upl)),
                unrealized_plpc=s(money(upl / abs(cost), "0.000001")),
                unrealized_intraday_pl=s(money(ipl)),
                unrealized_intraday_plpc=s(money(ipl / abs(qty * ref * m), "0.000001")),
                current_price=s(cur),
                lastday_price=s(prev),
                change_today=s(money(cur / prev - 1, "0.000001")),
                qty_available=s(qty),
            )
        )
    positions.sort(key=lambda p: abs(Decimal(p["market_value"])), reverse=True)
    long_mv, short_mv = money(long_mv), money(short_mv)
    equity = cash + long_mv + short_mv
    day_pl = sum(Decimal(p["unrealized_intraday_pl"]) for p in positions)
    last_equity = equity - day_pl

    initial = money((long_mv - short_mv) / 2)
    maintenance = money((long_mv - short_mv) * Decimal("0.3"))
    excess = equity - initial
    account = dict(
        id=uid("account"),
        admin_configurations={},
        user_configurations=None,
        account_number=ACCOUNT_NUMBER,
        status="ACTIVE",
        crypto_status="ACTIVE",
        options_approved_level=3,
        options_trading_level=3,
        currency="USD",
        buying_power=s(money(excess * 2)),
        regt_buying_power=s(money(excess * 2)),
        daytrading_buying_power="0",
        effective_buying_power=s(money(excess * 2)),
        non_marginable_buying_power=s(money(excess)),
        options_buying_power=s(money(excess)),
        bod_dtbp="0",
        cash=s(money(cash)),
        accrued_fees="0",
        portfolio_value=s(money(equity)),
        pattern_day_trader=False,
        trading_blocked=False,
        transfers_blocked=False,
        account_blocked=False,
        created_at=z(CREATED),
        trade_suspended_by_user=False,
        multiplier="2",
        shorting_enabled=True,
        equity=s(money(equity)),
        last_equity=s(money(last_equity)),
        long_market_value=s(long_mv),
        short_market_value=s(short_mv),
        position_market_value=s(money(long_mv - short_mv)),
        initial_margin=s(initial),
        maintenance_margin=s(maintenance),
        last_maintenance_margin=s(maintenance),
        sma=s(money(excess)),
        daytrade_count=0,
        balance_asof=prev_day.isoformat(),
        crypto_tier=1,
        intraday_adjustments="0",
        pending_reg_taf_fees="0",
    )

    # -------------------------------------------------- the equity curve
    def book_at(t: datetime):
        c, held = START_CASH, {}
        for when, c2, h in snapshots_of_book:
            if when <= t:
                c, held = c2, h
        return c, held

    def stock_price(sym: str, t: datetime) -> Decimal:
        """The 15-minute closes where the fixtures have them, else a gentle
        path from the previous close to the day's close."""
        d = t.astimezone(NY).date()
        open_at, close_at = ny(d, time(9, 30)), ny(d, time(16, 0))
        close = current[sym] if d == TODAY else daily[sym][d]
        prev_close = daily[sym][max(x for x in days if x < d)]
        if t >= close_at:
            return close
        if t <= open_at:
            return prev_close
        pts = [(t0, c) for t0, c in intraday.get(sym, []) if open_at < t0 < close_at]
        knots = [(open_at, float(prev_close))] + pts + [(close_at, float(close))]
        seed = sum(map(ord, sym)) % 7
        for (t0, v0), (t1, v1) in zip(knots, knots[1:]):
            if t0 <= t <= t1:
                f = (t - t0).total_seconds() / max(1.0, (t1 - t0).total_seconds())
                wiggle = 0.0 if pts else 0.004 * v0 * math.sin(f * math.pi) * math.sin(seed + t.hour)
                return Decimal(str(round(v0 + (v1 - v0) * f + wiggle, 4)))
        return close

    def equity_at(t: datetime, closing: bool) -> Decimal | None:
        if t < CREATED:
            return None
        c, held = book_at(t)
        total = c
        for sym, qty in held.items():
            if mult(sym) == 100:
                when, prem = opened[sym]
                total += qty * 100 * option_price(sym, t, when, prem)
            elif closing:
                total += qty * daily[sym][t.astimezone(NY).date()]
            else:
                total += qty * stock_price(sym, t)
        return money(total)

    def history(name: str, stamps: list[datetime], closing: bool, base: Decimal, timeframe: str, base_day: date):
        eq = [equity_at(t, closing) for t in stamps]
        # Now is the account's equity.
        eq[-1] = money(equity)
        out = dict(
            timestamp=[int(t.timestamp()) for t in stamps],
            equity=[float(e) if e is not None else None for e in eq],
            profit_loss=[float(money(e - base)) if e is not None else None for e in eq],
            profit_loss_pct=[round(float((e - base) / base), 6) if e is not None else None for e in eq],
            base_value=float(base),
            base_value_asof=base_day.isoformat(),
            timeframe=timeframe,
            cashflow={},
        )
        write(root / "paper-api.alpaca.markets/v2/account/portfolio" / f"history@{name}.json", out)

    def close_of(d: date) -> Decimal:
        return equity_at(ny(d, time(16, 0)), True) or START_CASH

    # 1D: every five minutes of today's session, from the previous close.
    day_stamps = [ny(TODAY, time(9, 30)) + timedelta(minutes=5 * i) for i in range(79)]
    history("1D", day_stamps, False, last_equity, "5Min", prev_day)
    # 1W: hourly over five sessions.
    week = [d for d in days if d <= TODAY][-5:]
    week_stamps = [ny(d, time(9, 30)) + timedelta(hours=h) for d in week for h in range(7)]
    week_stamps.append(ny(TODAY, time(16, 0)))
    before_week = max(d for d in days if d < week[0])
    history("1W", week_stamps, False, close_of(before_week), "1H", before_week)
    # 1M, 3M, 1A: daily closes (stamped at midnight New York time).
    for name, span in [("1M", 31), ("3M", 92), ("1A", 365)]:
        chosen = [d for d in days if TODAY - timedelta(days=span) < d <= TODAY]
        stamps = [ny(d, time(16, 0)) for d in chosen]
        base_day = max(d for d in days if d < chosen[0])
        if ny(base_day, time(16, 0)) < CREATED:
            # Before the account existed: P&L from the opening balance.
            base_day, base = CREATED.astimezone(NY).date(), START_CASH
        else:
            base = close_of(base_day)
        out_stamps = [ny(d, time(0, 0)) for d in chosen]
        eq = [equity_at(t, True) for t in stamps]
        eq[-1] = money(equity)
        write(
            root / "paper-api.alpaca.markets/v2/account/portfolio" / f"history@{name}.json",
            dict(
                timestamp=[int(t.timestamp()) for t in out_stamps],
                equity=[float(e) if e is not None else None for e in eq],
                profit_loss=[float(money(e - base)) if e is not None else None for e in eq],
                profit_loss_pct=[round(float((e - base) / base), 6) if e is not None else None for e in eq],
                base_value=float(base),
                base_value_asof=base_day.isoformat(),
                timeframe="1D",
                cashflow={},
            ),
        )

    # -------------------------------------------- options and order events
    opt = {}
    for occ, (prev, now, delta, iv) in OPTION_MARKS.items():
        mid = Decimal(now)
        half = max(Decimal("0.05"), money(mid * Decimal("0.03")))
        t = z(ny(TODAY, time(15, 59, 58)))
        opt[occ] = dict(
            latestQuote=dict(ap=float(mid + half), bp=float(mid - half), **{"as": 12}, bs=9, ax="C", bx="C", c=" ", t=t),
            latestTrade=dict(p=float(mid), s=1, x="C", c="I", t=t),
            greeks=dict(
                delta=delta,
                gamma=round(abs(delta) * 0.14, 4),
                theta=round(-float(mid) * 0.012, 4),
                vega=round(float(mid) * 0.045, 4),
                rho=round(delta * 0.04, 4),
            ),
            impliedVolatility=iv,
        )
    write(root / "data.alpaca.markets/v1beta1/options/snapshots.json", dict(snapshots=opt, next_page_token=None))

    oid = uid("order", sym, "today")
    order = dict(
        id=oid,
        client_order_id=uid("client", sym, "today"),
        created_at=z(order_at - timedelta(seconds=2)),
        submitted_at=z(order_at - timedelta(seconds=2)),
        symbol=sym,
        asset_class="us_equity",
        side="buy",
        type="limit",
        order_type="limit",
        time_in_force="day",
        qty=str(total),
        limit_price=s(fill_price + Decimal("0.05")),
        extended_hours=False,
        order_class="",
    )
    frames = [
        dict(stream="authorization", data=dict(status="authorized", action="authenticate")),
        dict(stream="listening", data=dict(streams=["trade_updates"])),
        dict(
            stream="trade_updates",
            data=dict(event="new", timestamp=z(order_at - timedelta(seconds=1)), order=dict(order, status="new", filled_qty="0")),
        ),
    ]
    filled = 0
    for i, q in enumerate(parts):
        filled += q
        last = i == len(parts) - 1
        frames.append(
            dict(
                stream="trade_updates",
                data=dict(
                    event="fill" if last else "partial_fill",
                    execution_id=uid("exec", i),
                    timestamp=z(order_at + timedelta(seconds=2 * i)),
                    price=s(fill_price),
                    qty=str(q),
                    position_qty=str(filled),
                    order=dict(
                        order,
                        status="filled" if last else "partially_filled",
                        filled_qty=str(filled),
                        filled_avg_price=s(fill_price),
                        updated_at=z(order_at + timedelta(seconds=2 * i)),
                    ),
                ),
            )
        )
    stream = root / "paper-api.alpaca.markets/stream.jsonl"
    stream.write_text("\n".join(json.dumps(f, separators=(",", ":")) for f in frames) + "\n", encoding="utf-8", newline="\n")
    print(f"{stream}")

    write(root / "paper-api.alpaca.markets/v2/account.json", account)
    write(root / "paper-api.alpaca.markets/v2/positions.json", positions)
    write(root / "paper-api.alpaca.markets/v2/account/activities.json", activities)
    print(f"equity {money(equity)}  cash {money(cash)}  day P&L {day_pl}  positions {len(positions)}  activities {len(activities)}")


def write(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, separators=(",", ":")), encoding="utf-8", newline="\n")
    print(path)


if __name__ == "__main__":
    main()
