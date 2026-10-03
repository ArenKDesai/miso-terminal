//! Map geometry: where MISO's pricing nodes are, the footprint and state
//! outlines, and the projection used to draw them.
//!
//! The data is compiled in from `assets/map/miso_map.json` (built by
//! `tools/build_map_asset.py`). Node positions come from MISO's own
//! contour-map feed, so only the ~317 nodes MISO plots have a location.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const ASSET: &str = include_str!("../../../assets/map/miso_map.json");

#[derive(Debug, Deserialize)]
pub struct GeoNode {
    pub node: String,
    /// `Hub`, `Load zone`, `Interface` or `Generator`.
    #[serde(rename = "type")]
    pub kind: String,
    pub lon: f64,
    pub lat: f64,
}

/// One transmission line of the 230 kV-and-up backbone (HIFLD), simplified.
#[derive(Debug, Deserialize)]
pub struct GeoLine {
    /// Nominal voltage, kV: 230, 345, 500 or 765.
    pub kv: u16,
    /// `lon, lat, lon, lat, ...`
    #[serde(rename = "p")]
    pub points: Vec<f64>,
}

#[derive(Debug, Deserialize)]
pub struct MapData {
    pub source: String,
    pub nodes: Vec<GeoNode>,
    /// Outer rings of the MISO footprint, `[lon, lat]`.
    pub footprint: Vec<Vec<[f64; 2]>>,
    /// State outlines clipped to the footprint's surroundings.
    pub states: Vec<Vec<[f64; 2]>>,
    /// The transmission backbone, lowest voltage first.
    #[serde(default)]
    pub lines: Vec<GeoLine>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

impl MapData {
    pub fn node(&self, name: &str) -> Option<&GeoNode> {
        self.index.get(name).map(|&i| &self.nodes[i])
    }
}

/// The embedded map, parsed once.
pub fn map() -> &'static MapData {
    static MAP: OnceLock<MapData> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m: MapData = serde_json::from_str(ASSET).expect("embedded map asset is valid JSON");
        m.index = m
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.node.clone(), i))
            .collect();
        m
    })
}

/// Transmission lines in projected coordinates, `(kV, points)`, computed once.
pub fn lines_xy() -> &'static [(u16, Vec<[f64; 2]>)] {
    static LINES: OnceLock<Vec<(u16, Vec<[f64; 2]>)>> = OnceLock::new();
    LINES.get_or_init(|| {
        map()
            .lines
            .iter()
            .map(|l| {
                let pts = l
                    .points
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[lon, lat]| project(*lon, *lat))
                    .collect();
                (l.kv, pts)
            })
            .collect()
    })
}

/// The footprint's rings in projected coordinates, computed once.
pub fn footprint_xy() -> &'static [Vec<[f64; 2]>] {
    static RINGS: OnceLock<Vec<Vec<[f64; 2]>>> = OnceLock::new();
    RINGS.get_or_init(|| {
        map()
            .footprint
            .iter()
            .map(|r| r.iter().map(|p| project(p[0], p[1])).collect())
            .collect()
    })
}

/// Lambert conformal conic (standard parallels 33°N and 45°N, central meridian
/// 100°W): the projection MISO's own maps use. Spherical, unit radius; the
/// output is only ever used for drawing.
pub fn project(lon: f64, lat: f64) -> [f64; 2] {
    use std::f64::consts::FRAC_PI_4;
    let (p1, p2, p0, l0) = (
        33f64.to_radians(),
        45f64.to_radians(),
        38f64.to_radians(),
        (-100f64).to_radians(),
    );
    let t = |p: f64| (FRAC_PI_4 + p / 2.0).tan();
    let n = (p1.cos() / p2.cos()).ln() / (t(p2) / t(p1)).ln();
    let f = p1.cos() * t(p1).powf(n) / n;
    let rho = |p: f64| f / t(p).powf(n);
    let theta = n * (lon.to_radians() - l0);
    [
        rho(lat.to_radians()) * theta.sin(),
        rho(p0) - rho(lat.to_radians()) * theta.cos(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_loads_and_nodes_are_in_north_america() {
        let m = map();
        assert!(m.nodes.len() > 300);
        assert!(!m.footprint.is_empty() && !m.states.is_empty());
        assert_eq!(
            m.nodes.iter().filter(|n| n.node.ends_with(".HUB")).count(),
            8
        );
        assert!(m.node("MINN.HUB").is_some());
        assert!(m.lines.len() > 1000, "the transmission backbone");
        assert!(m.lines.iter().all(|l| [230, 345, 500, 765].contains(&l.kv)
            && l.points.len() >= 4
            && l.points.len() % 2 == 0));
        for n in &m.nodes {
            assert!(
                // Interfaces sit in neighbouring regions (CPLE is in the Carolinas).
                (-125.0..-66.0).contains(&n.lon) && (24.0..55.0).contains(&n.lat),
                "{}",
                n.node
            );
        }
    }

    #[test]
    fn projection_keeps_compass_directions() {
        let minneapolis = project(-93.27, 44.98);
        let new_orleans = project(-90.07, 29.95);
        let detroit = project(-83.05, 42.33);
        assert!(minneapolis[1] > new_orleans[1], "north is up");
        assert!(detroit[0] > minneapolis[0], "east is right");
        // The central meridian projects to x = 0.
        assert!(project(-100.0, 40.0)[0].abs() < 1e-12);
    }
}
