use super::expr::{materialize_with_storage, query_to_expr};
use super::optimize::{EGraphOptimizer, OptimizationStrategy, RpqOptimizer};
use super::result::{PreparedRpqMatrix, RpqMatrixResult};
/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
use crate::eval::Evaluator;
use crate::graph::{GraphDecomposition, MatrixStorage, set_global_matrix_storage_hint};
use crate::rpq::{Endpoint, RpqError, RpqQuery};
use std::sync::Arc;

#[derive(Clone)]
pub struct RpqMatrixEvaluator {
    optimizer: Arc<EGraphOptimizer>,
}

impl RpqMatrixEvaluator {
    pub fn unoptimized() -> Self {
        Self::optimized(OptimizationStrategy::NoOpt)
    }
    pub fn optimized(opt: OptimizationStrategy) -> Self {
        RpqMatrixEvaluator {
            optimizer: Arc::new(EGraphOptimizer::new(opt)),
        }
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

        let expr = query_to_expr(query, graph)?;
        let expr = self.optimizer.optimize(expr, graph)?;

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
    use crate::formats::MatrixMarket;
    use crate::graph::{Graph, InMemory, MatrixStatsMode};
    use crate::rpq::{Endpoint, PathExpr, RpqQuery};
    use crate::utils::build_graph;

    #[test]
    fn optimizer_specific_matrix_statistics_preserve_results() {
        let query = RpqQuery {
            subject: Endpoint::Variable("x".into()),
            path: PathExpr::ZeroOrMore(Box::new(PathExpr::Label("knows".into()))),
            object: Endpoint::Variable("y".into()),
        };
        for (mode, optimizer) in [
            (MatrixStatsMode::None, OptimizationStrategy::PangHybrid),
            (MatrixStatsMode::None, OptimizationStrategy::Sampling),
            (MatrixStatsMode::Extended, OptimizationStrategy::Mnc),
        ] {
            let graph = Graph::<InMemory>::try_from(
                MatrixMarket::from_dir("tests/testdata/mm_small").with_matrix_stats(mode),
            )
            .unwrap();
            let expected = RpqMatrixEvaluator::unoptimized()
                .evaluate(&query, &graph)
                .unwrap()
                .nnz;
            let actual = RpqMatrixEvaluator::optimized(optimizer)
                .evaluate(&query, &graph)
                .unwrap()
                .nnz;
            assert_eq!(actual, expected, "{optimizer:?}");
        }
    }

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
            OptimizationStrategy::PangHybrid,
            OptimizationStrategy::Sampling,
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
