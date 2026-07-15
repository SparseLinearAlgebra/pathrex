use egg::{Extractor, RecExpr, Runner};

use super::cost::CardinalityCostFn;
use super::plan::{RpqPlan, make_rules};

#[derive(Clone, Copy)]
pub enum OptimizationStrategy {
    NoOpt,
    Cardinality,
    RandomOpt, // TODO: should be same as random from la-n-egg-rpq: https://github.com/SparseLinearAlgebra/la-n-egg-rpq/blob/main/src/main.rs#L75
    Simple,    // TODO
    Wander,    // TODO
}

pub(super) fn optimize_expr_cardinality(
    expr: RecExpr<RpqPlan>,
    graph_size: usize,
) -> RecExpr<RpqPlan> {
    let rules = make_rules();
    let runner = Runner::default()
        .with_explanations_disabled()
        .with_expr(&expr)
        .run(&rules);

    let extractor = Extractor::new(
        &runner.egraph,
        CardinalityCostFn {
            n: graph_size as f64,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        },
    );
    let (_, plan) = extractor.find_best(runner.roots[0]);
    plan
}
