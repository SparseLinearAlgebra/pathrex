use std::{cmp::Ordering, collections::HashMap};

use egg::{CostFunction, Id};

use super::{
    plan::RpqPlan,
    stats::{CountVector, LabelCountVectors},
};



#[derive(Clone, Debug, PartialEq)]
pub struct JoinCost {
    pub score: f64,
    pub nnz: f64,
    pub nnz_r: f64,
    pub nnz_c: f64,
}

impl Eq for JoinCost {}

impl PartialOrd for JoinCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for JoinCost {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.score.total_cmp(&other.score) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.nnz.total_cmp(&other.nnz) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.nnz_r.total_cmp(&other.nnz_r) {
            Ordering::Equal => {}
            ord => return ord,
        }
        self.nnz_c.total_cmp(&other.nnz_c)
    }
}

// Approach based on formula for SQL JOIN operation
// Got from https://github.com/chernishev/Database-Engines-Course/tree/master/Lecture%203
pub struct JoinCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
}

// TODO: enforce or encode `n > 0`; several estimates divide by `n` or `n^2`.
// TODO: decide whether all estimated cardinalities should be clamped to `[0, n^2]`.
impl CostFunction<RpqPlan> for JoinCostFn {
    type Cost = JoinCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::NamedVertex(_name) => JoinCost {
                score: 0.0,
                nnz: 1 as f64,
                nnz_r: 1 as f64,
                nnz_c: 1 as f64,
            },

            RpqPlan::Label(meta) => JoinCost {
                score: 0.0,
                nnz: meta.nvals as f64,
                nnz_r: meta.nonzero_rows as f64,
                nnz_c: meta.nonzero_cols as f64,
            },

            RpqPlan::Seq([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = (ca.nnz * cb.nnz) / denom;
                let score = ca.score + cb.score + op_cost;

                // TODO: this can exceed `n^2` when child estimates are already loose.
                let nnz_est = ca.nnz * cb.nnz / (self.n * self.n);

                JoinCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                // TODO: score uses the raw union estimate; decide if it should be clamped too.
                let overlap = (ca.nnz * cb.nnz) / (self.n * self.n);
                let op_cost = ca.nnz + cb.nnz - overlap;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = (ca.nnz + cb.nnz - overlap).min(self.n * self.n).max(0.0);

                let nnz_r_est = (ca.nnz_r + cb.nnz_r - (ca.nnz_r * cb.nnz_r) / self.n)
                    .min(self.n)
                    .max(0.0);

                let nnz_c_est = (ca.nnz_c + cb.nnz_c - (ca.nnz_c * cb.nnz_c) / self.n)
                    .min(self.n)
                    .max(0.0);

                JoinCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: nnz_r_est,
                    nnz_c: nnz_c_est,
                }
            }

            RpqPlan::Star([a]) => {
                let ca = costs(*a);

                // TODO: full dense closure is a conservative upper bound, not a tight estimate.
                let penalty = self.star_penalty * ca.nnz.max(1.0);
                let score = ca.score + penalty;

                JoinCost {
                    score,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                }
            }

            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                // TODO: LStar/RStar currently reuse Seq-like row/column estimates and do not
                // model the closure side directly.
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;
                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                JoinCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                // TODO: LStar/RStar currently reuse Seq-like row/column estimates and do not
                // model the closure side directly.
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;

                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                JoinCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }
        }
    }
}

// TODO: random cost fn for evaluating of accuracy of our solution
// pub struct _RandomCostFn;
// impl CostFunction<RpqPlan> for RandomCostFn {
//     type Cost = f64;
//     fn cost<C>(&mut self, _enode: &RpqPlan, _costs: C) -> Self::Cost
//     where
//         C: FnMut(Id) -> Self::Cost,
//     {
//         rand::random()
//     }
// }

#[derive(Clone, Debug)]
pub(super) struct MetaAcCost {
    score: f64,
    nnz: f64,
    nnz_r: f64,
    nnz_c: f64,
}

impl PartialEq for MetaAcCost {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for MetaAcCost {}
impl PartialOrd for MetaAcCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MetaAcCost {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then(self.nnz.total_cmp(&other.nnz))
            .then(self.nnz_r.total_cmp(&other.nnz_r))
            .then(self.nnz_c.total_cmp(&other.nnz_c))
    }
}

#[derive(Clone, Debug)]
pub(super) struct HybridCost {
    score: f64,
    nnz: f64,
    nnz_r: f64,
    nnz_c: f64,
}

impl PartialEq for HybridCost {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for HybridCost {}
impl PartialOrd for HybridCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HybridCost {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then(self.nnz.total_cmp(&other.nnz))
            .then(self.nnz_r.total_cmp(&other.nnz_r))
            .then(self.nnz_c.total_cmp(&other.nnz_c))
    }
}

// Naive metadata estimator
// got from 2.1 of paper https://mboehm7.github.io/resources/sigmod2019.pdf
pub(super) struct MetaAcCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
}

fn metaac_matmul_nnz(lhs_nnz: f64, rhs_nnz: f64, n: f64) -> f64 {
    let output_cells = (n * n).max(1.0);
    let p = ((lhs_nnz / output_cells) * (rhs_nnz / output_cells)).clamp(0.0, 1.0);
    (output_cells * (1.0 - (1.0 - p).powf(n))).clamp(0.0, output_cells)
}

// Due to Join cost fun estimates only join operation
// and don't even estimate nnz of result matrices, I combine
// it with metaac
pub(super) struct HybridCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
}

impl CostFunction<RpqPlan> for MetaAcCostFn {
    type Cost = MetaAcCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::NamedVertex(_name) => MetaAcCost {
                score: 0.0,
                nnz: 1.0,
                nnz_r: 1.0,
                nnz_c: 1.0,
            },
            RpqPlan::Label(meta) => MetaAcCost {
                score: 0.0,
                nnz: meta.nvals as f64,
                nnz_r: meta.nonzero_rows as f64,
                nnz_c: meta.nonzero_cols as f64,
            },
            RpqPlan::Seq([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let op_cost = ca.nnz * cb.nnz / self.n.max(1.0);
                let score = ca.score + cb.score + op_cost;
                let nnz_est = metaac_matmul_nnz(ca.nnz, cb.nnz, self.n);
                MetaAcCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                }
            }
            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let overlap = ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                let nnz_est = (ca.nnz + cb.nnz - overlap).clamp(0.0, self.n * self.n);
                let score = ca.score + cb.score + nnz_est;
                let nnz_r_est = (ca.nnz_r + cb.nnz_r - ca.nnz_r * cb.nnz_r / self.n.max(1.0))
                    .clamp(0.0, self.n);
                let nnz_c_est = (ca.nnz_c + cb.nnz_c - ca.nnz_c * cb.nnz_c / self.n.max(1.0))
                    .clamp(0.0, self.n);
                MetaAcCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: nnz_r_est,
                    nnz_c: nnz_c_est,
                }
            }
            RpqPlan::Star([a]) => {
                let ca = costs(*a);
                let penalty = self.star_penalty * ca.nnz.max(1.0);
                MetaAcCost {
                    score: ca.score + penalty,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                }
            }
            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = self.lr_multiplier * ca.nnz * cb.nnz / denom;
                let score = ca.score + cb.score + op_cost;
                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                MetaAcCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                }
            }
            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = self.lr_multiplier * ca.nnz * cb.nnz / denom;
                let score = ca.score + cb.score + op_cost;
                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                MetaAcCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                }
            }
        }
    }
}

impl CostFunction<RpqPlan> for HybridCostFn {
    type Cost = HybridCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::NamedVertex(_name) => HybridCost {
                score: 0.0,
                nnz: 1.0,
                nnz_r: 1.0,
                nnz_c: 1.0,
            },
            RpqPlan::Label(meta) => HybridCost {
                score: 0.0,
                nnz: meta.nvals as f64,
                nnz_r: meta.nonzero_rows as f64,
                nnz_c: meta.nonzero_cols as f64,
            },
            RpqPlan::Seq([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = ca.nnz * cb.nnz / denom;
                let score = ca.score + cb.score + op_cost;
                let universe = (self.n * self.n).max(1.0);
                let p = ((ca.nnz / universe) * (cb.nnz / universe)).clamp(0.0, 1.0);
                let nnz_est = (universe * (1.0 - (1.0 - p).powf(self.n))).clamp(0.0, universe);
                HybridCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                }
            }
            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let overlap = ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                let nnz_est = (ca.nnz + cb.nnz - overlap).clamp(0.0, self.n * self.n);
                let score = ca.score + cb.score + nnz_est;
                let nnz_r_est = (ca.nnz_r + cb.nnz_r - ca.nnz_r * cb.nnz_r / self.n.max(1.0))
                    .clamp(0.0, self.n);
                let nnz_c_est = (ca.nnz_c + cb.nnz_c - ca.nnz_c * cb.nnz_c / self.n.max(1.0))
                    .clamp(0.0, self.n);
                HybridCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: nnz_r_est,
                    nnz_c: nnz_c_est,
                }
            }
            RpqPlan::Star([a]) => {
                let ca = costs(*a);
                let penalty = self.star_penalty * ca.nnz.max(1.0);
                HybridCost {
                    score: ca.score + penalty,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                }
            }
            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = self.lr_multiplier * ca.nnz * cb.nnz / denom;
                let score = ca.score + cb.score + op_cost;
                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                HybridCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                }
            }
            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = self.lr_multiplier * ca.nnz * cb.nnz / denom;
                let score = ca.score + cb.score + op_cost;
                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                HybridCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct MncCost {
    score: f64,
    nnz: f64,
    nnz_r: f64,
    nnz_c: f64,
    row_counts: Option<CountVector>,
    col_counts: Option<CountVector>,
    row_extended: Option<CountVector>,
    col_extended: Option<CountVector>,
}

impl PartialEq for MncCost {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for MncCost {}
impl PartialOrd for MncCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MncCost {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then(self.nnz.total_cmp(&other.nnz))
            .then(self.nnz_r.total_cmp(&other.nnz_r))
            .then(self.nnz_c.total_cmp(&other.nnz_c))
    }
}

pub(super) struct MncCostFn {
    n: f64,
    star_penalty: f64,
    lr_multiplier: f64,
    labels: HashMap<String, LabelCountVectors>,
    vertices: HashMap<String, LabelCountVectors>,
    dot_cache: HashMap<(usize, usize), f64>,
    matmul_cache: HashMap<(usize, usize, usize, usize, Option<usize>, Option<usize>), f64>,
    scale_cache: HashMap<(usize, u64, u64), CountVector>,
    add_cache: HashMap<(usize, usize, u64, u64), CountVector>,
}

impl MncCostFn {
    pub(super) fn new(
        n: f64,
        labels: HashMap<String, LabelCountVectors>,
        vertices: HashMap<String, LabelCountVectors>,
    ) -> Self {
        Self {
            n,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
            labels,
            vertices,
            dot_cache: HashMap::new(),
            matmul_cache: HashMap::new(),
            scale_cache: HashMap::new(),
            add_cache: HashMap::new(),
        }
    }

    fn dot(&mut self, a: &CountVector, b: &CountVector) -> Option<f64> {
        let key = (a.cache_key(), b.cache_key());
        if let Some(value) = self.dot_cache.get(&key) {
            return Some(*value);
        }
        let value = a.dot(b)?;
        self.dot_cache.insert(key, value);
        Some(value)
    }

    fn matmul_nnz(&mut self, a: &MncCost, b: &MncCost) -> Option<f64> {
        let (ar, ac, br, bc) = (
            a.row_counts.as_ref()?,
            a.col_counts.as_ref()?,
            b.row_counts.as_ref()?,
            b.col_counts.as_ref()?,
        );
        let key = (
            ar.cache_key(),
            ac.cache_key(),
            br.cache_key(),
            bc.cache_key(),
            a.col_extended.as_ref().map(CountVector::cache_key),
            b.row_extended.as_ref().map(CountVector::cache_key),
        );
        if let Some(value) = self.matmul_cache.get(&key) {
            return Some(*value);
        }
        let value = CountVector::mnc_matmul_nnz(
            ar,
            ac,
            br,
            bc,
            a.col_extended.as_ref(),
            b.row_extended.as_ref(),
        )?;
        self.matmul_cache.insert(key, value);
        Some(value)
    }

    fn scale(&mut self, vector: &CountVector, factor: f64) -> Option<CountVector> {
        let key = (vector.cache_key(), factor.to_bits(), self.n.to_bits());
        if let Some(value) = self.scale_cache.get(&key) {
            return Some(value.clone());
        }
        let value = vector.scale(factor, self.n)?;
        self.scale_cache.insert(key, value.clone());
        Some(value)
    }

    fn add(&mut self, a: &CountVector, b: &CountVector, lambda: f64) -> Option<CountVector> {
        let (x, y) = (
            a.cache_key().min(b.cache_key()),
            a.cache_key().max(b.cache_key()),
        );
        let key = (x, y, lambda.to_bits(), self.n.to_bits());
        if let Some(value) = self.add_cache.get(&key) {
            return Some(value.clone());
        }
        let value = a.mnc_add(b, lambda, self.n)?;
        self.add_cache.insert(key, value.clone());
        Some(value)
    }

    fn seq(&mut self, a: MncCost, b: MncCost) -> MncCost {
        let op_cost = match (&a.col_counts, &b.row_counts) {
            (Some(ac), Some(br)) => self.dot(ac, br),
            _ => None,
        }
        .unwrap_or_else(|| a.nnz * b.nnz / self.n.max(1.0));
        let nnz = self
            .matmul_nnz(&a, &b)
            .unwrap_or_else(|| metaac_matmul_nnz(a.nnz, b.nnz, self.n));
        let row_counts = a
            .row_counts
            .as_ref()
            .and_then(|v| self.scale(v, nnz / a.nnz.max(1.0)));
        let col_counts = b
            .col_counts
            .as_ref()
            .and_then(|v| self.scale(v, nnz / b.nnz.max(1.0)));
        MncCost {
            score: a.score + b.score + op_cost,
            nnz,
            nnz_r: row_counts
                .as_ref()
                .map_or(a.nnz_r.min(self.n), CountVector::nonzero_count),
            nnz_c: col_counts
                .as_ref()
                .map_or(b.nnz_c.min(self.n), CountVector::nonzero_count),
            row_counts,
            col_counts,
            row_extended: None,
            col_extended: None,
        }
    }

    fn alt(&mut self, a: MncCost, b: MncCost) -> MncCost {
        let overlap = a.nnz * b.nnz / (self.n * self.n).max(1.0);
        let fallback_nnz = (a.nnz + b.nnz - overlap).clamp(0.0, self.n * self.n);
        let fallback_nnz_r =
            (a.nnz_r + b.nnz_r - a.nnz_r * b.nnz_r / self.n.max(1.0)).clamp(0.0, self.n);
        let fallback_nnz_c =
            (a.nnz_c + b.nnz_c - a.nnz_c * b.nnz_c / self.n.max(1.0)).clamp(0.0, self.n);
        let denominator = (a.nnz * b.nnz).max(1.0);
        let lambda_cols = match (&a.col_counts, &b.col_counts) {
            (Some(ac), Some(bc)) => self.dot(ac, bc).unwrap_or(0.0) / denominator,
            _ => 0.0,
        }
        .clamp(0.0, 1.0);
        let lambda_rows = match (&a.row_counts, &b.row_counts) {
            (Some(ar), Some(br)) => self.dot(ar, br).unwrap_or(0.0) / denominator,
            _ => 0.0,
        }
        .clamp(0.0, 1.0);
        let row_counts = match (&a.row_counts, &b.row_counts) {
            (Some(ar), Some(br)) => self.add(ar, br, lambda_cols),
            _ => None,
        };
        let col_counts = match (&a.col_counts, &b.col_counts) {
            (Some(ac), Some(bc)) => self.add(ac, bc, lambda_rows),
            _ => None,
        };
        let nnz = match (&row_counts, &col_counts) {
            (Some(rows), Some(cols)) => (rows.sum() + cols.sum()) / 2.0,
            (Some(rows), None) => rows.sum(),
            (None, Some(cols)) => cols.sum(),
            (None, None) => fallback_nnz,
        }
        .clamp(0.0, self.n * self.n);
        MncCost {
            score: a.score + b.score + nnz,
            nnz,
            nnz_r: row_counts
                .as_ref()
                .map_or(fallback_nnz_r, CountVector::nonzero_count),
            nnz_c: col_counts
                .as_ref()
                .map_or(fallback_nnz_c, CountVector::nonzero_count),
            row_counts,
            col_counts,
            row_extended: None,
            col_extended: None,
        }
    }
}

impl CostFunction<RpqPlan> for MncCostFn {
    type Cost = MncCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::NamedVertex(name) => {
                let counts = self.vertices.get(name);
                let row_counts = counts.map(|v| v.row_counts.clone());
                let col_counts = counts.map(|v| v.col_counts.clone());
                let row_extended = counts.map(|v| v.row_extended.clone());
                let col_extended = counts.map(|v| v.col_extended.clone());
                MncCost {
                    score: 0.0,
                    nnz: 1.0,
                    nnz_r: row_counts.as_ref().map_or(1.0, CountVector::nonzero_count),
                    nnz_c: col_counts.as_ref().map_or(1.0, CountVector::nonzero_count),
                    row_counts,
                    col_counts,
                    row_extended,
                    col_extended,
                }
            }
            RpqPlan::Label(meta) => {
                let counts = self.labels.get(&meta.name);
                let row_counts = counts.map(|v| v.row_counts.clone());
                let col_counts = counts.map(|v| v.col_counts.clone());
                let row_extended = counts.map(|v| v.row_extended.clone());
                let col_extended = counts.map(|v| v.col_extended.clone());
                MncCost {
                    score: 0.0,
                    nnz: meta.nvals as f64,
                    nnz_r: row_counts
                        .as_ref()
                        .map_or(meta.nonzero_rows as f64, CountVector::nonzero_count),
                    nnz_c: col_counts
                        .as_ref()
                        .map_or(meta.nonzero_cols as f64, CountVector::nonzero_count),
                    row_counts,
                    col_counts,
                    row_extended,
                    col_extended,
                }
            }
            RpqPlan::Seq([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                self.seq(ca, cb)
            }
            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                self.alt(ca, cb)
            }
            RpqPlan::Star([a]) => {
                let ca = costs(*a);
                let penalty = self.star_penalty * ca.nnz.max(1.0);
                MncCost {
                    score: ca.score + penalty,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                    row_counts: None,
                    col_counts: None,
                    row_extended: None,
                    col_extended: None,
                }
            }
            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = self.lr_multiplier * ca.nnz * cb.nnz / denom;
                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                MncCost {
                    score: ca.score + cb.score + op_cost,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                    row_counts: None,
                    col_counts: None,
                    row_extended: None,
                    col_extended: None,
                }
            }
            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);
                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = self.lr_multiplier * ca.nnz * cb.nnz / denom;
                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n).max(1.0);
                MncCost {
                    score: ca.score + cb.score + op_cost,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n),
                    nnz_c: cb.nnz_c.min(self.n),
                    row_counts: None,
                    col_counts: None,
                    row_extended: None,
                    col_extended: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egg::RecExpr;

    use crate::rpq::rpqmatrix::{
        optimize::optimize_expr_join,
        plan::{LabelMeta, RpqPlan},
    };
    use crate::{graph::GraphDecomposition, utils::build_graph};

    use super::*;

    #[test]
    fn independent_costs_preserve_seq_estimates() {
        let a = Id::from(0);
        let b = Id::from(1);
        let left = RpqPlan::Label(LabelMeta {
            name: "left".to_string(),
            nvals: 10,
            nonzero_rows: 2,
            nonzero_cols: 4,
        });
        let right = RpqPlan::Label(LabelMeta {
            name: "right".to_string(),
            nvals: 20,
            nonzero_rows: 5,
            nonzero_cols: 6,
        });
        let seq = RpqPlan::Seq([a, b]);

        let mut metaac = MetaAcCostFn {
            n: 10.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let ma = metaac.cost(&left, |_| unreachable!());
        let mb = metaac.cost(&right, |_| unreachable!());
        let meta_cost = metaac.cost(&seq, |id| if id == a { ma.clone() } else { mb.clone() });
        let expected_nnz = 100.0 * (1.0 - (1.0_f64 - 0.02).powf(10.0));
        assert!((meta_cost.nnz - expected_nnz).abs() < 1e-10);
        assert_eq!(meta_cost.score, 20.0);
        assert_eq!((meta_cost.nnz_r, meta_cost.nnz_c), (2.0, 6.0));

        let mut hybrid = HybridCostFn {
            n: 10.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let ha = hybrid.cost(&left, |_| unreachable!());
        let hb = hybrid.cost(&right, |_| unreachable!());
        let hybrid_cost = hybrid.cost(&seq, |id| if id == a { ha.clone() } else { hb.clone() });
        assert!((hybrid_cost.nnz - expected_nnz).abs() < 1e-10);
        assert!((hybrid_cost.score - 200.0 / 6.0).abs() < 1e-10);

        let mut mnc = MncCostFn::new(10.0, HashMap::new(), HashMap::new());
        let ca = mnc.cost(&left, |_| unreachable!());
        let cb = mnc.cost(&right, |_| unreachable!());
        let cost = mnc.cost(&seq, |id| if id == a { ca.clone() } else { cb.clone() });
        assert_eq!(cost.score, meta_cost.score);
        assert!((cost.nnz - meta_cost.nnz).abs() < 1e-10);
        assert_eq!((cost.nnz_r, cost.nnz_c), (2.0, 6.0));
    }

    #[test]
    fn mnc_seq_uses_leaf_extensions_without_propagating_them() {
        let graph = build_graph(&[
            ("u", "a", "A"),
            ("v", "b", "A"),
            ("v", "c", "A"),
            ("a", "x", "B"),
            ("b", "x", "B"),
            ("c", "y", "B"),
        ]);
        let labels = ["A", "B"]
            .into_iter()
            .map(|name| {
                (
                    name.to_string(),
                    LabelCountVectors::from_matrix(graph.get_graph(name).unwrap().matrix())
                        .unwrap(),
                )
            })
            .collect();
        let mut mnc = MncCostFn::new(7.0, labels, HashMap::new());
        let a = mnc.cost(
            &RpqPlan::Label(LabelMeta {
                name: "A".to_string(),
                nvals: 3,
                nonzero_rows: 2,
                nonzero_cols: 3,
            }),
            |_| unreachable!(),
        );
        let b = mnc.cost(
            &RpqPlan::Label(LabelMeta {
                name: "B".to_string(),
                nvals: 3,
                nonzero_rows: 3,
                nonzero_cols: 2,
            }),
            |_| unreachable!(),
        );
        let result = mnc.seq(a, b);
        assert_eq!(result.nnz, 3.0);
        assert!(result.row_extended.is_none());
        assert!(result.col_extended.is_none());
    }

    fn assert_finite_nonnegative(cost: &JoinCost) {
        assert!(cost.score.is_finite(), "score must be finite: {cost:?}");
        assert!(cost.nnz.is_finite(), "nnz must be finite: {cost:?}");
        assert!(cost.nnz_r.is_finite(), "nnz_r must be finite: {cost:?}");
        assert!(cost.nnz_c.is_finite(), "nnz_c must be finite: {cost:?}");

        assert!(cost.score >= 0.0, "score must be non-negative: {cost:?}");
        assert!(cost.nnz >= 0.0, "nnz must be non-negative: {cost:?}");
        assert!(cost.nnz_r >= 0.0, "nnz_r must be non-negative: {cost:?}");
        assert!(cost.nnz_c >= 0.0, "nnz_c must be non-negative: {cost:?}");
    }

    fn child_cost(id: Id, a: Id, ca: &JoinCost, b: Id, cb: &JoinCost) -> JoinCost {
        if id == a {
            ca.clone()
        } else if id == b {
            cb.clone()
        } else {
            panic!("unexpected child id: {id:?}")
        }
    }

    fn unary_child_cost(id: Id, child: Id, cost: &JoinCost) -> JoinCost {
        if id == child {
            cost.clone()
        } else {
            panic!("unexpected child id: {id:?}")
        }
    }

    #[test]
    fn card_cost_order_uses_score_then_nnz_then_rows_then_cols() {
        let base = JoinCost {
            score: 10.0,
            nnz: 20.0,
            nnz_r: 30.0,
            nnz_c: 40.0,
        };

        assert!(
            JoinCost {
                score: 9.0,
                nnz: 100.0,
                nnz_r: 100.0,
                nnz_c: 100.0,
            } < base
        );
        assert!(
            JoinCost {
                score: 10.0,
                nnz: 19.0,
                nnz_r: 100.0,
                nnz_c: 100.0,
            } < base
        );
        assert!(
            JoinCost {
                score: 10.0,
                nnz: 20.0,
                nnz_r: 29.0,
                nnz_c: 100.0,
            } < base
        );
        assert!(
            JoinCost {
                score: 10.0,
                nnz: 20.0,
                nnz_r: 30.0,
                nnz_c: 39.0,
            } < base
        );
    }

    #[test]
    fn join_cost_base_nodes_use_vertex_and_label_metadata() {
        let mut cost_fn = JoinCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };

        let named = cost_fn.cost(&RpqPlan::NamedVertex("A".to_string()), |_| {
            panic!("NamedVertex must not request child costs")
        });
        assert_eq!(
            named,
            JoinCost {
                score: 0.0,
                nnz: 1.0,
                nnz_r: 1.0,
                nnz_c: 1.0,
            }
        );

        let label = cost_fn.cost(
            &RpqPlan::Label(LabelMeta {
                name: "knows".to_string(),
                nvals: 17,
                nonzero_rows: 5,
                nonzero_cols: 9,
            }),
            |_| panic!("Label must not request child costs"),
        );
        assert_eq!(
            label,
            JoinCost {
                score: 0.0,
                nnz: 17.0,
                nnz_r: 5.0,
                nnz_c: 9.0,
            }
        );
    }
    #[test]
    fn join_cost_seq_correctness() {
        let mut cost_fn = JoinCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);

        let ca = JoinCost {
            score: 2.0,
            nnz: 20.0,
            nnz_r: 4.0,
            nnz_c: 8.0,
        };

        let cb = JoinCost {
            score: 3.0,
            nnz: 30.0,
            nnz_r: 6.0,
            nnz_c: 10.0,
        };

        let seq = cost_fn.cost(&RpqPlan::Seq([a, b]), |id| {
            if id == a {
                ca.clone()
            } else if id == b {
                cb.clone()
            } else {
                panic!("unexpected child id: {id:?}")
            }
        });
        assert_eq!(
            seq,
            JoinCost {
                score: 65.0,
                nnz: 0.06,
                nnz_r: 4.0,
                nnz_c: 10.0,
            }
        );
    }
    #[test]
    fn join_cost_seq_correctness_zero_denom() {
        let mut cost_fn = JoinCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);

        let ca = JoinCost {
            score: 2.0,
            nnz: 20.0,
            nnz_r: 0.0,
            nnz_c: 4.0,
        };

        let cb = JoinCost {
            score: 3.0,
            nnz: 30.0,
            nnz_r: 4.0,
            nnz_c: 0.0,
        };

        let seq = cost_fn.cost(&RpqPlan::Seq([a, b]), |id| {
            if id == a {
                ca.clone()
            } else if id == b {
                cb.clone()
            } else {
                panic!("unexpected child id: {id:?}")
            }
        });
        assert_eq!(
            seq,
            JoinCost {
                score: 605.0,
                nnz: 0.06,
                nnz_r: 0.0,
                nnz_c: 0.0,
            }
        );
    }
    #[test]
    fn join_cost_alt_with_zero_children_stays_finite_nonnegative() {
        let mut cost_fn = JoinCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);
        let ca = JoinCost {
            score: 0.0,
            nnz: 0.0,
            nnz_r: 0.0,
            nnz_c: 0.0,
        };
        let cb = JoinCost {
            score: 3.0,
            nnz: 30.0,
            nnz_r: 0.0,
            nnz_c: 10.0,
        };

        let alt = cost_fn.cost(&RpqPlan::Alt([a, b]), |id| child_cost(id, a, &ca, b, &cb));

        assert_finite_nonnegative(&alt);
        assert_eq!(
            alt,
            JoinCost {
                score: 33.0,
                nnz: 30.0,
                nnz_r: 0.0,
                nnz_c: 10.0,
            }
        );
    }

    #[test]
    fn join_cost_star_with_zero_nnz_uses_min_penalty() {
        let mut cost_fn = JoinCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let ca = JoinCost {
            score: 7.0,
            nnz: 0.0,
            nnz_r: 0.0,
            nnz_c: 0.0,
        };

        let star = cost_fn.cost(&RpqPlan::Star([a]), |id| unary_child_cost(id, a, &ca));

        assert_finite_nonnegative(&star);
        assert_eq!(
            star,
            JoinCost {
                score: 57.0,
                nnz: 10_000.0,
                nnz_r: 100.0,
                nnz_c: 100.0,
            }
        );
    }

    #[test]
    fn join_cost_lstar_and_rstar_zero_denom_stay_finite_nonnegative() {
        let mut cost_fn = JoinCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);
        let ca = JoinCost {
            score: 2.0,
            nnz: 20.0,
            nnz_r: 0.0,
            nnz_c: 4.0,
        };
        let cb = JoinCost {
            score: 3.0,
            nnz: 30.0,
            nnz_r: 4.0,
            nnz_c: 0.0,
        };

        let lstar = cost_fn.cost(&RpqPlan::LStar([a, b]), |id| child_cost(id, a, &ca, b, &cb));
        let rstar = cost_fn.cost(&RpqPlan::RStar([a, b]), |id| child_cost(id, a, &ca, b, &cb));

        assert_finite_nonnegative(&lstar);
        assert_finite_nonnegative(&rstar);
        let expected = JoinCost {
            score: 3005.0,
            nnz: 0.3,
            nnz_r: 0.0,
            nnz_c: 0.0,
        };

        assert_eq!(lstar, expected);
        assert_eq!(rstar, expected);
    }
    #[test]
    fn join_cost_build_lstar() {
        let mut expr = RecExpr::default();
        let a = expr.add(RpqPlan::Label(LabelMeta {
            name: "knows".to_string(),
            nvals: 17,
            nonzero_rows: 5,
            nonzero_cols: 9,
        }));
        let b = expr.add(RpqPlan::Label(LabelMeta {
            name: "knows".to_string(),
            nvals: 17,
            nonzero_rows: 5,
            nonzero_cols: 9,
        }));
        let star = expr.add(RpqPlan::Star([a]));
        let _seq = expr.add(RpqPlan::Seq([star, b]));
        let opt = optimize_expr_join(expr, 100);
        let root = opt.as_ref().last().expect("optimized expr is non-empty");

        assert!(matches!(root, RpqPlan::LStar(_)));
    }
    #[test]
    fn join_cost_build_rstar() {
        let mut expr = RecExpr::default();
        let a = expr.add(RpqPlan::Label(LabelMeta {
            name: "knows".to_string(),
            nvals: 17,
            nonzero_rows: 5,
            nonzero_cols: 9,
        }));
        let b = expr.add(RpqPlan::Label(LabelMeta {
            name: "knows".to_string(),
            nvals: 17,
            nonzero_rows: 5,
            nonzero_cols: 9,
        }));
        let star = expr.add(RpqPlan::Star([b]));
        let _seq = expr.add(RpqPlan::Seq([a, star]));
        let opt = optimize_expr_join(expr, 100);
        let root = opt.as_ref().last().expect("optimized expr is non-empty");

        assert!(matches!(root, RpqPlan::RStar(_)));
    }
    //TODO: maybe cover other rules
}
