use super::expr::{materialize_with_storage, query_to_expr};
use super::optimize::{OptimizationStrategy, optimize_expr_cardinality};
use super::result::{PreparedRpqMatrix, RpqMatrixResult};
/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
use crate::eval::Evaluator;
use crate::graph::{GraphDecomposition, MatrixStorage, set_global_matrix_storage_hint};
use crate::rpq::{Endpoint, RpqError, RpqQuery};

#[derive(Clone, Copy)]
pub struct RpqMatrixEvaluator {
    optimizer: OptimizationStrategy,
}

impl RpqMatrixEvaluator {
    pub fn unoptimized() -> Self {
        return RpqMatrixEvaluator {
            optimizer: OptimizationStrategy::NoOpt,
        };
    }
    pub fn optimized(opt: OptimizationStrategy) -> Self {
        return RpqMatrixEvaluator { optimizer: opt };
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
            OptimizationStrategy::Cardinality => {
                expr = optimize_expr_cardinality(expr, graph.num_nodes());
            }
            _ => todo!(),
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
}
