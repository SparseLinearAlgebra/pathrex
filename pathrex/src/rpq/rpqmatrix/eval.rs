use super::expr::{materialize_with_storage, query_to_expr};
use super::optimize::{
    OptimizationStrategy, OptimizerCache, optimize_expr_hybrid, optimize_expr_join,
    optimize_expr_metaac, optimize_expr_mnc,
};
use super::result::{PreparedRpqMatrix, RpqMatrixResult};
/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
use crate::eval::Evaluator;
use crate::graph::{GraphDecomposition, MatrixStorage, set_global_matrix_storage_hint};
use crate::rpq::{Endpoint, RpqError, RpqQuery};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct RpqMatrixEvaluator {
    optimizer: OptimizationStrategy,
    cache: Arc<Mutex<OptimizerCache>>,
}

impl RpqMatrixEvaluator {
    pub fn unoptimized() -> Self {
        return RpqMatrixEvaluator {
            optimizer: OptimizationStrategy::NoOpt,
            cache: Arc::default(),
        };
    }
    pub fn optimized(opt: OptimizationStrategy) -> Self {
        return RpqMatrixEvaluator {
            optimizer: opt,
            cache: Arc::default(),
        };
    }
}

impl Default for RpqMatrixEvaluator {
    fn default() -> Self {
        RpqMatrixEvaluator::unoptimized()
    }
}

fn storage_for_query(query: &RpqQuery) -> MatrixStorage {
    match (&query.subject, &query.object) {
        (Endpoint::Variable(_), Endpoint::Named(_)) => MatrixStorage::Csc,
        _ => MatrixStorage::Csr,
    }
}

impl Evaluator for RpqMatrixEvaluator {
    type Query = RpqQuery;
    type Result = RpqMatrixResult;
    type Error = RpqError;
    type Prepared = PreparedRpqMatrix;

    fn prepare<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<PreparedRpqMatrix, RpqError> {
        let storage = storage_for_query(query);
        set_global_matrix_storage_hint(storage)?;

        let mut expr = query_to_expr(query, graph)?;
        match self.optimizer {
            OptimizationStrategy::NoOpt => {}
            OptimizationStrategy::Join => {
                expr = optimize_expr_join(expr, graph.num_nodes());
            }
            OptimizationStrategy::MetaAc => expr = optimize_expr_metaac(expr, graph.num_nodes()),
            OptimizationStrategy::Mnc => expr = optimize_expr_mnc(expr, graph, &self.cache)?,
            OptimizationStrategy::Hybrid => expr = optimize_expr_hybrid(expr, graph.num_nodes()),
            OptimizationStrategy::RandomOpt
            | OptimizationStrategy::Simple
            | OptimizationStrategy::Wander => todo!(),
        }

        let (plans, owned_matrices) = materialize_with_storage(&expr, graph, storage)?;

        Ok(PreparedRpqMatrix {
            plans,
            owned_matrices,
            storage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpq::{Endpoint, PathExpr, RpqQuery};
    use crate::utils::build_graph;

    #[test]
    fn evaluate_single_edge_nnz() {
        let graph = build_graph(&[("A", "B", "p")]);
        let q = RpqQuery {
            subject: Endpoint::Variable("x".into()),
            path: PathExpr::Label("p".into()),
            object: Endpoint::Variable("y".into()),
        };
        let result = RpqMatrixEvaluator::default()
            .evaluate(&q, &graph)
            .expect("evaluate");
        assert_eq!(result.nnz, 1);
    }

    #[test]
    fn evaluate_named_subject_no_match_nnz() {
        // Graph: A --p--> B
        // Query: <C> p ?y  -> C has no outgoing p edges, nnz=0
        let graph = build_graph(&[("A", "B", "p"), ("C", "D", "q")]);
        let q = RpqQuery {
            subject: Endpoint::Named("C".into()),
            path: PathExpr::Label("p".into()),
            object: Endpoint::Variable("y".into()),
        };
        let result = RpqMatrixEvaluator::default()
            .evaluate(&q, &graph)
            .expect("evaluate");
        assert_eq!(result.nnz, 0, "C has no outgoing p edges");
    }

    #[test]
    fn optimizers_preserve_results() {
        let graph = build_graph(&[
            ("A", "B", "p"),
            ("B", "C", "p"),
            ("B", "C", "q"),
            ("A", "D", "q"),
        ]);
        let queries = [
            (
                RpqQuery {
                    subject: Endpoint::Named("A".into()),
                    path: PathExpr::Sequence(
                        Box::new(PathExpr::Label("p".into())),
                        Box::new(PathExpr::Label("q".into())),
                    ),
                    object: Endpoint::Variable("y".into()),
                },
                1,
            ),
            (
                RpqQuery {
                    subject: Endpoint::Named("A".into()),
                    path: PathExpr::ZeroOrMore(Box::new(PathExpr::Label("p".into()))),
                    object: Endpoint::Variable("y".into()),
                },
                3,
            ),
            (
                RpqQuery {
                    subject: Endpoint::Variable("x".into()),
                    path: PathExpr::ZeroOrMore(Box::new(PathExpr::Label("p".into()))),
                    object: Endpoint::Named("C".into()),
                },
                3,
            ),
        ];

        for optimizer in [
            OptimizationStrategy::MetaAc,
            OptimizationStrategy::Mnc,
            OptimizationStrategy::Hybrid,
        ] {
            let evaluator = RpqMatrixEvaluator::optimized(optimizer);
            for (query, expected) in &queries {
                for _ in 0..2 {
                    let result = evaluator
                        .evaluate(query, &graph)
                        .expect("optimized evaluation");
                    assert_eq!(
                        result.nnz, *expected,
                        "optimizer={optimizer:?}, query={query:?}"
                    );
                }
            }
        }
    }
}
