use egg::{Extractor, RecExpr, Runner};
use std::sync::LazyLock;

use super::cost::PangHybridCostFn;
use super::plan::{RpqPlan, make_rules};
use crate::{graph::GraphDecomposition, rpq::RpqError};

static RULES: LazyLock<Vec<egg::Rewrite<RpqPlan, ()>>> = LazyLock::new(make_rules);

#[derive(Clone, Copy, Debug)]
pub enum OptimizationStrategy {
    NoOpt,
    PangHybrid,
    RandomOpt, // TODO: should be same as random from la-n-egg-rpq: https://github.com/SparseLinearAlgebra/la-n-egg-rpq/blob/main/src/main.rs#L75
    Simple,    // TODO
    Wander,    // TODO
}

/// Common interface for selecting an expression before plan materialization.
///
/// Graph access stays generic; no dynamic dispatch is required.
pub(super) trait RpqOptimizer {
    fn optimize<G: GraphDecomposition>(
        &self,
        expr: RecExpr<RpqPlan>,
        graph: &G,
    ) -> Result<RecExpr<RpqPlan>, RpqError>;
}

/// E-graph optimization strategies.
pub(super) struct EGraphOptimizer {
    strategy: OptimizationStrategy,
}

impl EGraphOptimizer {
    pub(super) fn new(strategy: OptimizationStrategy) -> Self {
        Self { strategy }
    }

    fn runner(expr: &RecExpr<RpqPlan>) -> Runner<RpqPlan, ()> {
        Runner::default()
            .with_explanations_disabled()
            .with_expr(expr)
            .run(&*RULES)
    }

    fn extract_with<C: egg::CostFunction<RpqPlan>>(
        expr: &RecExpr<RpqPlan>,
        cost: C,
    ) -> RecExpr<RpqPlan> {
        let runner = Self::runner(expr);
        Extractor::new(&runner.egraph, cost)
            .find_best(runner.roots[0])
            .1
    }

    fn optimize_expr_pang_hybrid(&self, expr: RecExpr<RpqPlan>, n: usize) -> RecExpr<RpqPlan> {
        Self::extract_with(&expr, PangHybridCostFn::new(n as f64))
    }
}

impl RpqOptimizer for EGraphOptimizer {
    fn optimize<G: GraphDecomposition>(
        &self,
        expr: RecExpr<RpqPlan>,
        graph: &G,
    ) -> Result<RecExpr<RpqPlan>, RpqError> {
        match self.strategy {
            OptimizationStrategy::NoOpt => Ok(expr),
            OptimizationStrategy::PangHybrid => {
                Ok(self.optimize_expr_pang_hybrid(expr, graph.num_nodes()))
            }
            OptimizationStrategy::RandomOpt
            | OptimizationStrategy::Simple
            | OptimizationStrategy::Wander => Err(RpqError::UnsupportedPath(format!(
                "optimizer {:?} is not implemented",
                self.strategy
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_rules_expose_collapsed_closures_in_all_endpoint_contexts() {
        let star = "(* (/ (* 1) (* 2)))";
        let plus = "(+ (/ (* 1) (* 2)))";
        for body in [star, plus] {
            for (source, target) in [
                (body.to_string(), "(* (| 1 2))".to_string()),
                (format!("(/ {body} dst)"), "(/ (* (| 1 2)) dst)".into()),
                (format!("(/ src {body})"), "(/ src (* (| 1 2)))".into()),
                (
                    format!("(/ (/ src {body}) dst)"),
                    "(/ (/ src (* (| 1 2))) dst)".into(),
                ),
            ] {
                let expr = source.parse().unwrap();
                let runner = EGraphOptimizer::runner(&expr);
                assert!(
                    matches!(runner.stop_reason, Some(egg::StopReason::Saturated)),
                    "{source}: {:?}",
                    runner.stop_reason
                );
                assert!(runner.egraph.total_size() < 100, "{source}");
                let target: RecExpr<RpqPlan> = target.parse().unwrap();
                let target_id = runner
                    .egraph
                    .lookup_expr(&target)
                    .unwrap_or_else(|| panic!("collapsed candidate missing for {source}"));
                assert_eq!(
                    runner.egraph.find(runner.roots[0]),
                    runner.egraph.find(target_id),
                    "{source}"
                );
                eprintln!(
                    "{source}: {} nodes, {:?}",
                    runner.egraph.total_size(),
                    runner.stop_reason
                );
            }
        }
    }

    #[test]
    fn shared_rules_do_not_collapse_a_sequence_without_outer_closure() {
        let expr = "(/ (* 1) (* 2))".parse().unwrap();
        let runner = EGraphOptimizer::runner(&expr);
        let forbidden: RecExpr<RpqPlan> = "(* (| 1 2))".parse().unwrap();
        if let Some(id) = runner.egraph.lookup_expr(&forbidden) {
            assert_ne!(runner.egraph.find(runner.roots[0]), runner.egraph.find(id));
        }
    }

    #[test]
    fn no_opt_preserves_the_input_expression() {
        let graph = crate::utils::build_graph(&[("a", "b", "p")]);
        let expr: RecExpr<RpqPlan> = "(/ 1 2)".parse().unwrap();
        let optimizer = EGraphOptimizer::new(OptimizationStrategy::NoOpt);
        assert_eq!(optimizer.optimize(expr.clone(), &graph).unwrap(), expr);
    }

    #[test]
    fn unimplemented_strategies_return_the_existing_error() {
        let graph = crate::utils::build_graph(&[("a", "b", "p")]);
        let expr: RecExpr<RpqPlan> = "1".parse().unwrap();
        for strategy in [
            OptimizationStrategy::RandomOpt,
            OptimizationStrategy::Simple,
            OptimizationStrategy::Wander,
        ] {
            let optimizer = EGraphOptimizer::new(strategy);
            let error = optimizer.optimize(expr.clone(), &graph).unwrap_err();
            assert!(matches!(error, RpqError::UnsupportedPath(message)
                if message == format!("optimizer {strategy:?} is not implemented")));
        }
    }
}
