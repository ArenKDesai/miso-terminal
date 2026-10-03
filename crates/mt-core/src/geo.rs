//! Planar geometry for maps: point-in-polygon masks and inverse-distance-
//! weighted surfaces on a regular grid. Coordinates are whatever plane the
//! caller projected into.

/// A point `[x, y]`.
pub type Pt = [f64; 2];

/// Ray-casting point-in-polygon test for one ring.
pub fn point_in_ring(p: Pt, ring: &[Pt]) -> bool {
    let mut inside = false;
    let mut j = ring.len().wrapping_sub(1);
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Bounding box of every point in `rings`.
pub fn bounds<'a>(rings: impl IntoIterator<Item = &'a [Pt]>) -> Option<(Pt, Pt)> {
    let mut b: Option<(Pt, Pt)> = None;
    for p in rings.into_iter().flatten() {
        let (lo, hi) = b.get_or_insert((*p, *p));
        *lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        *hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    b
}

/// A regular grid over a rectangle; row 0 is the top (largest y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    pub min: Pt,
    pub max: Pt,
    pub nx: usize,
    pub ny: usize,
}

impl GridSpec {
    /// `nx` columns, rows chosen to keep cells square.
    pub fn new(min: Pt, max: Pt, nx: usize) -> Self {
        let nx = nx.max(1);
        let w = (max[0] - min[0]).max(f64::EPSILON);
        let h = (max[1] - min[1]).max(f64::EPSILON);
        let ny = ((h / w) * nx as f64).round().max(1.0) as usize;
        Self { min, max, nx, ny }
    }

    pub fn center(&self, ix: usize, iy: usize) -> Pt {
        let dx = (self.max[0] - self.min[0]) / self.nx as f64;
        let dy = (self.max[1] - self.min[1]) / self.ny as f64;
        [
            self.min[0] + dx * (ix as f64 + 0.5),
            self.max[1] - dy * (iy as f64 + 0.5),
        ]
    }

    pub fn cells(&self) -> usize {
        self.nx * self.ny
    }
}

/// Which cells (row-major, top row first) fall inside any of `rings`.
pub fn mask(spec: &GridSpec, rings: &[Vec<Pt>]) -> Vec<bool> {
    let boxes: Vec<Option<(Pt, Pt)>> = rings.iter().map(|r| bounds([r.as_slice()])).collect();
    (0..spec.cells())
        .map(|i| {
            let p = spec.center(i % spec.nx, i / spec.nx);
            rings.iter().zip(&boxes).any(|(ring, b)| {
                b.is_some_and(|(lo, hi)| {
                    p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]
                }) && point_in_ring(p, ring)
            })
        })
        .collect()
}

/// Inverse-distance-weighted values for the masked cells (`None` outside the
/// mask or without samples). Larger `power` keeps values more local.
pub fn idw(spec: &GridSpec, mask: &[bool], samples: &[(Pt, f64)], power: i32) -> Vec<Option<f64>> {
    (0..spec.cells())
        .map(|i| {
            if !mask.get(i).copied().unwrap_or(false) || samples.is_empty() {
                return None;
            }
            let p = spec.center(i % spec.nx, i / spec.nx);
            let (mut num, mut den) = (0.0, 0.0);
            for (q, v) in samples {
                let d2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
                if d2 < 1e-18 {
                    return Some(*v);
                }
                let w = 1.0 / d2.sqrt().powi(power);
                num += w * v;
                den += w;
            }
            (den > 0.0).then(|| num / den)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<Pt> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn points_in_and_out_of_a_ring() {
        assert!(point_in_ring([5.0, 5.0], &square()));
        assert!(!point_in_ring([15.0, 5.0], &square()));
        assert!(!point_in_ring([5.0, -1.0], &square()));
        assert_eq!(
            bounds([square().as_slice()]),
            Some(([0.0, 0.0], [10.0, 10.0]))
        );
    }

    #[test]
    fn grid_mask_and_surface() {
        let spec = GridSpec::new([-5.0, 0.0], [15.0, 10.0], 20);
        assert_eq!(spec.ny, 10, "square cells");
        assert_eq!(spec.center(0, 0), [-4.5, 9.5], "row 0 is the top");
        let m = mask(&spec, &[square()]);
        assert_eq!(
            m.iter().filter(|x| **x).count(),
            100,
            "the 10x10 square's cells"
        );
        let samples = [([2.5, 5.0], 10.0), ([7.5, 5.0], 30.0)];
        let v = idw(&spec, &m, &samples, 2);
        assert!(v[0].is_none(), "outside the mask");
        let mid = v[5 * 20 + 10].unwrap(); // x = 5.5, between the samples
        assert!(mid > 10.0 && mid < 30.0);
        let near_left = v[5 * 20 + 7].unwrap(); // x = 2.5, on the first sample
        assert!((near_left - 10.0).abs() < 1.0);
    }
}
