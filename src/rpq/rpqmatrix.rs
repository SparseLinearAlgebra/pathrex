//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

use std::ptr::null_mut;

use egg::{Id, RecExpr, define_language};

use crate::graph::{GraphDecomposition, GraphblasMatrix, ensure_grb_init};
use crate::lagraph_sys::*;
use crate::rpq::{Endpoint, PathExpr, RpqError, RpqEvaluator, RpqQuery};
use crate::{grb_ok, la_ok};

unsafe impl Send for RPQMatrixPlan {}

define_language! {
    pub enum RpqPlan {
        Label(String),
        "/" = Seq([Id; 2]),
        "|" = Alt([Id; 2]),
        "*" = Star([Id; 1]),
    }
}

/// Compile a [`PathExpr`] into [`RecExpr<RpqPlan>`].
pub fn to_expr(path: &PathExpr) -> Result<RecExpr<RpqPlan>, RpqError> {
    let mut expr = RecExpr::default();
    to_expr_aux(path, &mut expr)?;
    Ok(expr)
}

fn to_expr_aux(path: &PathExpr, expr: &mut RecExpr<RpqPlan>) -> Result<Id, RpqError> {
    match path {
        PathExpr::Label(label) => Ok(expr.add(RpqPlan::Label(label.clone()))),

        PathExpr::Sequence(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr)?;
            let r = to_expr_aux(rhs, expr)?;
            Ok(expr.add(RpqPlan::Seq([l, r])))
        }

        PathExpr::Alternative(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr)?;
            let r = to_expr_aux(rhs, expr)?;
            Ok(expr.add(RpqPlan::Alt([l, r])))
        }

        PathExpr::ZeroOrMore(inner) => {
            let i = to_expr_aux(inner, expr)?;
            Ok(expr.add(RpqPlan::Star([i])))
        }

        PathExpr::OneOrMore(inner) => {
            let e = to_expr_aux(inner, expr)?;
            let s = expr.add(RpqPlan::Star([e]));
            Ok(expr.add(RpqPlan::Seq([e, s])))
        }

        PathExpr::ZeroOrOne(_) => Err(RpqError::UnsupportedPath(
            "ZeroOrOne (?) is not supported by RPQMatrix".into(),
        )),
    }
}

/// Convert a [`RecExpr<RpqPlan>`] into the flat [`RPQMatrixPlan`] array that
/// `LAGraph_RPQMatrix` expects.
pub fn materialize<G: GraphDecomposition>(
    expr: &RecExpr<RpqPlan>,
    graph: &G,
) -> Result<Vec<RPQMatrixPlan>, RpqError> {
    let null_plan = RPQMatrixPlan {
        op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
        lhs: null_mut(),
        rhs: null_mut(),
        mat: null_mut(),
        res_mat: null_mut(),
    };
    let mut plans = vec![null_plan; expr.len()];

    for (id, node) in expr.as_ref().iter().enumerate() {
        plans[id] = match node {
            RpqPlan::Label(label) => {
                let lg = graph
                    .get_graph(label)
                    .map_err(|_| RpqError::LabelNotFound(label.clone()))?;
                let mat = unsafe { (*lg.inner).A };
                RPQMatrixPlan {
                    op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
                    lhs: null_mut(),
                    rhs: null_mut(),
                    mat,
                    res_mat: null_mut(),
                }
            }

            RpqPlan::Seq([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_CONCAT,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::Alt([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_LOR,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::Star([i]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE,
                lhs: null_mut(),
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*i)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },
        };
    }

    Ok(plans)
}

/// Output of [`RpqMatrixEvaluator`]: full path relation matrix and its nnz.
#[derive(Debug)]
pub struct RpqMatrixResult {
    pub nnz: u64,
    pub matrix: GraphblasMatrix,
}

/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
pub struct RpqMatrixEvaluator;

impl RpqEvaluator for RpqMatrixEvaluator {
    type Result = RpqMatrixResult;

    fn evaluate<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<RpqMatrixResult, RpqError> {
        if !matches!(query.object, Endpoint::Variable(_)) {
            return Err(RpqError::UnsupportedPath(
                "bound object term is not yet supported by RpqMatrixEvaluator".into(),
            ));
        }

        if let Endpoint::Named(id) = &query.subject {
            graph
                .get_node_id(id)
                .ok_or_else(|| RpqError::VertexNotFound(id.clone()))?;
        }

        ensure_grb_init().map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        let expr = to_expr(&query.path)?;

        let mut plans = materialize(&expr, graph)?;
        let root_ptr = unsafe { plans.as_mut_ptr().add(plans.len() - 1) };

        let mut nnz: GrB_Index = 0;
        la_ok!(LAGraph_RPQMatrix(&mut nnz, root_ptr))
            .map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        let matrix = unsafe {
            let mat = (*root_ptr).res_mat;
            (*root_ptr).res_mat = null_mut();
            GraphblasMatrix { inner: mat }
        };

        grb_ok!(LAGraph_DestroyRpqMatrixPlan(root_ptr))
            .map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        Ok(RpqMatrixResult {
            nnz: nnz as u64,
            matrix,
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
        let result = RpqMatrixEvaluator.evaluate(&q, &graph).expect("evaluate");
        assert_eq!(result.nnz, 1);
    }
}
