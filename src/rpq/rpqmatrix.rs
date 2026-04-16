//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

use std::ptr::null_mut;

use egg::{Id, RecExpr, define_language};

use crate::graph::{GraphDecomposition, GraphblasMatrix, ensure_grb_init};
use crate::lagraph_sys::*;
use crate::rpq::{Endpoint, PathExpr, RpqError, RpqEvaluator, RpqQuery};
use crate::{grb_ok, la_ok};

define_language! {
    pub enum RpqPlan {
        Label(String),
        NamedVertex(String),
        "/" = Seq([Id; 2]),
        "|" = Alt([Id; 2]),
        "*" = Star([Id; 1]),
    }
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

/// Compile a [`RpqQuery`]  into
/// [`RecExpr<RpqPlan>`].
pub fn query_to_expr(query: &RpqQuery) -> Result<RecExpr<RpqPlan>, RpqError> {
    let mut expr = RecExpr::default();
    let path_root = to_expr_aux(&query.path, &mut expr)?;

    let _root = match (&query.subject, &query.object) {
        (Endpoint::Variable(_), Endpoint::Variable(_)) => path_root,
        (Endpoint::Named(name), Endpoint::Variable(_)) => {
            let diag = expr.add(RpqPlan::NamedVertex(name.clone()));
            expr.add(RpqPlan::Seq([diag, path_root]))
        }
        (Endpoint::Variable(_), Endpoint::Named(name)) => {
            let diag = expr.add(RpqPlan::NamedVertex(name.clone()));
            expr.add(RpqPlan::Seq([path_root, diag]))
        }
        (Endpoint::Named(sub), Endpoint::Named(obj)) => {
            let diag_sub = expr.add(RpqPlan::NamedVertex(sub.clone()));
            let seq1 = expr.add(RpqPlan::Seq([diag_sub, path_root]));
            let diag_obj = expr.add(RpqPlan::NamedVertex(obj.clone()));
            expr.add(RpqPlan::Seq([seq1, diag_obj]))
        }
    };

    Ok(expr)
}

/// Convert a [`RecExpr<RpqPlan>`] into the flat [`RPQMatrixPlan`] array that
/// `LAGraph_RPQMatrix` expects.
///
/// Returns the plan array and a list of owned diagonal matrices that must be
/// freed after evaluation.
pub fn materialize<G: GraphDecomposition>(
    expr: &RecExpr<RpqPlan>,
    graph: &G,
) -> Result<(Vec<RPQMatrixPlan>, Vec<GrB_Matrix>), RpqError> {
    let null_plan = RPQMatrixPlan {
        op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
        lhs: null_mut(),
        rhs: null_mut(),
        mat: null_mut(),
        res_mat: null_mut(),
    };
    let mut plans = vec![null_plan; expr.len()];
    let mut owned_matrices: Vec<GrB_Matrix> = Vec::new();
    let n = graph.num_nodes() as GrB_Index;

    for (id, node) in expr.as_ref().iter().enumerate() {
        plans[id] = match node {
            RpqPlan::Label(label) => {
                let lg = graph.get_graph(label)?;
                let mat = unsafe { (*lg.inner).A };
                RPQMatrixPlan {
                    op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
                    lhs: null_mut(),
                    rhs: null_mut(),
                    mat,
                    res_mat: null_mut(),
                }
            }

            RpqPlan::NamedVertex(name) => {
                let vertex_id = graph
                    .get_node_id(name)
                    .ok_or_else(|| RpqError::VertexNotFound(name.clone()))?
                    as GrB_Index;
                let mut mat: GrB_Matrix = null_mut();
                grb_ok!(LAGraph_RPQMatrix_label(&mut mat, vertex_id, n, n,))?;
                if mat.is_null() {
                    return Err(RpqError::Graph(crate::graph::GraphError::GraphBlas(
                        GrB_Info::GrB_INVALID_VALUE,
                    )));
                }
                owned_matrices.push(mat);
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

    Ok((plans, owned_matrices))
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
        ensure_grb_init()?;

        let expr = query_to_expr(query)?;
        let (mut plans, owned_matrices) = materialize(&expr, graph)?;

        let root_ptr = unsafe { plans.as_mut_ptr().add(plans.len() - 1) };

        let mut nnz: GrB_Index = 0;
        la_ok!(LAGraph_RPQMatrix(&mut nnz, root_ptr))?;

        let matrix = unsafe {
            let mat = (*root_ptr).res_mat;
            (*root_ptr).res_mat = null_mut();
            GraphblasMatrix { inner: mat }
        };

        grb_ok!(LAGraph_DestroyRpqMatrixPlan(root_ptr))?;

        // Free diagonal matrices created for named vertices.
        for mut mat in owned_matrices {
            unsafe {
                LAGraph_RPQMatrix_Free(&mut mat);
            }
        }

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
        let result = RpqMatrixEvaluator.evaluate(&q, &graph).expect("evaluate");
        assert_eq!(result.nnz, 0, "C has no outgoing p edges");
    }
}
