use std::cmp::Ordering;

use egg::{CostFunction, Id};

use super::plan::{LabelMeta, RpqPlan};

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
    fn product(
        &self,
        a: &PangHybridEstimate,
        b: &PangHybridEstimate,
        join_denominator: f64,
    ) -> (PangHybridEstimate, f64) {
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
    fn closure(
        &mut self,
        body: PangHybridEstimate,
        seed: PangHybridEstimate,
        left: bool,
        single: bool,
    ) -> PangHybridEstimate {
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
    fn cost<C: FnMut(Id) -> PangHybridEstimate>(
        &mut self,
        node: &RpqPlan,
        mut costs: C,
    ) -> PangHybridEstimate {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
