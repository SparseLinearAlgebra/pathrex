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

/// Statistics of C in R = (has_identity ? I : empty) union C.
/// C may contain diagonal pairs; it is not assumed to be irreflexive.
#[derive(Clone, Debug, Default)]
struct CoreEstimate {
    nnz: f64,
    rows: f64,
    cols: f64,
    // Bounds conditional on the estimated core sizes, not confidence intervals
    // or guaranteed bounds on the actual graph cardinality.
    diagonal_lower: f64,
    diagonal_upper: f64,
}

#[derive(Clone, Debug)]
pub(super) struct PangHybridEstimate {
    // Full materialized relation; nnz uses the upper end of the diagonal range.
    estimate: Estimate,
    core: CoreEstimate,
    has_identity: bool,
    nnz_lower: f64,
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
        self.estimate
            .cmp(&other.estimate)
            .then(self.nnz_lower.total_cmp(&other.nnz_lower))
    }
}

pub(super) struct PangHybridCostFn {
    n: f64,
}

impl PangHybridCostFn {
    pub fn new(n: f64) -> Self {
        Self { n }
    }

    fn relation(
        &self,
        mut core: CoreEstimate,
        has_identity: bool,
        score: f64,
    ) -> PangHybridEstimate {
        core.rows = core.rows.clamp(0.0, self.n);
        core.cols = core.cols.clamp(0.0, self.n);
        core.nnz = core.nnz.clamp(0.0, core.rows * core.cols);
        core.rows = core.rows.min(core.nnz);
        core.cols = core.cols.min(core.nnz);
        let diagonal_cap = core.nnz.min(core.rows).min(core.cols);
        core.diagonal_lower = core.diagonal_lower.clamp(0.0, diagonal_cap);
        core.diagonal_upper = core.diagonal_upper.clamp(core.diagonal_lower, diagonal_cap);
        let (nnz_lower, nnz, rows, cols) = if has_identity {
            (
                (self.n + core.nnz - core.diagonal_upper).clamp(self.n, self.n * self.n),
                (self.n + core.nnz - core.diagonal_lower).clamp(self.n, self.n * self.n),
                self.n,
                self.n,
            )
        } else {
            (core.nnz, core.nnz, core.rows, core.cols)
        };
        PangHybridEstimate {
            estimate: Estimate {
                score,
                nnz,
                rows,
                cols,
            },
            core,
            has_identity,
            nnz_lower,
        }
    }

    fn label(&self, meta: &LabelMeta) -> PangHybridEstimate {
        self.relation(
            CoreEstimate {
                nnz: meta.nvals as f64,
                rows: meta.nonzero_rows as f64,
                cols: meta.nonzero_cols as f64,
                diagonal_lower: 0.0,
                diagonal_upper: meta.nvals.min(meta.nonzero_rows).min(meta.nonzero_cols) as f64,
            },
            false,
            0.0,
        )
    }

    fn vertex(&self) -> PangHybridEstimate {
        self.relation(
            CoreEstimate {
                nnz: 1.0,
                rows: 1.0,
                cols: 1.0,
                diagonal_lower: 1.0,
                diagonal_upper: 1.0,
            },
            false,
            0.0,
        )
    }

    fn core_product(&self, a: &CoreEstimate, b: &CoreEstimate, denominator: f64) -> CoreEstimate {
        if a.nnz == 0.0 || b.nnz == 0.0 {
            return CoreEstimate::default();
        }
        // Retain the previous scalar compatibility heuristic for the core.
        // Seq and closure still choose their original kinds of denominator.
        let join = a.cols * b.rows / denominator.max(1.0);
        let nnz = (join * (a.nnz / a.cols) * (b.nnz / b.rows))
            .min(a.rows * b.cols)
            .min(self.n * self.n);
        let rows = (a.rows * join / a.cols).min(a.rows).min(nnz);
        let cols = (b.cols * join / b.rows).min(b.cols).min(nnz);
        CoreEstimate {
            nnz,
            rows,
            cols,
            diagonal_lower: 0.0,
            diagonal_upper: nnz.min(rows).min(cols),
        }
    }

    fn core_union(&self, a: &CoreEstimate, b: &CoreEstimate) -> CoreEstimate {
        if a.nnz == 0.0 {
            return b.clone();
        }
        if b.nnz == 0.0 {
            return a.clone();
        }
        let n = self.n.max(1.0);
        CoreEstimate {
            nnz: (a.nnz + b.nnz - a.nnz * b.nnz / (n * n))
                .max(a.nnz.max(b.nnz))
                .min(self.n * self.n),
            rows: (a.rows + b.rows - a.rows * b.rows / n)
                .max(a.rows.max(b.rows))
                .min(self.n),
            cols: (a.cols + b.cols - a.cols * b.cols / n)
                .max(a.cols.max(b.cols))
                .min(self.n),
            diagonal_lower: a.diagonal_lower.max(b.diagonal_lower),
            diagonal_upper: (a.diagonal_upper + b.diagonal_upper).min(self.n),
        }
    }

    fn product(
        &self,
        a: &PangHybridEstimate,
        b: &PangHybridEstimate,
        denominator: f64,
    ) -> (PangHybridEstimate, f64) {
        // Keep the physical work proxy on the full materialized matrices.
        let work = a.estimate.nnz * b.estimate.nnz / a.estimate.rows.max(b.estimate.cols).max(1.0);
        let mut core = self.core_product(&a.core, &b.core, denominator);
        if a.has_identity {
            core = self.core_union(&core, &b.core);
        }
        if b.has_identity {
            core = self.core_union(&core, &a.core);
        }
        (
            self.relation(core, a.has_identity && b.has_identity, 0.0),
            work,
        )
    }

    fn alternate(&self, a: &PangHybridEstimate, b: &PangHybridEstimate) -> PangHybridEstimate {
        self.relation(
            self.core_union(&a.core, &b.core),
            a.has_identity || b.has_identity,
            0.0,
        )
    }

    fn closure_steps(&self, body: &PangHybridEstimate, initial: f64) -> usize {
        if body.core.nnz == 0.0 || initial == 0.0 {
            return 1;
        }
        // Deliberately retain the old depth heuristic, applied only to C.
        let growth = body.core.nnz / self.n.max(1.0);
        if growth >= 1.0 {
            6
        } else {
            (-initial.max(1.0).ln() / growth.ln())
                .ceil()
                .clamp(1.0, 64.0) as usize
        }
    }

    fn closure(
        &self,
        body: PangHybridEstimate,
        seed: PangHybridEstimate,
        left: bool,
        single: bool,
        reflexive: bool,
    ) -> PangHybridEstimate {
        let step = self.relation(body.core.clone(), false, 0.0);
        if body.has_identity && body.core.nnz == 0.0 {
            let mut out = if single { body.clone() } else { seed.clone() };
            out.estimate.score = if single {
                body.estimate.score + self.n
            } else {
                body.estimate.score + seed.estimate.score + body.estimate.nnz + seed.estimate.nnz
            };
            return out;
        }
        let mut result = if single { step.clone() } else { seed.clone() };
        let mut power = result.clone();
        let initial = if single {
            step.core.nnz
        } else if left {
            self.product(&step, &result, self.n.max(1.0)).0.core.nnz
        } else {
            self.product(&result, &step, self.n.max(1.0)).0.core.nnz
        };
        let steps = self.closure_steps(&step, initial);
        let mut score = body.estimate.score + if single { 0.0 } else { seed.estimate.score };
        if single || (step.core.nnz > 0.0 && seed.estimate.nnz > 0.0) {
            for _ in 0..steps - usize::from(single) {
                let (product, work) = if left {
                    self.product(&body, &result, self.n.max(1.0))
                } else {
                    self.product(&result, &body, self.n.max(1.0))
                };
                let next_power = if left {
                    self.product(&step, &power, self.n.max(1.0)).0
                } else {
                    self.product(&power, &step, self.n.max(1.0)).0
                };
                let mut next = if next_power.core.nnz < 1.0 {
                    result.clone()
                } else {
                    self.alternate(&result, &next_power)
                };
                // A closure keeps the seed's bound side. A symbolic full I is
                // unconstrained and must not restrict C to its empty core.
                if single || (!left && !seed.has_identity) {
                    next.core.rows = seed.core.rows;
                }
                if single || (left && !seed.has_identity) {
                    next.core.cols = seed.core.cols;
                }
                next = self.relation(next.core, next.has_identity, 0.0);
                score += work + result.estimate.nnz + product.estimate.nnz + next.estimate.nnz;
                result = next;
                power = next_power;
                if power.core.nnz < 1.0 || result.core.nnz >= result.core.rows * result.core.cols {
                    break;
                }
            }
        }
        if single {
            result = self.relation(result.core, reflexive || body.has_identity, 0.0);
            if reflexive {
                score += self.n + result.core.nnz + result.estimate.nnz;
            } else {
                score += body.estimate.nnz;
            }
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
                let (mut out, work) = self.product(&a, &b, a.core.rows.max(b.core.cols).max(1.0));
                out.estimate.score = a.estimate.score + b.estimate.score + work;
                out
            }
            RpqPlan::Alt([left, right]) => {
                let (a, b) = (costs(*left), costs(*right));
                let mut out = if left == right {
                    a.clone()
                } else {
                    self.alternate(&a, &b)
                };
                out.estimate.score = a.estimate.score + b.estimate.score + out.estimate.nnz;
                out
            }
            RpqPlan::Star([a]) => {
                let a = costs(*a);
                self.closure(a.clone(), a, false, true, true)
            }
            RpqPlan::Plus([a]) => {
                let a = costs(*a);
                self.closure(a.clone(), a, false, true, false)
            }
            RpqPlan::LStar([a, b]) => self.closure(costs(*a), costs(*b), true, false, false),
            RpqPlan::RStar([a, b]) => self.closure(costs(*b), costs(*a), false, false, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(nnz: f64, rows: f64, cols: f64, diagonal: Option<f64>) -> CoreEstimate {
        CoreEstimate {
            nnz,
            rows,
            cols,
            diagonal_lower: diagonal.unwrap_or(0.0),
            diagonal_upper: diagonal.unwrap_or(nnz.min(rows).min(cols)),
        }
    }

    fn assert_same_relation(a: &PangHybridEstimate, b: &PangHybridEstimate) {
        assert_eq!(a.has_identity, b.has_identity);
        assert_eq!(a.estimate.nnz, b.estimate.nnz);
        assert_eq!(a.estimate.rows, b.estimate.rows);
        assert_eq!(a.estimate.cols, b.estimate.cols);
        assert_eq!(a.nnz_lower, b.nnz_lower);
        assert_eq!(a.core.nnz, b.core.nnz);
        assert_eq!(a.core.rows, b.core.rows);
        assert_eq!(a.core.cols, b.core.cols);
        assert_eq!(a.core.diagonal_lower, b.core.diagonal_lower);
        assert_eq!(a.core.diagonal_upper, b.core.diagonal_upper);
    }

    #[test]
    fn full_identity_does_not_enlarge_the_core() {
        let model = PangHybridCostFn::new(100_000_000.0);
        let star = model.relation(core(100.0, 20.0, 30.0, None), true, 0.0);
        assert_eq!(star.estimate.rows, model.n);
        assert_eq!(star.estimate.cols, model.n);
        assert_eq!(star.core.rows, 20.0);
        assert_eq!(star.core.cols, 30.0);
        assert_eq!(star.nnz_lower, model.n + 80.0);
        assert_eq!(star.estimate.nnz, model.n + 100.0);
    }

    #[test]
    fn identity_products_preserve_every_small_core_including_diagonals() {
        for n in 0_usize..=3 {
            let model = PangHybridCostFn::new(n as f64);
            let identity = model.relation(CoreEstimate::default(), true, 0.0);
            for bits in 0_u32..(1_u32 << (n * n)) {
                let has = |i: usize, j: usize| (bits & (1_u32 << (i * n + j))) != 0_u32;
                let stats = core(
                    bits.count_ones() as f64,
                    (0..n).filter(|&i| (0..n).any(|j| has(i, j))).count() as f64,
                    (0..n).filter(|&j| (0..n).any(|i| has(i, j))).count() as f64,
                    Some((0..n).filter(|&i| has(i, i)).count() as f64),
                );
                for has_identity in [false, true] {
                    let a = model.relation(stats.clone(), has_identity, 0.0);
                    assert_same_relation(&a, &model.product(&identity, &a, 1.0).0);
                    assert_same_relation(&a, &model.product(&a, &identity, model.n.max(1.0)).0);
                    if has_identity {
                        let want = model.n + stats.nnz - stats.diagonal_lower;
                        assert_eq!(a.nnz_lower, want);
                        assert_eq!(a.estimate.nnz, want);
                    }
                }
            }
        }
    }

    #[test]
    fn unions_count_the_full_identity_once() {
        let model = PangHybridCostFn::new(100.0);
        let identity = model.relation(CoreEstimate::default(), true, 0.0);
        assert_same_relation(&identity, &model.alternate(&identity, &identity));
        let vertex = model.vertex();
        let union = model.alternate(&identity, &vertex);
        assert_eq!(union.estimate.nnz, 100.0);
        assert_eq!(union.nnz_lower, 100.0);
        assert_eq!(union.core.nnz, 1.0);
        assert!(union.has_identity);
    }

    #[test]
    fn a_star_times_b_keeps_b_even_when_core_product_vanishes() {
        let model = PangHybridCostFn::new(100_000_000.0);
        let a = model.relation(core(2.0, 2.0, 2.0, None), true, 0.0);
        let b = model.vertex();
        let out = model.product(&a, &b, model.n).0;
        assert!(!out.has_identity);
        assert!(out.core.nnz >= b.core.nnz);
        assert!(out.core.rows >= b.core.rows);
        assert!(out.core.cols >= b.core.cols);
        assert!(out.core.diagonal_lower >= b.core.diagonal_lower);
    }

    #[test]
    fn star_product_keeps_both_cores_and_a_single_identity() {
        let model = PangHybridCostFn::new(100.0);
        let a = model.relation(core(2.0, 2.0, 2.0, None), true, 0.0);
        let b = model.relation(core(3.0, 3.0, 3.0, None), true, 0.0);
        let out = model.product(&a, &b, 3.0).0;
        assert!(out.has_identity);
        assert!(out.core.nnz >= 3.0);
        assert!(out.core.rows >= 3.0);
        assert!(out.core.cols >= 3.0);
        assert!(out.estimate.nnz >= model.n);
        assert!(out.estimate.nnz <= model.n + out.core.nnz);
    }

    #[test]
    fn closure_growth_ignores_the_full_identity_component() {
        let model = PangHybridCostFn::new(100_000_000.0);
        let a = model.relation(core(2.0, 2.0, 2.0, None), false, 0.0);
        let star = model.relation(a.core.clone(), true, 0.0);
        assert_eq!(
            model.closure_steps(&a, 2.0),
            model.closure_steps(&star, 2.0)
        );
        assert_eq!(model.closure_steps(&star, 2.0), 1);
        let dense = model.relation(core(model.n, 10_000.0, 10_000.0, None), false, 0.0);
        assert_eq!(model.closure_steps(&dense, model.n), 6);
    }

    #[test]
    fn unary_closures_preserve_cycle_diagonals_and_nullable_inputs() {
        let mut model = PangHybridCostFn::new(100.0);
        let cycle = model.relation(core(4.0, 2.0, 2.0, Some(2.0)), false, 0.0);
        let plus = model.cost(&RpqPlan::Plus([Id::from(0)]), |_| cycle.clone());
        let star = model.cost(&RpqPlan::Star([Id::from(0)]), |_| cycle.clone());
        assert!(!plus.has_identity);
        assert_eq!(plus.estimate.nnz, 4.0);
        assert_eq!(star.estimate.nnz, 102.0);
        assert_eq!(star.nnz_lower, 102.0);
        assert_eq!(star.core.diagonal_lower, 2.0);
        let nested_plus = model.cost(&RpqPlan::Plus([Id::from(0)]), |_| star.clone());
        assert_same_relation(&star, &nested_plus);
    }

    #[test]
    fn empty_closures_work_for_zero_and_nonzero_dimensions() {
        for n in [0.0, 1.0, 100.0] {
            let model = PangHybridCostFn::new(n);
            let empty = model.relation(CoreEstimate::default(), false, 0.0);
            let plus = model.closure(empty.clone(), empty.clone(), false, true, false);
            let star = model.closure(empty.clone(), empty, false, true, true);
            assert!(!plus.has_identity);
            assert_eq!(plus.estimate.nnz, 0.0);
            assert!(star.has_identity);
            assert_eq!(star.estimate.nnz, n);
            assert_eq!(star.core.nnz, 0.0);
            let nested_star = model.closure(star.clone(), star.clone(), false, true, true);
            let nested_plus = model.closure(star.clone(), star.clone(), false, true, false);
            assert_same_relation(&star, &nested_star);
            assert_same_relation(&star, &nested_plus);
        }
    }

    #[test]
    fn seeded_closures_keep_the_seed_and_its_bound_side() {
        let model = PangHybridCostFn::new(4.0);
        let body = model.relation(core(4.0, 2.0, 2.0, None), false, 0.0);
        let seed = model.vertex();
        for left in [false, true] {
            let out = model.closure(body.clone(), seed.clone(), left, false, false);
            assert!(out.core.nnz >= seed.core.nnz);
            assert!(out.core.diagonal_lower >= seed.core.diagonal_lower);
            assert!(!out.has_identity);
            if left {
                assert_eq!(out.core.cols, seed.core.cols);
            } else {
                assert_eq!(out.core.rows, seed.core.rows);
            }
            let identity = model.relation(CoreEstimate::default(), true, 0.0);
            let with_identity_seed = model.closure(body.clone(), identity, left, false, false);
            assert!(with_identity_seed.has_identity);
            assert!(with_identity_seed.core.nnz > 0.0);
        }
    }

    #[test]
    fn physical_work_still_sees_full_materialized_matrices() {
        let model = PangHybridCostFn::new(100.0);
        let a = model.relation(CoreEstimate::default(), true, 0.0);
        let b = model.vertex();
        let (_, work) = model.product(&a, &b, 1.0);
        assert_eq!(work, 1.0);
        let (_, work) = model.product(&a, &a, 1.0);
        assert_eq!(work, 100.0);
    }
}
