use super::expr::{materialize_with_storage, query_to_expr};
use super::learning::{LinearCostModel, LinearObservation};
use super::optimize::{EGraphOptimizer, OptimizationStrategy, RpqOptimizer};
use super::result::{PreparedRpqMatrix, RpqMatrixResult};
use super::stochastic::candidates;
/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
use crate::eval::Evaluator;
use crate::eval::PreparedEvaluator;
use crate::graph::{GraphDecomposition, MatrixStorage, set_global_matrix_storage_hint};
use crate::rpq::{Endpoint, RpqError, RpqQuery};
use std::sync::Arc;
use std::time::Instant;

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

    /// Construct an evaluator with frozen full-plan linear coefficients.
    pub fn with_linear_model(model: LinearCostModel) -> Self {
        Self {
            optimizer: Arc::new(EGraphOptimizer::with_linear_model(model)),
        }
    }

    /// Collect training observations for all parenthesizations searched by
    /// inference. This explicit training API times ordinary full-plan execution;
    /// query preparation and returned-result destruction are outside the timer.
    pub fn collect_linear_observations<G: GraphDecomposition>(
        query: &RpqQuery,
        graph: &G,
        runs: usize,
        warmups: usize,
    ) -> Result<Vec<LinearObservation>, RpqError> {
        if runs == 0 {
            return Err(RpqError::UnsupportedPath(
                "training runs must be positive".into(),
            ));
        }
        let storage = storage_for_query(query);
        set_global_matrix_storage_hint(storage)?;
        let expr = query_to_expr(query, graph)?;
        let candidates = candidates(expr, graph.num_nodes())?;
        let mut observations = Vec::new();
        for candidate in candidates {
            let (plans, owned_matrices) =
                materialize_with_storage(&candidate.expr, graph, storage)?;
            let mut prepared = PreparedRpqMatrix {
                plans,
                owned_matrices,
                storage,
            };
            for _ in 0..warmups {
                prepared.execute()?;
            }
            for _ in 0..runs {
                let started = Instant::now();
                let result = prepared.execute()?;
                let elapsed_us = started.elapsed().as_secs_f64() * 1e6;
                drop(result);
                observations.push(LinearObservation {
                    features: candidate.features,
                    elapsed_us,
                });
            }
        }
        Ok(observations)
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

    #[test]
    fn linear_model_preserves_full_relations_and_remains_frozen() {
        use crate::graph::GraphDecomposition;
        use crate::lagraph_sys::{GrB_Info, GrB_Matrix_extractElement_BOOL};
        let graph = build_graph(&[
            ("a", "b", "p"),
            ("b", "c", "p"),
            ("c", "a", "p"),
            ("b", "d", "q"),
            ("d", "b", "q"),
        ]);
        let model = LinearCostModel::from_coefficients([1.0, 0.5, 2.0]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("frozen.model");
        model.save(&path).unwrap();
        let evaluator =
            RpqMatrixEvaluator::with_linear_model(LinearCostModel::load(&path).unwrap());
        for pattern in [
            "?s <p>/<q>/<p> ?t",
            "<a> <p>/<q>/<p> ?t",
            "?s <p>/<q>/<p> <b>",
            "<a> <p>/<q>/<p> <b>",
            "?s <p> ?t",
        ] {
            let mut query = crate::sparql::parse_rpq(&format!(
                "BASE <http://example.org/> SELECT * WHERE {{ {pattern} }}"
            ))
            .unwrap();
            query.strip_base("http://example.org/");
            let expected = RpqMatrixEvaluator::unoptimized()
                .evaluate(&query, &graph)
                .unwrap();
            let mut prepared = evaluator.clone().prepare(&query, &graph).unwrap();
            for _ in 0..2 {
                let actual = prepared.execute().unwrap();
                assert_eq!(actual.nnz, expected.nnz, "{pattern}");
                for row in 0..graph.num_nodes() as u64 {
                    for column in 0..graph.num_nodes() as u64 {
                        let entry = |matrix| {
                            let mut value = false;
                            let status = unsafe {
                                GrB_Matrix_extractElement_BOOL(&mut value, matrix, row, column)
                            };
                            assert!(
                                status == GrB_Info::GrB_SUCCESS || status == GrB_Info::GrB_NO_VALUE
                            );
                            status == GrB_Info::GrB_SUCCESS && value
                        };
                        assert_eq!(
                            entry(actual.matrix.inner),
                            entry(expected.matrix.inner),
                            "{pattern} ({row},{column})"
                        );
                    }
                }
            }
        }
        assert_eq!(model.coefficients(), [1.0, 0.5, 2.0]);
    }

    #[test]
    fn training_collection_uses_the_inference_bank_and_excludes_warmups() {
        let graph = build_graph(&[
            ("a", "b", "p"),
            ("b", "c", "p"),
            ("c", "a", "p"),
            ("a", "c", "q"),
            ("b", "a", "q"),
        ]);
        let query = RpqQuery {
            subject: Endpoint::Variable("s".into()),
            path: PathExpr::Sequence(
                Box::new(PathExpr::Label("p".into())),
                Box::new(PathExpr::Sequence(
                    Box::new(PathExpr::Label("q".into())),
                    Box::new(PathExpr::Label("p".into())),
                )),
            ),
            object: Endpoint::Variable("t".into()),
        };
        let expr = query_to_expr(&query, &graph).unwrap();
        let bank = candidates(expr, graph.num_nodes()).unwrap();
        let observations =
            RpqMatrixEvaluator::collect_linear_observations(&query, &graph, 2, 1).unwrap();
        assert_eq!(observations.len(), 2 * bank.len());
        for (candidate, measured) in bank.iter().zip(observations.chunks_exact(2)) {
            for observation in measured {
                assert_eq!(observation.features, candidate.features);
                assert!(observation.elapsed_us.is_finite() && observation.elapsed_us > 0.0);
            }
        }
    }

    #[test]
    fn every_candidate_preserves_the_full_relation_for_sequence_associations() {
        use crate::graph::GraphDecomposition;
        use crate::lagraph_sys::{GrB_Info, GrB_Matrix_extractElement_BOOL};
        let graph = build_graph(&[
            ("a", "b", "p"),
            ("b", "c", "p"),
            ("c", "a", "p"),
            ("a", "c", "q"),
            ("c", "b", "q"),
            ("b", "b", "q"),
        ]);
        for (path, labels) in [
            ("<p>/<q>/<p>", 3),
            ("<p>/<p>/<p>/<p>", 4),
            ("<q>/<p>/<q>/<p>/<q>", 5),
        ] {
            for (source, target) in [("?s", "?t"), ("<a>", "?t"), ("?s", "<b>"), ("<a>", "<b>")] {
                let mut query = crate::sparql::parse_rpq(&format!(
                    "BASE <http://example.org/> SELECT * WHERE {{ {source} {path} {target} }}"
                ))
                .unwrap();
                query.strip_base("http://example.org/");
                let expected = RpqMatrixEvaluator::unoptimized()
                    .evaluate(&query, &graph)
                    .unwrap();
                let bank =
                    candidates(query_to_expr(&query, &graph).unwrap(), graph.num_nodes()).unwrap();
                let factors = labels + usize::from(source == "<a>") + usize::from(target == "<b>");
                assert_eq!(bank.len(), [1, 1, 2, 5, 14, 42, 132][factors - 1]);
                for candidate in bank {
                    let storage = storage_for_query(&query);
                    let (plans, owned_matrices) =
                        materialize_with_storage(&candidate.expr, &graph, storage).unwrap();
                    let mut prepared = PreparedRpqMatrix {
                        plans,
                        owned_matrices,
                        storage,
                    };
                    for _ in 0..2 {
                        let actual = prepared.execute().unwrap();
                        for row in 0..graph.num_nodes() as u64 {
                            for col in 0..graph.num_nodes() as u64 {
                                let entry = |matrix| {
                                    let mut value = false;
                                    let status = unsafe {
                                        GrB_Matrix_extractElement_BOOL(&mut value, matrix, row, col)
                                    };
                                    assert!(
                                        status == GrB_Info::GrB_SUCCESS
                                            || status == GrB_Info::GrB_NO_VALUE
                                    );
                                    status == GrB_Info::GrB_SUCCESS && value
                                };
                                assert_eq!(
                                    entry(actual.matrix.inner),
                                    entry(expected.matrix.inner),
                                    "{source} {path} {target}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn linear_optimizer_requires_a_model_and_rejects_out_of_scope_queries() {
        let graph = build_graph(&[("a", "b", "p")]);
        let query = RpqQuery {
            subject: Endpoint::Variable("s".into()),
            path: PathExpr::Label("p".into()),
            object: Endpoint::Variable("t".into()),
        };
        assert!(
            RpqMatrixEvaluator::optimized(OptimizationStrategy::Stochastic)
                .prepare(&query, &graph)
                .is_err()
        );
        assert!(RpqMatrixEvaluator::collect_linear_observations(&query, &graph, 0, 0).is_err());
        let evaluator = RpqMatrixEvaluator::with_linear_model(
            LinearCostModel::from_coefficients([1.0; 3]).unwrap(),
        );
        for path in [
            "<p>|<p>",
            "<p>*",
            "<p>+",
            "<p>?",
            "<p>/(<p>|<p>)",
            "(<p>/<p>)*",
        ] {
            let mut query = crate::sparql::parse_rpq(&format!(
                "BASE <http://example.org/> SELECT * WHERE {{ ?s {path} ?t }}"
            ))
            .unwrap();
            query.strip_base("http://example.org/");
            assert!(
                matches!(
                    evaluator.prepare(&query, &graph),
                    Err(RpqError::UnsupportedPath(_))
                ),
                "{path}"
            );
            assert!(
                matches!(
                    RpqMatrixEvaluator::collect_linear_observations(&query, &graph, 1, 0),
                    Err(RpqError::UnsupportedPath(_))
                ),
                "{path}"
            );
        }
    }
}
