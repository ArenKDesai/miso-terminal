//! Ridge regression: least squares with a penalty on the coefficients, the
//! linear model beside the trees. Columns are standardised, a missing value
//! takes its column's training mean, and the penalty is picked from a few
//! values on the latest rows (the data is in time order), then the model is
//! refitted on all of them.

use crate::gbm::Matrix;

/// Penalties tried, as multiples of the number of rows.
const PENALTIES: [f64; 4] = [0.001, 0.01, 0.1, 1.0];

/// The share of rows, from the end, that picks the penalty.
const VALIDATION: f64 = 0.15;

#[derive(Clone, Debug, PartialEq)]
pub struct Ridge {
    means: Vec<f64>,
    scales: Vec<f64>,
    intercept: f64,
    coefficients: Vec<f64>,
}

impl Ridge {
    /// Fit to `x` (row-major, NaN for missing) and `y`, rows in time order.
    pub fn fit(x: &Matrix<'_>, y: &[f64]) -> Option<Self> {
        let n = x.rows();
        let p = x.features;
        if n != y.len() || n < 2 * (p + 2) {
            return None;
        }
        let cut = n - ((n as f64 * VALIDATION) as usize).max(1);
        let rows = |r: std::ops::Range<usize>| &x.values[r.start * p..r.end * p];
        let mut best: Option<(f64, f64)> = None;
        for lambda in PENALTIES {
            let Some(m) = solve(rows(0..cut), p, &y[..cut], lambda) else {
                continue;
            };
            let err: f64 = (cut..n)
                .map(|i| (y[i] - m.predict(&x.values[i * p..(i + 1) * p])).powi(2))
                .sum();
            if best.is_none_or(|(e, _)| err < e) {
                best = Some((err, lambda));
            }
        }
        solve(rows(0..n), p, y, best?.1)
    }

    pub fn predict(&self, row: &[f64]) -> f64 {
        self.intercept
            + row
                .iter()
                .zip(&self.means)
                .zip(&self.scales)
                .zip(&self.coefficients)
                .map(|(((v, m), s), b)| {
                    let v = if v.is_finite() { *v } else { *m };
                    b * (v - m) / s
                })
                .sum::<f64>()
    }
}

/// Fit with penalty `lambda` × rows on standardised columns.
fn solve(values: &[f64], p: usize, y: &[f64], lambda: f64) -> Option<Ridge> {
    let n = y.len();
    let nf = n as f64;
    let mut means = vec![0.0; p];
    let mut counts = vec![0usize; p];
    for row in values.chunks(p) {
        for (j, v) in row.iter().enumerate() {
            if v.is_finite() {
                means[j] += v;
                counts[j] += 1;
            }
        }
    }
    for (m, c) in means.iter_mut().zip(&counts) {
        *m /= (*c).max(1) as f64;
    }
    let z = |row: &[f64], j: usize| {
        if row[j].is_finite() {
            row[j] - means[j]
        } else {
            0.0
        }
    };
    let mut scales = vec![0.0; p];
    for row in values.chunks(p) {
        for (j, s) in scales.iter_mut().enumerate() {
            *s += z(row, j).powi(2);
        }
    }
    for s in &mut scales {
        *s = (*s / nf).sqrt();
        if *s < 1e-12 {
            *s = 1.0;
        }
    }
    let y_mean = y.iter().sum::<f64>() / nf;
    // Normal equations: (ZᵀZ + λnI) b = Zᵀ(y − ȳ).
    let mut a = vec![0.0; p * p];
    let mut rhs = vec![0.0; p];
    for (row, yi) in values.chunks(p).zip(y) {
        let zs: Vec<f64> = (0..p).map(|j| z(row, j) / scales[j]).collect();
        for j in 0..p {
            rhs[j] += zs[j] * (yi - y_mean);
            for k in 0..=j {
                a[j * p + k] += zs[j] * zs[k];
            }
        }
    }
    for j in 0..p {
        for k in 0..j {
            a[k * p + j] = a[j * p + k];
        }
        a[j * p + j] += lambda * nf;
    }
    let coefficients = cholesky_solve(&mut a, p, rhs)?;
    Some(Ridge {
        means,
        scales,
        intercept: y_mean,
        coefficients,
    })
}

/// Solve `a x = b` for a symmetric positive-definite `a` (overwritten).
fn cholesky_solve(a: &mut [f64], p: usize, mut b: Vec<f64>) -> Option<Vec<f64>> {
    for j in 0..p {
        let d = a[j * p + j] - (0..j).map(|k| a[j * p + k].powi(2)).sum::<f64>();
        if d <= 0.0 {
            return None;
        }
        let d = d.sqrt();
        a[j * p + j] = d;
        for i in j + 1..p {
            let s = a[i * p + j] - (0..j).map(|k| a[i * p + k] * a[j * p + k]).sum::<f64>();
            a[i * p + j] = s / d;
        }
    }
    // L y = b, then Lᵀ x = y.
    for i in 0..p {
        b[i] = (b[i] - (0..i).map(|k| a[i * p + k] * b[k]).sum::<f64>()) / a[i * p + i];
    }
    for i in (0..p).rev() {
        b[i] = (b[i] - (i + 1..p).map(|k| a[k * p + i] * b[k]).sum::<f64>()) / a[i * p + i];
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_a_plane() {
        // y = 3 + 2·x0 − x1, exactly; a tiny penalty barely moves it.
        let rows: Vec<[f64; 2]> = (0..200)
            .map(|i| [(i % 17) as f64, ((i * 7) % 23) as f64])
            .collect();
        let y: Vec<f64> = rows.iter().map(|r| 3.0 + 2.0 * r[0] - r[1]).collect();
        let values: Vec<f64> = rows.iter().flatten().copied().collect();
        let m = Ridge::fit(
            &Matrix {
                values: &values,
                features: 2,
            },
            &y,
        )
        .unwrap();
        assert!(
            (m.predict(&[10.0, 5.0]) - 18.0).abs() < 0.1,
            "{}",
            m.predict(&[10.0, 5.0])
        );
        // A missing value counts as its column's mean.
        let mean0 = rows.iter().map(|r| r[0]).sum::<f64>() / 200.0;
        assert!((m.predict(&[f64::NAN, 5.0]) - m.predict(&[mean0, 5.0])).abs() < 1e-9);
    }

    #[test]
    fn cholesky_by_hand() {
        // [[4, 2], [2, 3]] x = [2, 1]: x = [0.5, 0].
        let mut a = vec![4.0, 2.0, 2.0, 3.0];
        let x = cholesky_solve(&mut a, 2, vec![2.0, 1.0]).unwrap();
        assert!((x[0] - 0.5).abs() < 1e-12 && x[1].abs() < 1e-12);
        let mut singular = vec![1.0, 1.0, 1.0, 1.0];
        assert!(cholesky_solve(&mut singular, 2, vec![1.0, 1.0]).is_none());
    }

    #[test]
    fn constant_columns_and_short_data() {
        let values: Vec<f64> = (0..60).flat_map(|i| [1.0, i as f64]).collect();
        let y: Vec<f64> = (0..60).map(|i| 2.0 * i as f64).collect();
        let m = Ridge::fit(
            &Matrix {
                values: &values,
                features: 2,
            },
            &y,
        )
        .unwrap();
        assert!((m.predict(&[1.0, 30.0]) - 60.0).abs() < 1.0);
        assert!(
            Ridge::fit(
                &Matrix {
                    values: &values[..6],
                    features: 2
                },
                &y[..3]
            )
            .is_none()
        );
    }
}
