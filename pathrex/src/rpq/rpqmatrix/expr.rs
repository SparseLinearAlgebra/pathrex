use std::ptr::null_mut;

use egg::{Id, RecExpr};

use super::plan::{LabelMeta, RpqPlan};
use crate::graph::GraphDecomposition;
use crate::grb_ok;
use crate::lagraph_sys::*;
use crate::rpq::{Endpoint, PathExpr, RpqError, RpqQuery};

fn label_meta<G: GraphDecomposition>(label: &str, graph: &G) -> Result<LabelMeta, RpqError> {
    if let Some(metadata) = graph.get_metadata().and_then(|m| m.matrix(label)) {
        return Ok(LabelMeta {
            name: label.to_owned(),
            nvals: metadata.nvals,
            nonzero_rows: metadata.nonzero_rows,
            nonzero_cols: metadata.nonzero_cols,
        });
    }

    // TODO: maybe create optimized (for mm format) and nonoptimized (for other formats) plans
    let lg = graph.get_graph(label)?;
    let nvals = lg.nvals()? as usize;
    Ok(LabelMeta {
        name: label.to_owned(),
        nvals,
        nonzero_rows: nvals,
        nonzero_cols: nvals,
    })
}

fn to_expr_aux<G: GraphDecomposition>(
    path: &PathExpr,
    expr: &mut RecExpr<RpqPlan>,
    graph: &G,
) -> Result<Id, RpqError> {
    match path {
        PathExpr::Label(label) => Ok(expr.add(RpqPlan::Label(label_meta(label, graph)?))),

        PathExpr::Sequence(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr, graph)?;
            let r = to_expr_aux(rhs, expr, graph)?;
            Ok(expr.add(RpqPlan::Seq([l, r])))
        }

        PathExpr::Alternative(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr, graph)?;
            let r = to_expr_aux(rhs, expr, graph)?;
            Ok(expr.add(RpqPlan::Alt([l, r])))
        }

        PathExpr::ZeroOrMore(inner) => {
            let i = to_expr_aux(inner, expr, graph)?;
            Ok(expr.add(RpqPlan::Star([i])))
        }

        PathExpr::OneOrMore(inner) => {
            let e = to_expr_aux(inner, expr, graph)?;
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
pub fn query_to_expr<G: GraphDecomposition>(
    query: &RpqQuery,
    graph: &G,
) -> Result<RecExpr<RpqPlan>, RpqError> {
    let mut expr = RecExpr::default();
    let path_root = to_expr_aux(&query.path, &mut expr, graph)?;

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
                let lg = graph.get_graph(&label.name)?;
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
                unsafe {
                    crate::graph::ensure_grb_init()?;
                    grb_ok!(LAGraph_RPQMatrix_label(&mut mat, vertex_id, n, n,))?
                };
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

            RpqPlan::LStar([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE_L,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::RStar([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE_R,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },
        };
    }

    Ok((plans, owned_matrices))
}
