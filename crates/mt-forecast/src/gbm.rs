//! Gradient-boosted regression trees, small and pure Rust.
//!
//! Each feature is cut into at most [`BINS`] quantile bins once, so finding a
//! split is a pass over a histogram rather than a sort. Trees grow level by
//! level to a fixed depth on squared error, with L2 regularisation of the
//! leaves and a learning rate. A missing value (NaN) gets a bin of its own,
//! below every other, so a split also learns where missing values go.
//!
//! There is no hyperparameter search to run on a desktop: the parameters are
//! fixed at values that suit a few thousand to tens of thousands of hourly
//! rows, and the number of trees comes from early stopping on the latest
//! rows (the data is in time order, so they are the honest validation set),
//! after which the model is refitted on everything with that many trees.

/// Cut points per feature, at most; a value's bin is the number of cut
/// points below it, plus one (bin 0 is for missing values).
pub const BINS: usize = 64;

/// The fixed parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub learning_rate: f64,
    pub max_depth: usize,
    /// Rows a leaf must keep.
    pub min_leaf: usize,
    /// L2 penalty on leaf values.
    pub lambda: f64,
    pub max_trees: usize,
    /// Stop when this many trees in a row have not improved the validation
    /// error.
    pub patience: usize,
    /// The share of rows, from the end, held out to choose the tree count.
    pub validation: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            learning_rate: 0.05,
            max_depth: 5,
            min_leaf: 20,
            lambda: 1.0,
            max_trees: 600,
            patience: 40,
            validation: 0.15,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Split {
        feature: usize,
        /// Bins up to and including this one go left.
        bin: u8,
        left: usize,
        right: usize,
    },
    Leaf(f64),
}

#[derive(Clone, Debug, PartialEq)]
struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    fn predict(&self, bins: &[u8]) -> f64 {
        let mut i = 0;
        loop {
            match self.nodes[i] {
                Node::Split {
                    feature,
                    bin,
                    left,
                    right,
                } => i = if bins[feature] <= bin { left } else { right },
                Node::Leaf(v) => return v,
            }
        }
    }
}

/// A fitted model.
#[derive(Clone, Debug, PartialEq)]
pub struct Gbm {
    /// Per feature, the cut points between bins.
    edges: Vec<Vec<f64>>,
    base: f64,
    trees: Vec<Tree>,
}

/// Rows as a row-major matrix: `rows × features`.
pub struct Matrix<'a> {
    pub values: &'a [f64],
    pub features: usize,
}

impl Matrix<'_> {
    pub fn rows(&self) -> usize {
        self.values.len() / self.features.max(1)
    }

    fn row(&self, i: usize) -> &[f64] {
        &self.values[i * self.features..(i + 1) * self.features]
    }
}

fn cut_points(column: &[f64]) -> Vec<f64> {
    let mut v: Vec<f64> = column.iter().copied().filter(|x| x.is_finite()).collect();
    v.sort_by(f64::total_cmp);
    v.dedup();
    if v.len() <= BINS {
        // Midway between distinct values.
        return v.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();
    }
    let mut edges: Vec<f64> = (1..BINS).map(|k| v[k * v.len() / BINS]).collect();
    edges.dedup();
    edges
}

fn bin_of(edges: &[f64], x: f64) -> u8 {
    if x.is_nan() {
        0
    } else {
        // At most BINS − 1 cut points, so this fits in a u8 with room.
        (1 + edges.partition_point(|e| *e < x)) as u8
    }
}

impl Gbm {
    /// Fit to `x` (row-major, NaN for missing) and `y`, rows in time order.
    /// `None` without enough rows to grow a tree.
    pub fn fit(x: &Matrix<'_>, y: &[f64], params: &Params) -> Option<Self> {
        let n = x.rows();
        if n != y.len() || n < 4 * params.min_leaf || x.features == 0 {
            return None;
        }
        let edges: Vec<Vec<f64>> = (0..x.features)
            .map(|f| cut_points(&(0..n).map(|i| x.row(i)[f]).collect::<Vec<_>>()))
            .collect();
        let bins: Vec<u8> = (0..n)
            .flat_map(|i| {
                x.row(i)
                    .iter()
                    .zip(&edges)
                    .map(|(v, e)| bin_of(e, *v))
                    .collect::<Vec<_>>()
            })
            .collect();
        // Choose the number of trees on the latest rows, then refit on all.
        let cut = n - ((n as f64 * params.validation) as usize).max(1);
        let trees = if cut >= 4 * params.min_leaf {
            let probe = grow(
                &bins,
                x.features,
                &y[..cut],
                params,
                Some((&bins[cut * x.features..], &y[cut..])),
            );
            probe.len().max(1)
        } else {
            params.max_trees / 4
        };
        let base = mean(y);
        let all = grow(
            &bins,
            x.features,
            y,
            &Params {
                max_trees: trees,
                ..*params
            },
            None,
        );
        Some(Self {
            edges,
            base,
            trees: all,
        })
    }

    pub fn trees(&self) -> usize {
        self.trees.len()
    }

    /// The prediction for one row (NaN for missing values).
    pub fn predict(&self, row: &[f64]) -> f64 {
        let bins: Vec<u8> = row
            .iter()
            .zip(&self.edges)
            .map(|(v, e)| bin_of(e, *v))
            .collect();
        self.base + self.trees.iter().map(|t| t.predict(&bins)).sum::<f64>()
    }
}

fn mean(y: &[f64]) -> f64 {
    y.iter().sum::<f64>() / y.len().max(1) as f64
}

/// Grow trees on the first `y.len()` rows of `bins`. With a validation set,
/// stop once it has not improved for `patience` trees and return the trees
/// up to its best.
fn grow(
    bins: &[u8],
    features: usize,
    y: &[f64],
    params: &Params,
    validation: Option<(&[u8], &[f64])>,
) -> Vec<Tree> {
    let n = y.len();
    let base = mean(y);
    let mut pred = vec![base; n];
    let mut val_pred: Vec<f64> = validation.map_or_else(Vec::new, |(_, vy)| vec![base; vy.len()]);
    let mut trees = Vec::new();
    let (mut best_err, mut best_len) = (f64::INFINITY, 0);
    for _ in 0..params.max_trees {
        let residual: Vec<f64> = y.iter().zip(&pred).map(|(a, p)| a - p).collect();
        let tree = build_tree(bins, features, &residual, params);
        for (i, p) in pred.iter_mut().enumerate() {
            *p += tree.predict(&bins[i * features..(i + 1) * features]);
        }
        trees.push(tree);
        if let Some((vb, vy)) = validation {
            let t = trees.last().expect("just pushed");
            let mut err = 0.0;
            for (i, p) in val_pred.iter_mut().enumerate() {
                *p += t.predict(&vb[i * features..(i + 1) * features]);
                err += (vy[i] - *p).powi(2);
            }
            if err < best_err {
                (best_err, best_len) = (err, trees.len());
            } else if trees.len() - best_len >= params.patience {
                break;
            }
        }
    }
    if validation.is_some() {
        trees.truncate(best_len);
    }
    trees
}

/// One tree fitted to residuals, leaves already scaled by the learning rate.
fn build_tree(bins: &[u8], features: usize, residual: &[f64], params: &Params) -> Tree {
    let mut nodes = vec![Node::Leaf(0.0)];
    // (node index, its rows, depth)
    let mut open: Vec<(usize, Vec<usize>, usize)> = vec![(0, (0..residual.len()).collect(), 0)];
    while let Some((at, rows, depth)) = open.pop() {
        let sum: f64 = rows.iter().map(|&i| residual[i]).sum();
        let leaf = Node::Leaf(params.learning_rate * sum / (rows.len() as f64 + params.lambda));
        if depth >= params.max_depth || rows.len() < 2 * params.min_leaf {
            nodes[at] = leaf;
            continue;
        }
        let Some((feature, bin)) = best_split(bins, features, residual, &rows, sum, params) else {
            nodes[at] = leaf;
            continue;
        };
        let (l, r): (Vec<usize>, Vec<usize>) = rows
            .into_iter()
            .partition(|&i| bins[i * features + feature] <= bin);
        let (left, right) = (nodes.len(), nodes.len() + 1);
        nodes.push(Node::Leaf(0.0));
        nodes.push(Node::Leaf(0.0));
        nodes[at] = Node::Split {
            feature,
            bin,
            left,
            right,
        };
        open.push((left, l, depth + 1));
        open.push((right, r, depth + 1));
    }
    Tree { nodes }
}

/// The split with the largest gain in the regularised squared-error
/// objective, keeping `min_leaf` rows a side; `None` if nothing gains.
fn best_split(
    bins: &[u8],
    features: usize,
    residual: &[f64],
    rows: &[usize],
    sum: f64,
    params: &Params,
) -> Option<(usize, u8)> {
    let n = rows.len() as f64;
    let score = |g: f64, c: f64| g * g / (c + params.lambda);
    let parent = score(sum, n);
    let mut best: Option<(f64, usize, u8)> = None;
    let mut g = [0.0f64; BINS + 1];
    let mut c = [0.0f64; BINS + 1];
    for f in 0..features {
        g.fill(0.0);
        c.fill(0.0);
        for &i in rows {
            let b = bins[i * features + f] as usize;
            g[b] += residual[i];
            c[b] += 1.0;
        }
        let (mut gl, mut cl) = (0.0, 0.0);
        for b in 0..BINS {
            gl += g[b];
            cl += c[b];
            let cr = n - cl;
            if cl < params.min_leaf as f64 {
                continue;
            }
            if cr < params.min_leaf as f64 {
                break;
            }
            let gain = score(gl, cl) + score(sum - gl, cr) - parent;
            if gain > 1e-12 && best.is_none_or(|(bg, _, _)| gain > bg) {
                best = Some((gain, f, b as u8));
            }
        }
    }
    best.map(|(_, f, b)| (f, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(rows: &[Vec<f64>]) -> (Vec<f64>, usize) {
        (rows.iter().flatten().copied().collect(), rows[0].len())
    }

    #[test]
    fn learns_a_step_and_an_interaction() {
        // y = 10 when x0 > 0.5, else 0; plus 5 when x1 and x2 are both high.
        // x3 is noise.
        let mut rng = crate::TestRng(17);
        let rows: Vec<Vec<f64>> = (0..2000)
            .map(|_| (0..4).map(|_| rng.uniform()).collect())
            .collect();
        let y: Vec<f64> = rows
            .iter()
            .map(|r| {
                (if r[0] > 0.5 { 10.0 } else { 0.0 })
                    + if r[1] > 0.6 && r[2] > 0.6 { 5.0 } else { 0.0 }
            })
            .collect();
        let (values, features) = matrix(&rows);
        let m = Matrix {
            values: &values,
            features,
        };
        let gbm = Gbm::fit(&m, &y, &Params::default()).unwrap();
        assert!(gbm.trees() > 20, "{}", gbm.trees());
        let mae = rows
            .iter()
            .zip(&y)
            .map(|(r, t)| (gbm.predict(r) - t).abs())
            .sum::<f64>()
            / y.len() as f64;
        assert!(mae < 0.5, "{mae}");
        assert!((gbm.predict(&[0.9, 0.9, 0.9, 0.0]) - 15.0).abs() < 1.0);
        assert!(gbm.predict(&[0.1, 0.1, 0.9, 0.0]).abs() < 1.0);
    }

    #[test]
    fn missing_values_take_their_own_side() {
        // When x0 is missing, y is 100; otherwise y is x0.
        let rows: Vec<Vec<f64>> = (0..600)
            .map(|i| {
                vec![if i % 3 == 0 {
                    f64::NAN
                } else {
                    (i % 50) as f64
                }]
            })
            .collect();
        let y: Vec<f64> = rows
            .iter()
            .map(|r| if r[0].is_nan() { 100.0 } else { r[0] })
            .collect();
        let (values, features) = matrix(&rows);
        let gbm = Gbm::fit(
            &Matrix {
                values: &values,
                features,
            },
            &y,
            &Params::default(),
        )
        .unwrap();
        assert!((gbm.predict(&[f64::NAN]) - 100.0).abs() < 3.0);
        assert!((gbm.predict(&[25.0]) - 25.0).abs() < 4.0);
    }

    #[test]
    fn noise_stops_early_and_too_little_fits_nothing() {
        // Pure noise in y: the validation error never improves for long.
        let rows: Vec<Vec<f64>> = (0..1000).map(|i| vec![(i % 37) as f64]).collect();
        let y: Vec<f64> = (0..1000).map(|i| ((i * 7919) % 13) as f64).collect();
        let (values, features) = matrix(&rows);
        let gbm = Gbm::fit(
            &Matrix {
                values: &values,
                features,
            },
            &y,
            &Params::default(),
        )
        .unwrap();
        assert!(gbm.trees() < 200, "{}", gbm.trees());
        let (v, f) = matrix(&rows[..10]);
        assert!(
            Gbm::fit(
                &Matrix {
                    values: &v,
                    features: f
                },
                &y[..10],
                &Params::default()
            )
            .is_none()
        );
    }

    #[test]
    fn bins() {
        let e = cut_points(&[3.0, 1.0, 2.0, 2.0, f64::NAN]);
        assert_eq!(e, [1.5, 2.5]);
        assert_eq!(bin_of(&e, f64::NAN), 0);
        assert_eq!(bin_of(&e, 1.0), 1);
        assert_eq!(bin_of(&e, 2.0), 2);
        assert_eq!(bin_of(&e, 9.0), 3);
        let many: Vec<f64> = (0..10_000).map(f64::from).collect();
        assert!(cut_points(&many).len() < BINS);
    }
}
