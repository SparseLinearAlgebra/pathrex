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

// TODO: enforce or encode `n > 0`; several estimates divide by `n` or `n^2`.
// TODO: decide whether all estimated cardinalities should be clamped to `[0, n^2]`.
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

                // TODO: this can exceed `n^2` when child estimates are already loose.
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

                CardCost {
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

                // TODO: LStar/RStar currently reuse Seq-like row/column estimates and do not
                // model the closure side directly.
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

                // TODO: LStar/RStar currently reuse Seq-like row/column estimates and do not
                // model the closure side directly.
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

#[cfg(test)]
mod tests {
    use egg::RecExpr;

    use crate::rpq::rpqmatrix::{
        optimize::optimize_expr_cardinality,
        plan::{LabelMeta, RpqPlan},
    };

    use super::*;

    fn assert_finite_nonnegative(cost: &CardCost) {
        assert!(cost.score.is_finite(), "score must be finite: {cost:?}");
        assert!(cost.nnz.is_finite(), "nnz must be finite: {cost:?}");
        assert!(cost.nnz_r.is_finite(), "nnz_r must be finite: {cost:?}");
        assert!(cost.nnz_c.is_finite(), "nnz_c must be finite: {cost:?}");

        assert!(cost.score >= 0.0, "score must be non-negative: {cost:?}");
        assert!(cost.nnz >= 0.0, "nnz must be non-negative: {cost:?}");
        assert!(cost.nnz_r >= 0.0, "nnz_r must be non-negative: {cost:?}");
        assert!(cost.nnz_c >= 0.0, "nnz_c must be non-negative: {cost:?}");
    }

    fn child_cost(id: Id, a: Id, ca: &CardCost, b: Id, cb: &CardCost) -> CardCost {
        if id == a {
            ca.clone()
        } else if id == b {
            cb.clone()
        } else {
            panic!("unexpected child id: {id:?}")
        }
    }

    fn unary_child_cost(id: Id, child: Id, cost: &CardCost) -> CardCost {
        if id == child {
            cost.clone()
        } else {
            panic!("unexpected child id: {id:?}")
        }
    }

    #[test]
    fn card_cost_order_uses_score_then_nnz_then_rows_then_cols() {
        let base = CardCost {
            score: 10.0,
            nnz: 20.0,
            nnz_r: 30.0,
            nnz_c: 40.0,
        };

        assert!(
            CardCost {
                score: 9.0,
                nnz: 100.0,
                nnz_r: 100.0,
                nnz_c: 100.0,
            } < base
        );
        assert!(
            CardCost {
                score: 10.0,
                nnz: 19.0,
                nnz_r: 100.0,
                nnz_c: 100.0,
            } < base
        );
        assert!(
            CardCost {
                score: 10.0,
                nnz: 20.0,
                nnz_r: 29.0,
                nnz_c: 100.0,
            } < base
        );
        assert!(
            CardCost {
                score: 10.0,
                nnz: 20.0,
                nnz_r: 30.0,
                nnz_c: 39.0,
            } < base
        );
    }

    #[test]
    fn cardinality_cost_base_nodes_use_vertex_and_label_metadata() {
        let mut cost_fn = CardinalityCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };

        let named = cost_fn.cost(&RpqPlan::NamedVertex("A".to_string()), |_| {
            panic!("NamedVertex must not request child costs")
        });
        assert_eq!(
            named,
            CardCost {
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
            CardCost {
                score: 0.0,
                nnz: 17.0,
                nnz_r: 5.0,
                nnz_c: 9.0,
            }
        );
    }
    #[test]
    fn cardinality_cost_seq_correctness() {
        let mut cost_fn = CardinalityCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);

        let ca = CardCost {
            score: 2.0,
            nnz: 20.0,
            nnz_r: 4.0,
            nnz_c: 8.0,
        };

        let cb = CardCost {
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
            CardCost {
                score: 65.0,
                nnz: 0.06,
                nnz_r: 4.0,
                nnz_c: 10.0,
            }
        );
    }
    #[test]
    fn cardinality_cost_seq_correctness_zero_denom() {
        let mut cost_fn = CardinalityCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);

        let ca = CardCost {
            score: 2.0,
            nnz: 20.0,
            nnz_r: 0.0,
            nnz_c: 4.0,
        };

        let cb = CardCost {
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
            CardCost {
                score: 605.0,
                nnz: 0.06,
                nnz_r: 0.0,
                nnz_c: 0.0,
            }
        );
    }
    #[test]
    fn cardinality_cost_alt_with_zero_children_stays_finite_nonnegative() {
        let mut cost_fn = CardinalityCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);
        let ca = CardCost {
            score: 0.0,
            nnz: 0.0,
            nnz_r: 0.0,
            nnz_c: 0.0,
        };
        let cb = CardCost {
            score: 3.0,
            nnz: 30.0,
            nnz_r: 0.0,
            nnz_c: 10.0,
        };

        let alt = cost_fn.cost(&RpqPlan::Alt([a, b]), |id| child_cost(id, a, &ca, b, &cb));

        assert_finite_nonnegative(&alt);
        assert_eq!(
            alt,
            CardCost {
                score: 33.0,
                nnz: 30.0,
                nnz_r: 0.0,
                nnz_c: 10.0,
            }
        );
    }

    #[test]
    fn cardinality_cost_star_with_zero_nnz_uses_min_penalty() {
        let mut cost_fn = CardinalityCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let ca = CardCost {
            score: 7.0,
            nnz: 0.0,
            nnz_r: 0.0,
            nnz_c: 0.0,
        };

        let star = cost_fn.cost(&RpqPlan::Star([a]), |id| unary_child_cost(id, a, &ca));

        assert_finite_nonnegative(&star);
        assert_eq!(
            star,
            CardCost {
                score: 57.0,
                nnz: 10_000.0,
                nnz_r: 100.0,
                nnz_c: 100.0,
            }
        );
    }

    #[test]
    fn cardinality_cost_lstar_and_rstar_zero_denom_stay_finite_nonnegative() {
        let mut cost_fn = CardinalityCostFn {
            n: 100.0,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        };
        let a = Id::from(0);
        let b = Id::from(1);
        let ca = CardCost {
            score: 2.0,
            nnz: 20.0,
            nnz_r: 0.0,
            nnz_c: 4.0,
        };
        let cb = CardCost {
            score: 3.0,
            nnz: 30.0,
            nnz_r: 4.0,
            nnz_c: 0.0,
        };

        let lstar = cost_fn.cost(&RpqPlan::LStar([a, b]), |id| child_cost(id, a, &ca, b, &cb));
        let rstar = cost_fn.cost(&RpqPlan::RStar([a, b]), |id| child_cost(id, a, &ca, b, &cb));

        assert_finite_nonnegative(&lstar);
        assert_finite_nonnegative(&rstar);
        let expected = CardCost {
            score: 3005.0,
            nnz: 0.3,
            nnz_r: 0.0,
            nnz_c: 0.0,
        };

        assert_eq!(lstar, expected);
        assert_eq!(rstar, expected);
    }
    #[test]
    fn cardinality_cost_build_lstar() {
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
        let opt = optimize_expr_cardinality(expr, 100);
        let root = opt.as_ref().last().expect("optimized expr is non-empty");

        assert!(matches!(root, RpqPlan::LStar(_)));
    }
    #[test]
    fn cardinality_cost_build_rstar() {
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
        let opt = optimize_expr_cardinality(expr, 100);
        let root = opt.as_ref().last().expect("optimized expr is non-empty");

        assert!(matches!(root, RpqPlan::RStar(_)));
    }
    //TODO: maybe cover other rules
}
