//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

use std::ptr::null_mut;

use egg::{Id, RecExpr, define_language};
use spargebra::algebra::PropertyPathExpression;
use spargebra::term::TermPattern;

use crate::graph::{GraphDecomposition, GraphblasVector, ensure_grb_init};
use crate::lagraph_sys::*;
use crate::rpq::{RpqError, RpqEvaluator, RpqResult};
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

/// Compile a [`PropertyPathExpression`] into  [`RecExpr<RpqPlan>`].
pub fn to_expr(path: &PropertyPathExpression) -> Result<RecExpr<RpqPlan>, RpqError> {
    let mut expr = RecExpr::default();
    to_expr_aux(path, &mut expr)?;
    Ok(expr)
}

fn to_expr_aux(
    path: &PropertyPathExpression,
    expr: &mut RecExpr<RpqPlan>,
) -> Result<Id, RpqError> {
    match path {
        PropertyPathExpression::NamedNode(nn) => {
            Ok(expr.add(RpqPlan::Label(nn.as_str().to_owned())))
        }

        PropertyPathExpression::Sequence(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr)?;
            let r = to_expr_aux(rhs, expr)?;
            Ok(expr.add(RpqPlan::Seq([l, r])))
        }

        PropertyPathExpression::Alternative(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr)?;
            let r = to_expr_aux(rhs, expr)?;
            Ok(expr.add(RpqPlan::Alt([l, r])))
        }

        PropertyPathExpression::ZeroOrMore(inner) => {
            let i = to_expr_aux(inner, expr)?;
            Ok(expr.add(RpqPlan::Star([i])))
        }

        PropertyPathExpression::OneOrMore(inner) => {
            let e = to_expr_aux(inner, expr)?;
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

/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
pub struct RpqMatrixEvaluator;

impl RpqEvaluator for RpqMatrixEvaluator {
    fn evaluate<G: GraphDecomposition>(
        &self,
        subject: &TermPattern,
        path: &PropertyPathExpression,
        object: &TermPattern,
        graph: &G,
    ) -> Result<RpqResult, RpqError> {
        if !matches!(object, TermPattern::Variable(_)) {
            return Err(RpqError::UnsupportedPath(
                "bound object term is not yet supported by RpqMatrixEvaluator".into(),
            ));
        }

        ensure_grb_init().map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        let n = graph.num_nodes() as GrB_Index;

        let expr = to_expr(path)?;

        let mut plans = materialize(&expr, graph)?;
        let root_ptr = unsafe { plans.as_mut_ptr().add(plans.len() - 1) };

        let mut nnz: GrB_Index = 0;
        la_ok!(LAGraph_RPQMatrix(&mut nnz, root_ptr))
            .map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        let res_mat = unsafe { (*root_ptr).res_mat };

        let src = unsafe {
            GraphblasVector::new_bool(n).map_err(|e| RpqError::GraphBlas(e.to_string()))?
        };
        match subject {
            TermPattern::NamedNode(nn) => {
                let id = graph
                    .get_node_id(nn.as_str())
                    .ok_or_else(|| RpqError::VertexNotFound(nn.as_str().to_owned()))?
                    as GrB_Index;
                grb_ok!(GrB_Vector_setElement_BOOL(src.inner, true, id))
                    .map_err(|e| RpqError::GraphBlas(e.to_string()))?;
            }
            TermPattern::Variable(_) => {
                for i in 0..n {
                    grb_ok!(GrB_Vector_setElement_BOOL(src.inner, true, i))
                        .map_err(|e| RpqError::GraphBlas(e.to_string()))?;
                }
            }
            _ => {
                return Err(RpqError::UnsupportedPath(
                    "subject must be a variable or named node".into(),
                ));
            }
        }

        let result = unsafe {
            GraphblasVector::new_bool(n).map_err(|e| RpqError::GraphBlas(e.to_string()))?
        };
        grb_ok!(GrB_vxm(
            result.inner,
            null_mut(),
            null_mut(),
            GrB_LOR_LAND_SEMIRING_BOOL,
            src.inner,
            res_mat,
            null_mut(),
        ))
        .map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        grb_ok!(LAGraph_DestroyRpqMatrixPlan(root_ptr))
            .map_err(|e| RpqError::GraphBlas(e.to_string()))?;

        Ok(RpqResult { reachable: result })
    }
}
