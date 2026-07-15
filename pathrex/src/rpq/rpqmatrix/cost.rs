use std::cmp::Ordering;

use egg::{CostFunction, Id};

use super::plan::RpqPlan;

#[derive(Clone, Debug, PartialEq)]
pub struct CardCost {
    pub score: f64,
    pub nnz: f64,
    pub nnz_r: f64,
    pub nnz_c: f64,
}

impl Eq for CardCost {}

impl PartialOrd for CardCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CardCost {
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

pub struct CardinalityCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
}

// TODO: check value intervals
impl CostFunction<RpqPlan> for CardinalityCostFn {
    type Cost = CardCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::NamedVertex(_name) => CardCost {
                score: 0.0,
                nnz: 1 as f64,
                nnz_r: 1 as f64,
                nnz_c: 1 as f64,
            },
            RpqPlan::Label(meta) => CardCost {
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

                let nnz_est = ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

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

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: nnz_r_est,
                    nnz_c: nnz_c_est,
                }
            }

            RpqPlan::Star([a]) => {
                let ca = costs(*a);

                let penalty = self.star_penalty * ca.nnz.max(1.0);
                let score = ca.score + penalty;

                CardCost {
                    score,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                }
            }

            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;
                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;

                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }
        }
    }
}

pub struct RandomCostFn;
impl CostFunction<RpqPlan> for RandomCostFn {
    type Cost = f64;
    fn cost<C>(&mut self, _enode: &RpqPlan, _costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        rand::random()
    }
}
