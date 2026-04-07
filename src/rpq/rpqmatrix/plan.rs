use std::ptr::null_mut;

use egg::{Id, RecExpr};
use spargebra::algebra::PropertyPathExpression;

use super::optimizer::RpqPlan;
use crate::graph::{GraphDecomposition,GraphError};
use crate::lagraph_sys::*;
use crate::rpq::rpqmatrix::optimizer::LabelMeta;
use crate::rpq::{RpqError,};

/// Compile a [`PropertyPathExpression`] into  [`RecExpr<RpqPlan>`].
pub(super) fn to_expr<G: GraphDecomposition>(graph: &G, path: &PropertyPathExpression) -> Result<RecExpr<RpqPlan>, RpqError> {
    let mut expr = RecExpr::default();
    to_expr_aux(path, &mut expr,graph)?;
    Ok(expr)
}

pub(super) fn to_expr_aux<G: GraphDecomposition>(
    // 2) в граф добавить каталоги 
    path: &PropertyPathExpression,
    expr: &mut RecExpr<RpqPlan>,
    graph: &G, 
) -> Result<Id, RpqError> {
    match path {
        PropertyPathExpression::NamedNode(nn) => {
            let label =  nn.clone().into_string();
            let meta = (graph.get_meta(&label)).ok_or_else(|| RpqError::LabelNotFound(label.to_owned()))?;
            Ok(expr.add(RpqPlan::Label(LabelMeta{
                name: label,
                nvals: graph.num_nodes(),
                rreduce_nvals: meta.row_projections,
                creduce_nvals: meta.column_projections,
            })))
        }

        PropertyPathExpression::Sequence(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr, graph)?;
            let r = to_expr_aux(rhs, expr, graph)?;
            Ok(expr.add(RpqPlan::Seq([l, r])))
        }

        PropertyPathExpression::Alternative(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr, graph)?;
            let r = to_expr_aux(rhs, expr, graph)?;
            Ok(expr.add(RpqPlan::Alt([l, r])))
        }

        PropertyPathExpression::ZeroOrMore(inner) => {
            let i = to_expr_aux(inner, expr, graph)?;
            Ok(expr.add(RpqPlan::Star([i])))
        }

        PropertyPathExpression::OneOrMore(inner) => {
            let e = to_expr_aux(inner, expr, graph)?;
            let s = expr.add(RpqPlan::Star([e]));
            Ok(expr.add(RpqPlan::Seq([e, s])))
        }

        PropertyPathExpression::ZeroOrOne(_) => Err(RpqError::UnsupportedPath(
            "ZeroOrOne (?) is not supported by RPQMatrix".into(),
        )),

        PropertyPathExpression::Reverse(_) => Err(RpqError::UnsupportedPath(
            "Reverse paths are not supported".into(),
        )),

        PropertyPathExpression::NegatedPropertySet(_) => Err(RpqError::UnsupportedPath(
            "NegatedPropertySet paths are not supported".into(),
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
                    .get_graph(&label.name)
                    .map_err(|_| RpqError::LabelNotFound(label.name.clone()))?;
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
            RpqPlan::RStar([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE_R,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
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
        };
    }

    Ok(plans)
}
