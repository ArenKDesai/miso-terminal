# /// script
# requires-python = ">=3.10"
# dependencies = ["duckdb>=1.1"]
# ///
"""Export long hourly price history into MISO Terminal's local archive.

    uv run tools/export_history.py                       # hubs, load zones, interfaces
    uv run tools/export_history.py --nodes ALTE.ALTE MGE.AZ
    uv run tools/export_history.py --home some\\dir       # a portable install

Reads the Energy-Pricing-Journalist DuckDB (DA ex-post and RT nodal LMPs since
2023-01-01; default: a sibling checkout) read-only, and writes one file per
node into the terminal's cache, where GP and SPRD read history older than
their daily-report downloads. Re-run after refreshing the database; restart
the terminal to pick the new files up.

File format (gzip of): b"MTAH1\\0", first hour (i64 seconds, market time as if
UTC), hour count (u32), then six little-endian f32 columns of that length:
DA LMP, MCC, MLC, then RT LMP, MCC, MLC. Missing hours are NaN.
"""

from __future__ import annotations

import argparse
import gzip
import math
import os
import struct
import sys
from array import array
from datetime import datetime, timezone
from pathlib import Path

import duckdb

ROOT = Path(__file__).resolve().parent.parent
MAGIC = b"MTAH1\0"


def cache_dir(home: Path | None) -> Path:
    """Where the terminal keeps its cache (see resolve_paths in main.rs)."""
    if home is not None:
        return home / "cache" / "http"
    local = os.environ.get("LOCALAPPDATA")
    if not local:
        sys.exit("LOCALAPPDATA is not set; pass --home")
    return Path(local) / "MISO Terminal" / "cache" / "http"


def default_nodes(con) -> list[str]:
    """The eight trading hubs, every load zone and every interface."""
    rows = con.sql(
        "SELECT node FROM (SELECT node, any_value(node_type) AS t FROM lmp "
        "WHERE ts_est >= (SELECT max(ts_est) FROM lmp) - INTERVAL 7 DAY GROUP BY node) "
        "WHERE node LIKE '%.HUB' OR t IN ('Loadzone', 'Interface') ORDER BY node"
    ).fetchall()
    return [r[0] for r in rows]


def export(con, node: str, dest: Path) -> tuple[int, str]:
    rows = con.execute(
        "SELECT market, ts_est, lmp, mcc, mlc FROM lmp WHERE node = ? ORDER BY ts_est",
        [node],
    ).fetchall()
    if not rows:
        return 0, "no data"
    first = min(r[1] for r in rows)
    last = max(r[1] for r in rows)
    hours = int((last - first).total_seconds() // 3600) + 1
    cols = {m: [array("f", [math.nan]) * hours for _ in range(3)] for m in ("DA", "RT")}
    for market, ts, lmp, mcc, mlc in rows:
        i = int((ts - first).total_seconds() // 3600)
        for c, v in zip(cols[market], (lmp, mcc, mlc)):
            if v is not None:
                c[i] = v
    start = int(first.replace(tzinfo=timezone.utc).timestamp())
    body = bytearray(MAGIC)
    body += struct.pack("<qI", start, hours)
    for market in ("DA", "RT"):
        for c in cols[market]:
            if sys.byteorder != "little":
                c.byteswap()
            body += c.tobytes()
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(".tmp")
    tmp.write_bytes(gzip.compress(bytes(body), compresslevel=6))
    tmp.replace(dest)
    return hours, f"{first:%Y-%m-%d} to {last:%Y-%m-%d}"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", type=Path,
                    default=ROOT.parent / "Energy-Pricing-Journalist" / "db" / "data" / "miso_lmp.duckdb")
    ap.add_argument("--home", type=Path, help="the terminal's portable home, if it uses one")
    ap.add_argument("--nodes", nargs="+", help="nodes to export (default: hubs, load zones, interfaces)")
    args = ap.parse_args()
    if not args.db.exists():
        sys.exit(f"no database at {args.db}")
    con = duckdb.connect(str(args.db), read_only=True)
    nodes = [n.upper() for n in args.nodes] if args.nodes else default_nodes(con)
    out = cache_dir(args.home) / "archive" / "lmp"
    total = 0
    for node in nodes:
        # The terminal reads local://archive/lmp/<NODE> from its disk cache,
        # which stores entries gzipped under <cache>/archive/lmp/<NODE>.gz.
        safe = "".join(c if c.isascii() and (c.isalnum() or c in ".-_") else "_" for c in node)
        dest = out / f"{safe}.gz"
        hours, span = export(con, node, dest)
        if hours:
            total += dest.stat().st_size
        print(f"{node:<24} {span}")
    print(f"{len(nodes)} nodes, {total / 1e6:.1f} MB in {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
