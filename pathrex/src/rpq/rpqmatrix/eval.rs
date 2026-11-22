use egg::{Language, RecExpr};

use super::expr::{materialize_with_storage, query_to_expr};
use super::optimize::{EGraphOptimizer, OptimizationStrategy, RpqOptimizer};
use super::plan::RpqPlan;
use super::result::{PreparedRpqMatrix, RpqMatrixResult};
/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
use crate::eval::Evaluator;
use crate::graph::{GraphDecomposition, MatrixStorage, set_global_matrix_storage_hint};
use crate::rpq::{Endpoint, RpqError, RpqQuery};
use std::sync::Arc;

/// Selected RPQMatrix expression and its shared operation DAG.
#[derive(Debug)]
#[cfg_attr(feature = "bench", derive(serde::Serialize))]
pub struct RpqMatrixPlanExplanation {
    pub storage: &'static str,
    pub expression: String,
    pub root: usize,
    pub nodes: Vec<RpqMatrixPlanNode>,
}

#[derive(Debug)]
#[cfg_attr(feature = "bench", derive(serde::Serialize))]
pub struct RpqMatrixPlanNode {
    pub id: usize,
    pub operation: &'static str,
    pub label: Option<String>,
    pub nnz: Option<usize>,
    pub vertex: Option<String>,
    pub children: Vec<usize>,
}

#[derive(Clone)]
pub struct RpqMatrixEvaluator {
    optimizer: Arc<EGraphOptimizer>,
}

impl RpqMatrixEvaluator {
    fn optimized_expression<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<RecExpr<RpqPlan>, RpqError> {
        set_global_matrix_storage_hint(storage_for_query(query))?;
        let expr = query_to_expr(query, graph)?;
        self.optimizer.optimize(expr, graph)
    }

    /// Select a plan using the same path as `prepare`, without executing it.
    /// Named endpoint validation is retained even though no selectors are built.
    pub fn explain<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<RpqMatrixPlanExplanation, RpqError> {
        let expr = self.optimized_expression(query, graph)?;
        let mut nodes = Vec::with_capacity(expr.len());
        for (id, node) in expr.as_ref().iter().enumerate() {
            let (operation, label, nnz, vertex) = match node {
                RpqPlan::Label(meta) => ("label", Some(meta.name.clone()), Some(meta.nvals), None),
                RpqPlan::NamedVertex(name) => {
                    graph
                        .get_node_id(name)
                        .ok_or_else(|| RpqError::VertexNotFound(name.clone()))?;
                    ("vertex_selector", None, None, Some(name.clone()))
                }
                RpqPlan::Seq(_) => ("sequence", None, None, None),
                RpqPlan::Alt(_) => ("union", None, None, None),
                RpqPlan::Star(_) => ("star", None, None, None),
                RpqPlan::Plus(_) => ("plus", None, None, None),
                RpqPlan::LStar(_) => ("left_closure", None, None, None),
                RpqPlan::RStar(_) => ("right_closure", None, None, None),
            };
            nodes.push(RpqMatrixPlanNode {
                id,
                operation,
                label,
                nnz,
                vertex,
                children: node
                    .children()
                    .iter()
                    .map(|child| usize::from(*child))
                    .collect(),
            });
        }
        Ok(RpqMatrixPlanExplanation {
            storage: match storage_for_query(query) {
                MatrixStorage::Csr => "csr",
                MatrixStorage::Csc => "csc",
            },
            expression: expr.to_string(),
            root: expr.len() - 1,
            nodes,
        })
    }
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
        let expr = self.optimized_expression(query, graph)?;

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
    use crate::graph::{Graph, InMemory};
    use crate::rpq::{Endpoint, PathExpr, RpqQuery};
    use crate::utils::build_graph;

    #[test]
    fn explain_keeps_bound_selector_storage_and_valid_dag() {
        let graph = build_graph(&[("A", "B", "p"), ("B", "C", "q")]);
        let query = RpqQuery {
            subject: Endpoint::Variable("x".into()),
            path: PathExpr::Sequence(
                Box::new(PathExpr::Label("p".into())),
                Box::new(PathExpr::ZeroOrMore(Box::new(PathExpr::Label("q".into())))),
            ),
            object: Endpoint::Named("C".into()),
        };
        for strategy in [
            OptimizationStrategy::NoOpt,
            OptimizationStrategy::PangHybrid,
        ] {
            let evaluator = RpqMatrixEvaluator::optimized(strategy);
            let plan = evaluator.explain(&query, &graph).unwrap();
            assert_eq!(plan.storage, "csc");
            assert_eq!(plan.root, plan.nodes.len() - 1);
            assert!(
                plan.nodes
                    .iter()
                    .any(|node| node.vertex.as_deref() == Some("C"))
            );
            assert!(
                plan.nodes
                    .iter()
                    .all(|node| node.children.iter().all(|child| *child < node.id))
            );
            assert_eq!(evaluator.evaluate(&query, &graph).unwrap().nnz, 1);
        }
        let unknown = RpqQuery {
            object: Endpoint::Named("missing".into()),
            ..query
        };
        assert!(matches!(
            RpqMatrixEvaluator::unoptimized().explain(&unknown, &graph),
            Err(RpqError::VertexNotFound(_))
        ));
    }

    #[test]
    fn explain_represents_shared_subexpressions_once() {
        let graph = build_graph(&[("A", "B", "p")]);
        let query = RpqQuery {
            subject: Endpoint::Variable("x".into()),
            path: PathExpr::Sequence(
                Box::new(PathExpr::Label("p".into())),
                Box::new(PathExpr::Label("p".into())),
            ),
            object: Endpoint::Variable("y".into()),
        };
        let plan = RpqMatrixEvaluator::optimized(OptimizationStrategy::PangHybrid)
            .explain(&query, &graph)
            .unwrap();
        assert_eq!(plan.storage, "csr");
        let labels: Vec<_> = plan
            .nodes
            .iter()
            .filter(|node| node.label.as_deref() == Some("p"))
            .collect();
        assert_eq!(labels.len(), 1);
        let label_id = labels[0].id;
        assert_eq!(
            plan.nodes
                .iter()
                .flat_map(|node| &node.children)
                .filter(|child| **child == label_id)
                .count(),
            2
        );
    }

    #[test]
    fn pang_hybrid_preserves_matrix_market_results() {
        let query = RpqQuery {
            subject: Endpoint::Variable("x".into()),
            path: PathExpr::ZeroOrMore(Box::new(PathExpr::Label("knows".into()))),
            object: Endpoint::Variable("y".into()),
        };
        let optimizer = OptimizationStrategy::PangHybrid;
        let graph =
            Graph::<InMemory>::try_from(MatrixMarket::from_dir("tests/testdata/mm_small")).unwrap();
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

        let optimizer = OptimizationStrategy::PangHybrid;
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
