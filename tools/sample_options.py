# /// script
# requires-python = ">=3.10"
# dependencies = ["tzdata"]
# ///
"""Write the sample option chains behind the repository's Alpaca fixtures.

    uv run tools/sample_options.py            # into fixtures/
    uv run tools/sample_options.py some\\dir

OMON, the option tickets and their visual snapshots replay these offline.
They are made up, not recorded: option quotes are licensed (OPRA), and a
recorded chain would be priced around the real stock while the fixtures'
stock prices are synthetic. So each chain is priced with Black-Scholes around
the synthetic last price in `data.alpaca.markets/v2/stocks/snapshots.json`,
with a volatility smile, quotes rounded out to the contract's price step,
greeks, a few trades, daily bars and open interest. The sample account's
option positions (`v1beta1/options/snapshots.json`, from
`tools/sample_account.py`) keep their quotes: each chain's volatility level
is fitted so the model prices them at their marks, and their greeks and
implied volatility are rewritten from the model, so PORT and OMON agree.
Run it after `tools/sample_account.py`.

Writes paper-api.alpaca.markets/v2/options/contracts@<UNDERLYING>.json (and an
empty contracts.json, what any other underlying replays: no options listed) and
data.alpaca.markets/v1beta1/options/snapshots/<UNDERLYING>@<expiry>.json,
and updates the greeks in data.alpaca.markets/v1beta1/options/snapshots.json.
"""

from __future__ import annotations

import hashlib
import json
import math
import sys
import uuid
from datetime import date, datetime, time, timedelta, timezone
from decimal import ROUND_CEILING, ROUND_FLOOR, Decimal
from pathlib import Path
from zoneinfo import ZoneInfo

NY = ZoneInfo("America/New_York")
UTC = timezone.utc
ROOT = Path(__file__).resolve().parent.parent / "fixtures"

#: The session the fixtures were recorded after (as in sample_account.py).
TODAY = date(2026, 10, 2)
RATE = 0.04

#: underlying -> (dividend yield, penny program, [(expiry, low, high, step)]). The
#: volatility level is fitted to a held contract's mark, else BASE_VOL.
CHAINS = {
    "XLU": (
        0.03,
        True,
        [
            ("2026-10-09", 40, 50, 0.5),
            ("2026-10-16", 38, 52, 0.5),
            ("2026-10-23", 40, 50, 0.5),
            ("2026-11-20", 36, 54, 1),
            ("2026-12-18", 35, 55, 1),
            ("2027-01-15", 35, 55, 1),
            ("2027-03-19", 35, 55, 2.5),
        ],
    ),
    "VST": (
        0.0,
        False,
        [
            ("2026-10-16", 30, 46, 1),
            ("2026-11-20", 28, 48, 1),
            ("2026-12-18", 25, 50, 2.5),
        ],
    ),
}
BASE_VOL = {"XLU": 0.20, "VST": 0.45}
#: A contract adjusted after a corporate action: listed, but not in the chain.
ADJUSTED = {"XLU": "XLU1261218C00045000"}


def h(*parts: object) -> float:
    """A stable number in [0, 1) from the parts."""
    digest = hashlib.sha1("/".join(map(str, parts)).encode()).digest()
    return int.from_bytes(digest[:8], "big") / 2**64


def uid(*parts: object) -> str:
    return str(uuid.UUID(bytes=hashlib.sha1("/".join(map(str, parts)).encode()).digest()[:16], version=4))


def z(t: datetime) -> str:
    return t.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"


def ny(d: date, t: time) -> datetime:
    return datetime.combine(d, t, NY).astimezone(UTC)


def occ(root: str, expiry: date, right: str, strike: float) -> str:
    return f"{root}{expiry:%y%m%d}{right}{round(strike * 1000):08d}"


def n_cdf(x: float) -> float:
    return 0.5 * (1 + math.erf(x / math.sqrt(2)))


def n_pdf(x: float) -> float:
    return math.exp(-x * x / 2) / math.sqrt(2 * math.pi)


def black_scholes(right: str, s: float, k: float, t: float, vol: float, q: float):
    """Price and greeks per share: theta per calendar day, vega and rho per point."""
    t = max(t, 1 / 365)
    sq = vol * math.sqrt(t)
    d1 = (math.log(s / k) + (RATE - q + vol * vol / 2) * t) / sq
    d2 = d1 - sq
    disc, carry = math.exp(-RATE * t), math.exp(-q * t)
    if right == "C":
        price = s * carry * n_cdf(d1) - k * disc * n_cdf(d2)
        delta = carry * n_cdf(d1)
        theta = -s * carry * n_pdf(d1) * vol / (2 * math.sqrt(t)) - RATE * k * disc * n_cdf(d2) + q * s * carry * n_cdf(d1)
        rho = k * t * disc * n_cdf(d2)
    else:
        price = k * disc * n_cdf(-d2) - s * carry * n_cdf(-d1)
        delta = -carry * n_cdf(-d1)
        theta = -s * carry * n_pdf(d1) * vol / (2 * math.sqrt(t)) + RATE * k * disc * n_cdf(-d2) - q * s * carry * n_cdf(-d1)
        rho = -k * t * disc * n_cdf(-d2)
    gamma = carry * n_pdf(d1) / (s * sq)
    vega = s * carry * n_pdf(d1) * math.sqrt(t)
    return max(price, 0.0), dict(delta=delta, gamma=gamma, theta=theta / 365, vega=vega / 100, rho=rho / 100)


def smile(base: float, s: float, k: float, t: float) -> float:
    """Implied volatility: a put skew and a little curvature, flatter for longer expiries."""
    m = math.log(k / s) / max(math.sqrt(t), 0.15)
    return min(max(base * (1 - 0.35 * m + 0.6 * m * m), 0.5 * base), 2.5 * base)


def tick(price: float, underlying: str, penny: bool) -> Decimal:
    if underlying in ("SPY", "QQQ", "IWM"):
        return Decimal("0.01")
    if price < 3:
        return Decimal("0.01") if penny else Decimal("0.05")
    return Decimal("0.05") if penny else Decimal("0.10")


def to_step(v: float, step: Decimal, rounding) -> float:
    return float((Decimal(str(v)) / step).to_integral_value(rounding=rounding) * step)


def quote(mid: float, underlying: str, penny: bool, symbol: str) -> tuple[float, float]:
    half = max(0.02, 0.035 * mid)
    step = tick(mid, underlying, penny)
    bid = to_step(max(mid - half, 0.0), step, ROUND_FLOOR)
    ask = to_step(mid + half, step, ROUND_CEILING)
    if ask <= bid:
        ask = bid + float(step)
    return round(bid, 2), round(ask, 2)


def years(expiry: date, at: date) -> float:
    return ((expiry - at).days + 0.25) / 365


def strikes(low: float, high: float, step: float) -> list[float]:
    out, k = [], low
    while k <= high + 1e-9:
        out.append(round(k, 3))
        k += step
    return out


def fit_base(right: str, s: float, k: float, t: float, q: float, target: float) -> float:
    """The volatility level at which the model prices a held contract at its mark."""
    lo, hi = 0.02, 3.0
    for _ in range(80):
        mid = (lo + hi) / 2
        if black_scholes(right, s, k, t, smile(mid, s, k, t), q)[0] < target:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2


def main() -> None:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT
    stocks = json.loads((root / "data.alpaca.markets/v2/stocks/snapshots.json").read_text())
    stocks = stocks.get("snapshots", stocks)
    held_path = root / "data.alpaca.markets/v1beta1/options/snapshots.json"
    held = json.loads(held_path.read_text())["snapshots"] if held_path.exists() else {}
    close_at = ny(TODAY, time(16, 0))
    prev_day = TODAY - timedelta(days=1)

    for underlying, (q, penny, expiries) in CHAINS.items():
        spot = float(stocks[underlying]["latestTrade"]["p"])
        prev_spot = float(stocks[underlying]["prevDailyBar"]["c"])
        # Fit the volatility level to a held contract on this underlying, if any.
        base = BASE_VOL[underlying]
        for symbol, snap in held.items():
            if not symbol.startswith(underlying) or not symbol[len(underlying)].isdigit():
                continue
            exp = datetime.strptime(symbol[len(underlying) : len(underlying) + 6], "%y%m%d").date()
            right, k = symbol[len(underlying) + 6], int(symbol[-8:]) / 1000
            mark = (snap["latestQuote"]["bp"] + snap["latestQuote"]["ap"]) / 2
            base = fit_base(right, spot, k, years(exp, TODAY), q, mark)
        contracts = []
        for exp_s, low, high, step in expiries:
            exp = date.fromisoformat(exp_s)
            t, t_prev = years(exp, TODAY), years(exp, prev_day)
            snaps = {}
            for k in strikes(low, high, step):
                for right in ("C", "P"):
                    symbol = occ(underlying, exp, right, k)
                    vol = smile(base, spot, k, t)
                    price, greeks = black_scholes(right, spot, k, t, vol, q)
                    prev_price = black_scholes(right, prev_spot, k, t_prev, smile(base, prev_spot, k, t_prev), q)[0]
                    bid, ask = quote(price, underlying, penny, symbol)
                    prev_close = max(round(prev_price, 2), 0.01)
                    snap = {
                        "latestQuote": {
                            "ap": ask, "as": 1 + int(h(symbol, "as") * 40), "ax": "C",
                            "bp": bid, "bs": 1 + int(h(symbol, "bs") * 40), "bx": "C",
                            "c": " ", "t": z(close_at - timedelta(seconds=1 + int(h(symbol, "qt") * 30))),
                        },
                        "greeks": {key: round(v, 4) for key, v in greeks.items()},
                        "impliedVolatility": round(vol, 4),
                        "prevDailyBar": {
                            "c": prev_close, "h": prev_close, "l": prev_close, "n": 1, "o": prev_close,
                            "t": z(ny(prev_day, time(0, 0))), "v": 1, "vw": prev_close,
                        },
                    }
                    # Busier near the money and at the monthly expiries.
                    moneyness = math.log(k / spot) / max(math.sqrt(t), 0.15)
                    monthly = exp.weekday() == 4 and 15 <= exp.day <= 21
                    activity = math.exp(-4 * moneyness * moneyness) * (1.0 if monthly else 0.4)
                    volume = int(activity * 400 * (0.3 + h(symbol, "v")))
                    oi = int(activity * 3000 * (0.4 + h(symbol, "oi")))
                    if volume > 0 and bid > 0:
                        last = round(min(max(price + (h(symbol, "p") - 0.5) * (ask - bid), bid), ask), 2)
                        traded = close_at - timedelta(minutes=5 + int(h(symbol, "tt") * 360))
                        snap["latestTrade"] = {
                            "c": "I", "p": last, "s": 1 + int(h(symbol, "s") * 9), "t": z(traded), "x": "C",
                        }
                        low_p = round(min(last, prev_close) * 0.97, 2)
                        high_p = round(max(last, prev_close) * 1.03, 2)
                        snap["dailyBar"] = {
                            "c": last, "h": high_p, "l": low_p, "n": max(1, volume // 7),
                            "o": prev_close, "t": z(ny(TODAY, time(0, 0))), "v": volume, "vw": last,
                        }
                    if symbol in held:
                        # The sample account's own contracts keep their quotes,
                        # and take the model's greeks.
                        for key in ("latestQuote", "latestTrade"):
                            if key in held[symbol]:
                                snap[key] = held[symbol][key]
                        held[symbol]["greeks"] = snap["greeks"]
                        held[symbol]["impliedVolatility"] = snap["impliedVolatility"]
                    snaps[symbol] = snap
                    contracts.append(
                        {
                            "id": uid("contract", symbol),
                            "symbol": symbol,
                            "name": f"{underlying} {exp:%b %d %Y} {k:g} {'Call' if right == 'C' else 'Put'}",
                            "status": "active",
                            "tradable": True,
                            "expiration_date": exp_s,
                            "root_symbol": underlying,
                            "underlying_symbol": underlying,
                            "underlying_asset_id": uid("asset", underlying),
                            "type": "call" if right == "C" else "put",
                            "style": "american",
                            "strike_price": f"{k:g}",
                            "multiplier": "100",
                            "size": "100",
                            "open_interest": str(oi),
                            "open_interest_date": prev_day.isoformat(),
                            "close_price": f"{prev_close:g}",
                            "close_price_date": prev_day.isoformat(),
                            "ppind": penny,
                        }
                    )
            write(
                root / f"data.alpaca.markets/v1beta1/options/snapshots/{underlying}@{exp_s}.json",
                dict(snapshots=dict(sorted(snaps.items())), next_page_token=None),
            )
        if underlying in ADJUSTED:
            symbol = ADJUSTED[underlying]
            contracts.append(
                {
                    "id": uid("contract", symbol), "symbol": symbol,
                    "name": f"{underlying} adjusted Dec 18 2026 45 Call", "status": "active", "tradable": True,
                    "expiration_date": "2026-12-18", "root_symbol": symbol[:-15], "underlying_symbol": underlying,
                    "underlying_asset_id": uid("asset", underlying), "type": "call", "style": "american",
                    "strike_price": "45", "multiplier": "100", "size": "150", "open_interest": "12",
                    "open_interest_date": prev_day.isoformat(), "close_price": None, "close_price_date": None,
                    "ppind": False,
                }
            )
        contracts.sort(key=lambda c: c["symbol"])
        write(
            root / f"paper-api.alpaca.markets/v2/options/contracts@{underlying}.json",
            dict(option_contracts=contracts, next_page_token=None),
        )
        print(f"{underlying}: {len(contracts)} contracts in {len(expiries)} expiries around {spot} (volatility {base:.3f})")
    write(root / "paper-api.alpaca.markets/v2/options/contracts.json", dict(option_contracts=[], next_page_token=None))
    if held:
        write(held_path, dict(snapshots=held, next_page_token=None))


def write(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, separators=(",", ":")), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
