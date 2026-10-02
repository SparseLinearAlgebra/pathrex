use std::{cmp::Ordering, collections::HashMap, sync::Arc};

use egg::{CostFunction, Id};

use super::{
    plan::{LabelMeta, RpqPlan},
    sampling::{MatrixSampler, SampledRelation, SamplingConfig},
    stats::{CountVector, LabelCountVectors},
};
use crate::graph::LagraphGraph;

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
                nnz: 1.0,
                nnz_r: 1.0,
                nnz_c: 1.0,
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
                let nnz_est = ca.nnz * cb.nnz / self.n;

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
    (-output_cells * (n * (-p).ln_1p()).exp_m1()).clamp(0.0, output_cells)
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
            RpqPlan::LStar([a, b]) | RpqPlan::RStar([a, b]) => {
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

// Join operation work with MetaAC result cardinality.
pub(super) struct HybridCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
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
                let nnz_est = metaac_matmul_nnz(ca.nnz, cb.nnz, self.n);
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
            RpqPlan::LStar([a, b]) | RpqPlan::RStar([a, b]) => {
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

type MncMatmulKey = (usize, usize, usize, usize, Option<usize>, Option<usize>);

pub(super) struct MncCostFn {
    n: f64,
    star_penalty: f64,
    lr_multiplier: f64,
    labels: HashMap<String, LabelCountVectors>,
    vertices: HashMap<String, LabelCountVectors>,
    dot_cache: HashMap<(usize, usize), f64>,
    matmul_cache: HashMap<MncMatmulKey, f64>,
    scale_cache: HashMap<(usize, u64, u64), CountVector>,
    add_cache: HashMap<(usize, usize, u64, u64), CountVector>,
}

impl MncCostFn {
    pub(super) fn new(n: f64, labels: HashMap<String, LabelCountVectors>, vertices: HashMap<String, LabelCountVectors>) -> Self {
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
                let row_extended = counts.and_then(|v| v.row_extended.clone());
                let col_extended = counts.and_then(|v| v.col_extended.clone());
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
                let row_extended = counts.and_then(|v| v.row_extended.clone());
                let col_extended = counts.and_then(|v| v.col_extended.clone());
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
            RpqPlan::LStar([a, b]) | RpqPlan::RStar([a, b]) => {
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

#[derive(Clone, Debug)]
struct Estimate {
    score: f64,
    nnz: f64,
    rows: f64,
    cols: f64,
}

impl PartialEq for Estimate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Estimate {}

impl PartialOrd for Estimate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Estimate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then(self.nnz.total_cmp(&other.nnz))
            .then(self.rows.total_cmp(&other.rows))
            .then(self.cols.total_cmp(&other.cols))
    }
}

#[derive(Clone, Debug)]
pub(super) struct PangHybridEstimate {
    estimate: Estimate,
    identity: bool,
}
impl PartialEq for PangHybridEstimate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for PangHybridEstimate {}
impl PartialOrd for PangHybridEstimate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for PangHybridEstimate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.estimate.cmp(&other.estimate)
    }
}

pub(super) struct PangHybridCostFn {
    n: f64,
}

impl PangHybridCostFn {
    pub fn new(n: f64) -> Self {
        Self { n }
    }
    fn label(&self, meta: &LabelMeta) -> PangHybridEstimate {
        PangHybridEstimate {
            estimate: Estimate {
                score: 0.0,
                nnz: meta.nvals as f64,
                rows: meta.nonzero_rows as f64,
                cols: meta.nonzero_cols as f64,
            },
            identity: false,
        }
    }
    fn vertex(&self) -> PangHybridEstimate {
        PangHybridEstimate {
            estimate: Estimate {
                score: 0.0,
                nnz: 1.0,
                rows: 1.0,
                cols: 1.0,
            },
            identity: false,
        }
    }
    fn product(&self, a: &PangHybridEstimate, b: &PangHybridEstimate, join_denominator: f64) -> (PangHybridEstimate, f64) {
        let work = a.estimate.nnz * b.estimate.nnz / a.estimate.rows.max(b.estimate.cols).max(1.0);
        // Eq. 10 with an effective J. Seq passes Join's denominator;
        // closure passes n to approximate the support intersection.
        let join = a.estimate.cols * b.estimate.rows / join_denominator;
        let (nnz, rows, cols) = if join > 0.0 {
            let pairs =
                join * (a.estimate.nnz / a.estimate.cols) * (b.estimate.nnz / b.estimate.rows);
            let nnz = pairs
                .min(a.estimate.rows * b.estimate.cols)
                .min(self.n * self.n);
            (
                nnz,
                (a.estimate.rows * join / a.estimate.cols)
                    .min(a.estimate.rows)
                    .min(nnz),
                (b.estimate.cols * join / b.estimate.rows)
                    .min(b.estimate.cols)
                    .min(nnz),
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        let mut out = PangHybridEstimate {
            estimate: Estimate {
                score: 0.0,
                nnz,
                rows,
                cols,
            },
            identity: false,
        };
        if a.identity {
            out = b.clone()
        } else if b.identity {
            out = a.clone()
        }
        out.estimate.score = 0.0;
        (out, work)
    }
    fn alternate(&self, a: &PangHybridEstimate, b: &PangHybridEstimate) -> PangHybridEstimate {
        if a.estimate.nnz == 0.0 {
            return b.clone();
        }
        if b.estimate.nnz == 0.0 {
            return a.clone();
        }
        if a.identity && b.identity {
            let mut out = a.clone();
            out.estimate.score = 0.0;
            return out;
        }
        let n = self.n;
        let overlap = a.estimate.nnz * b.estimate.nnz / (n * n).max(1.0);
        let estimate = Estimate {
            score: 0.0,
            nnz: (a.estimate.nnz + b.estimate.nnz - overlap).clamp(0.0, n * n),
            rows: (a.estimate.rows + b.estimate.rows
                - a.estimate.rows * b.estimate.rows / n.max(1.0))
            .clamp(0.0, n),
            cols: (a.estimate.cols + b.estimate.cols
                - a.estimate.cols * b.estimate.cols / n.max(1.0))
            .clamp(0.0, n),
        };
        PangHybridEstimate {
            estimate,
            identity: false,
        }
    }
    fn closure_steps(&self, body: &PangHybridEstimate, initial: f64) -> usize {
        if body.estimate.nnz == 0.0 || initial == 0.0 {
            return 1;
        }
        // Eq. 24 uses a separate support-intersection estimate. Using the
        // Join-work proxy here would make growth >= 1 for every nonempty R.
        let growth = body.estimate.nnz / self.n.max(1.0);
        if growth == 0.0 {
            return 1;
        }
        if growth >= 1.0 {
            6
        } else {
            (-initial.max(1.0).ln() / growth.ln())
                .ceil()
                .clamp(1.0, 64.0) as usize
        }
    }
    fn closure(&mut self, body: PangHybridEstimate, seed: PangHybridEstimate, left: bool, single: bool) -> PangHybridEstimate {
        if body.identity {
            if single {
                let mut out = body;
                out.estimate.score += self.n;
                return out;
            }
            let mut out = seed;
            out.estimate.score += body.estimate.score + body.estimate.nnz + out.estimate.nnz;
            return out;
        }
        let initial = if single {
            seed.estimate.nnz
        } else if left {
            self.product(&body, &seed, self.n.max(1.0)).0.estimate.nnz
        } else {
            self.product(&seed, &body, self.n.max(1.0)).0.estimate.nnz
        };
        let steps = self.closure_steps(&body, initial);
        let mut result = seed.clone();
        let mut power = seed.clone();
        let mut score = body.estimate.score + if single { 0.0 } else { seed.estimate.score };
        if single || (body.estimate.nnz > 0.0 && seed.estimate.nnz > 0.0) {
            let products = steps - usize::from(single);
            for _ in 0..products {
                let (product, work) = if left {
                    self.product(&body, &result, self.n.max(1.0))
                } else {
                    self.product(&result, &body, self.n.max(1.0))
                };
                let (next_power, _) = if left {
                    self.product(&body, &power, self.n.max(1.0))
                } else {
                    self.product(&power, &body, self.n.max(1.0))
                };
                let mut next = if next_power.estimate.nnz < 1.0 {
                    result.clone()
                } else {
                    self.alternate(&result, &next_power)
                };
                if single || !left {
                    next.estimate.rows = seed.estimate.rows;
                }
                if single || left {
                    next.estimate.cols = seed.estimate.cols;
                }
                next.estimate.nnz = next
                    .estimate
                    .nnz
                    .min(next.estimate.rows * next.estimate.cols);
                score += work + result.estimate.nnz + product.estimate.nnz + next.estimate.nnz;
                result = next;
                power = next_power;
                if power.estimate.nnz < 1.0
                    || result.estimate.nnz >= result.estimate.rows * result.estimate.cols
                {
                    break;
                }
            }
        }
        if single {
            let plus = result.estimate.nnz;
            result.estimate.nnz = (plus + self.n).min(self.n * self.n);
            score += self.n + plus + result.estimate.nnz;
            result.estimate.rows = self.n;
            result.estimate.cols = self.n;
            result.identity = body.estimate.nnz == 0.0
        }
        result.estimate.score = score;
        result
    }
}

impl CostFunction<RpqPlan> for PangHybridCostFn {
    type Cost = PangHybridEstimate;
    fn cost<C: FnMut(Id) -> PangHybridEstimate>(&mut self, node: &RpqPlan, mut costs: C) -> PangHybridEstimate {
        match node {
            RpqPlan::Label(meta) => self.label(meta),
            RpqPlan::NamedVertex(_) => self.vertex(),
            RpqPlan::Seq([a, b]) => {
                let (a, b) = (costs(*a), costs(*b));
                let denominator = a.estimate.rows.max(b.estimate.cols).max(1.0);
                let (mut out, work) = self.product(&a, &b, denominator);
                out.estimate.score = a.estimate.score + b.estimate.score + work;
                out
            }
            RpqPlan::Alt([a, b]) => {
                let (a, b) = (costs(*a), costs(*b));
                let mut out = self.alternate(&a, &b);
                out.estimate.score = a.estimate.score + b.estimate.score + out.estimate.nnz;
                out
            }
            RpqPlan::Star([a]) => {
                let a = costs(*a);
                self.closure(a.clone(), a, false, true)
            }
            RpqPlan::LStar([a, b]) => self.closure(costs(*a), costs(*b), true, false),
            RpqPlan::RStar([a, b]) => self.closure(costs(*b), costs(*a), false, false),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct SamplingEstimate {
    base: HybridCost,
    sample: Option<SampledRelation>,
}
impl PartialEq for SamplingEstimate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for SamplingEstimate {}
impl PartialOrd for SamplingEstimate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SamplingEstimate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.base.cmp(&other.base)
    }
}

pub(super) struct SamplingCostFn {
    // Provides operation costs and a fallback when the sampled relation is unusable.
    baseline: HybridCostFn,
    sampler: MatrixSampler,
    label_ids: HashMap<String, usize>,
    vertices: HashMap<String, usize>,
}
impl SamplingCostFn {
    pub fn new(n: f64, mut graphs: Vec<(String, Arc<LagraphGraph>)>, vertices: HashMap<String, usize>, config: SamplingConfig) -> Self {
        graphs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let label_ids = graphs.iter().enumerate().map(|(id, (name, _))| (name.clone(), id)).collect();
        let matrices = graphs.iter().map(|(_, graph)| graph.matrix()).collect();
        let fixed_vertices = vertices.values().copied().collect::<Vec<_>>();
        Self {
            baseline: HybridCostFn {
                n,
                star_penalty: 50.0,
                lr_multiplier: 5.0,
            },
            sampler: MatrixSampler::new(n as usize, matrices, &fixed_vertices, config),
            label_ids,
            vertices,
        }
    }

    fn sampled(&self, mut base: HybridCost, sample: Option<SampledRelation>, output_charged: bool) -> SamplingEstimate {
        let old = base.nnz;
        let estimate = self.sampler.estimate(sample.as_ref());
        if estimate.converged && (estimate.exact || estimate.nnz > 0.0) {
            base.nnz = estimate.nnz.min(estimate.rows * estimate.cols);
            base.nnz_r = estimate.rows.min(base.nnz);
            base.nnz_c = estimate.cols.min(base.nnz);
            if output_charged {
                base.score = (base.score + base.nnz - old).max(0.0);
            }
        }
        SamplingEstimate { base, sample }
    }
}
impl CostFunction<RpqPlan> for SamplingCostFn {
    type Cost = SamplingEstimate;
    fn cost<C: FnMut(Id) -> SamplingEstimate>(&mut self, node: &RpqPlan, mut costs: C) -> SamplingEstimate {
        match node {
            RpqPlan::Label(meta) => SamplingEstimate {
                base: self.baseline.cost(node, |_| unreachable!()),
                sample: self.sampler.label(self.label_ids[&meta.name]),
            },
            RpqPlan::NamedVertex(name) => SamplingEstimate {
                base: self.baseline.cost(node, |_| unreachable!()),
                sample: self.sampler.vertex(self.vertices[name]),
            },
            RpqPlan::Seq([a, b]) => {
                let (left, right) = (costs(*a), costs(*b));
                let base = self.baseline.cost(node, |id| {
                    if id == *a {
                        left.base.clone()
                    } else {
                        right.base.clone()
                    }
                });
                let sample = self.sampler.seq(left.sample.as_ref(), right.sample.as_ref());
                self.sampled(base, sample, false)
            }
            RpqPlan::Alt([a, b]) => {
                let (left, right) = (costs(*a), costs(*b));
                let base = self.baseline.cost(node, |id| {
                    if id == *a {
                        left.base.clone()
                    } else {
                        right.base.clone()
                    }
                });
                let sample = self.sampler.alt(left.sample.as_ref(), right.sample.as_ref());
                self.sampled(base, sample, true)
            }
            RpqPlan::LStar([a, b]) => {
                let (left, right) = (costs(*a), costs(*b));
                let base = self.baseline.cost(node, |id| {
                    if id == *a {
                        left.base.clone()
                    } else {
                        right.base.clone()
                    }
                });
                let sample = self.sampler.closure(left.sample.as_ref(), right.sample.as_ref(), true);
                self.sampled(base, sample, false)
            }
            RpqPlan::RStar([a, b]) => {
                let (left, right) = (costs(*a), costs(*b));
                let base = self.baseline.cost(node, |id| {
                    if id == *a {
                        left.base.clone()
                    } else {
                        right.base.clone()
                    }
                });
                let sample = self.sampler.closure(right.sample.as_ref(), left.sample.as_ref(), false);
                self.sampled(base, sample, false)
            }
            RpqPlan::Star([a]) => {
                let child = costs(*a);
                let base = self.baseline.cost(node, |_| child.base.clone());
                let sample = self.sampler.star(child.sample.as_ref());
                self.sampled(base, sample, false)
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
    fn sampling_estimates_composed_relations() {
        let graph = build_graph(&[("a", "b", "p"), ("b", "c", "p"), ("c", "d", "q")]);
        let p = RpqPlan::Label(LabelMeta {
            name: "p".to_string(),
            nvals: 2,
            nonzero_rows: 2,
            nonzero_cols: 2,
        });
        let seq = RpqPlan::Seq([Id::from(0), Id::from(0)]);
        let star = RpqPlan::Star([Id::from(0)]);
        let q = RpqPlan::Label(LabelMeta {
            name: "q".to_string(),
            nvals: 1,
            nonzero_rows: 1,
            nonzero_cols: 1,
        });
        let left_star = RpqPlan::LStar([Id::from(0), Id::from(1)]);
        let right_star = RpqPlan::RStar([Id::from(1), Id::from(0)]);
        let matrices = ["p", "q"]
            .into_iter()
            .map(|name| (name.to_string(), graph.get_graph(name).unwrap()))
            .collect::<Vec<_>>();
        let config = SamplingConfig {
            percent: 100,
            seed: 0,
            max_star_iterations: 4,
        };
        let mut sampled = SamplingCostFn::new(4.0, matrices, HashMap::new(), config);
        let leaf = sampled.cost(&p, |_| unreachable!());
        assert_eq!(sampled.cost(&seq, |_| leaf.clone()).base.nnz, 1.0);
        assert_eq!(sampled.cost(&star, |_| leaf.clone()).base.nnz, 7.0);
        let q_leaf = sampled.cost(&q, |_| unreachable!());
        let alt = RpqPlan::Alt([Id::from(0), Id::from(1)]);
        let union = sampled.cost(&alt, |id| if id == Id::from(0) { leaf.clone() } else { q_leaf.clone() });
        assert_eq!(union.base.nnz, 3.0);
        assert_eq!(union.base.score, 3.0);
        let child = |id| {
            if id == Id::from(0) {
                leaf.clone()
            } else {
                q_leaf.clone()
            }
        };
        assert_eq!(sampled.cost(&left_star, child).base.nnz, 3.0);
        let child = |id| {
            if id == Id::from(0) {
                leaf.clone()
            } else {
                q_leaf.clone()
            }
        };
        assert_eq!(sampled.cost(&right_star, child).base.nnz, 1.0);
    }

    #[test]
    fn metaac_product_retains_small_estimates_on_large_graphs() {
        let estimate = metaac_matmul_nnz(1_000.0, 1_000.0, 100_000_000.0);
        assert!((estimate - 0.01).abs() < 1e-10);
    }

    #[test]
    fn pang_hybrid_star_steps_follow_growth() {
        let model = PangHybridCostFn::new(4.0);
        let sparse = PangHybridEstimate {
            estimate: Estimate {
                score: 0.0,
                nnz: 2.0,
                rows: 2.0,
                cols: 2.0,
            },
            identity: false,
        };
        let dense = PangHybridEstimate {
            estimate: Estimate {
                score: 0.0,
                nnz: 4.0,
                rows: 2.0,
                cols: 2.0,
            },
            identity: false,
        };
        assert_eq!(model.closure_steps(&sparse, sparse.estimate.nnz), 1);
        assert_eq!(model.closure_steps(&dense, dense.estimate.nnz), 6);
    }

    #[test]
    fn pang_hybrid_identity_remains_consistent_through_union_and_star() {
        let mut model = PangHybridCostFn::new(4.0);
        let empty = PangHybridEstimate {
            estimate: Estimate {
                score: 0.0,
                nnz: 0.0,
                rows: 0.0,
                cols: 0.0,
            },
            identity: false,
        };
        let identity = model.closure(empty.clone(), empty, false, true);
        let union = model.alternate(&identity, &identity);
        let nested_star = model.closure(identity.clone(), identity, false, true);
        for result in [union, nested_star.clone()] {
            assert!(result.identity);
            assert_eq!(result.estimate.nnz, 4.0);
            assert_eq!(result.estimate.rows, 4.0);
            assert_eq!(result.estimate.cols, 4.0);
        }

        let operand = PangHybridEstimate {
            estimate: Estimate {
                score: 1.0,
                nnz: 2.0,
                rows: 1.0,
                cols: 2.0,
            },
            identity: false,
        };
        let left = model.closure(nested_star.clone(), operand.clone(), true, false);
        let right = model.closure(nested_star, operand.clone(), false, false);
        for result in [left, right] {
            assert!(!result.identity);
            assert_eq!(result.estimate.nnz, operand.estimate.nnz);
            assert_eq!(result.estimate.rows, operand.estimate.rows);
            assert_eq!(result.estimate.cols, operand.estimate.cols);
        }
    }

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
                nnz: 6.0,
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
                nnz: 6.0,
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
