# /// script
# requires-python = ">=3.10"
# dependencies = ["shapely>=2"]
# ///
"""Build assets/map/miso_map.json for the MAP function.

    uv run tools/build_map_asset.py [path/to/3D-MISO-Map]

Reads the 3D-MISO-Map pipeline's raw data (default: a sibling checkout):

  price_nodes.json      317 pricing nodes located by MISO's own contour-map feed
                        (getvectorsource, retired with the old data broker in
                        Dec 2025, so this snapshot is the record)
  states.geojson        US state outlines (Census TIGERweb, public domain)
  miso_footprint.geojson  MISO balancing-authority polygon (HIFLD, public domain)
  miso_lines.geojson    HIFLD transmission lines touching the footprint (public
                        domain); only the 230 kV-and-up backbone is kept

Polygons are simplified to ~1 km and clipped to the footprint's bounding box,
and coordinates are rounded, to keep the compiled-in asset small.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from shapely.geometry import box, shape

ROOT = Path(__file__).resolve().parent.parent
TOLERANCE = 0.01  # degrees, ~1 km
MARGIN = 1.5  # degrees around the footprint
MIN_AREA = 0.02  # square degrees; drops slivers and tiny islands from the footprint
LINE_TOLERANCE = 0.015  # degrees, ~1.5 km: enough for a whole-footprint map
# HIFLD voltage classes kept, and the nominal kV each is drawn as.
LINE_CLASSES = {"220-287": 230, "345": 345, "500": 500, "735 AND ABOVE": 765, "DC": 0}


def lines(raw: Path) -> list[dict]:
    """The transmission backbone as {"kv", "p": [lon, lat, lon, lat, ...]}."""
    out = []
    for f in json.loads((raw / "miso_lines.geojson").read_text())["features"]:
        props = f["properties"]
        kv = LINE_CLASSES.get(props.get("VOLT_CLASS"))
        if kv is None or props.get("STATUS") not in (None, "IN SERVICE"):
            continue
        g = shape(f["geometry"]).simplify(LINE_TOLERANCE, preserve_topology=False)
        for part in getattr(g, "geoms", [g]):
            if part.is_empty or part.geom_type != "LineString":
                continue
            flat = [round(c, 3) for xy in part.coords for c in xy]
            if len(flat) >= 4:
                out.append({"kv": kv, "p": flat})
    # Draw low voltages first so the 500/765 kV lines sit on top.
    out.sort(key=lambda l: l["kv"] or 1000)
    return out


def rings(geom, min_area: float = 0.0) -> list[list[list[float]]]:
    """Exterior rings of a (multi)polygon as [[lon, lat], ...] lists."""
    polys = getattr(geom, "geoms", [geom])
    out = []
    for p in polys:
        if p.is_empty or p.geom_type != "Polygon" or p.area < min_area:
            continue
        coords = [[round(x, 3), round(y, 3)] for x, y in p.exterior.coords]
        if len(coords) >= 4:
            out.append(coords)
    return out


def main() -> int:
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT.parent / "3D-MISO-Map"
    raw = src / "pipeline" / "raw"
    footprint = shape(json.loads((raw / "miso_footprint.geojson").read_text())["features"][0]["geometry"])
    footprint = footprint.simplify(TOLERANCE, preserve_topology=True)
    minx, miny, maxx, maxy = footprint.bounds
    clip = box(minx - MARGIN, miny - MARGIN, maxx + MARGIN, maxy + MARGIN)

    states = []
    for f in json.loads((raw / "states.geojson").read_text())["features"]:
        g = shape(f["geometry"]).simplify(TOLERANCE, preserve_topology=True)
        if g.intersects(clip):
            states.extend(rings(g.intersection(clip)))

    nodes = [
        {"node": n["node"], "type": n["type"], "lon": round(n["lon"], 4), "lat": round(n["lat"], 4)}
        for n in json.loads((raw / "price_nodes.json").read_text())
    ]
    out = {
        "source": "3D-MISO-Map pipeline: MISO getvectorsource node positions, Census TIGERweb states, HIFLD MISO footprint",
        "bounds": [round(minx, 3), round(miny, 3), round(maxx, 3), round(maxy, 3)],
        "nodes": nodes,
        "footprint": rings(footprint, MIN_AREA),
        "states": states,
        "lines": lines(raw),
    }
    dest = ROOT / "assets" / "map" / "miso_map.json"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(out, separators=(",", ":")), encoding="utf-8")
    print(f"wrote {dest} ({dest.stat().st_size // 1024} KB): {len(nodes)} nodes, "
          f"{len(out['footprint'])} footprint rings, {len(states)} state rings, "
          f"{len(out['lines'])} transmission lines")
    return 0


if __name__ == "__main__":
    sys.exit(main())
