//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

use crate::graph::GraphDecomposition;
use crate::lagraph_sys::*;
use crate::rpq::RpqError;
use spargebra::algebra::PropertyPathExpression;

unsafe impl Send for RPQMatrixPlan {}

/// Build a flat `Vec<RpqMatrixPlan>` from a [`PropertyPathExpression`].
///
/// Label nodes have their `mat` field set to the corresponding `GrB_Matrix`
/// from `graph`. All other nodes have `mat = null`.
///
/// The root of the plan tree is the **last** element of the returned `Vec`.
///
/// # Errors
///
/// - [`RpqError::LabelNotFound`] if a label IRI is not present in `graph`.
/// - [`RpqError::UnsupportedPath`] for `Reverse` and `NegatedPropertySet`.
pub fn build_plans<G: GraphDecomposition>(
    path: &PropertyPathExpression,
    graph: &G,
) -> Result<Vec<RPQMatrixPlan>, RpqError> {
    let mut plans: Vec<RPQMatrixPlan> = Vec::new();
    build_recursive(path, graph, &mut plans)?;
    Ok(plans)
}

fn build_recursive<G: GraphDecomposition>(
    path: &PropertyPathExpression,
    graph: &G,
    plans: &mut Vec<RPQMatrixPlan>,
) -> Result<usize, RpqError> {
    match path {
        PropertyPathExpression::NamedNode(nn) => {
            let label = nn.as_str();
            let lg = graph
                .get_graph(label)
                .map_err(|_| RpqError::LabelNotFound(label.to_owned()))?;
            let mat = unsafe { (*lg.inner).A };
            let idx = plans.len();
            plans.push(RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
                lhs: std::ptr::null_mut(),
                rhs: std::ptr::null_mut(),
                mat,
                res_mat: std::ptr::null_mut(),
            });
            Ok(idx)
        }

        PropertyPathExpression::Sequence(lhs, rhs) => {
            let l = build_recursive(lhs, graph, plans)?;
            let r = build_recursive(rhs, graph, plans)?;
            let idx = plans.len();

            let lhs_ptr = unsafe { plans.as_mut_ptr().add(l) };
            let rhs_ptr = unsafe { plans.as_mut_ptr().add(r) };

            plans.push(RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_CONCAT,
                lhs: lhs_ptr,
                rhs: rhs_ptr,
                mat: std::ptr::null_mut(),
                res_mat: std::ptr::null_mut(),
            });
            Ok(idx)
        }

        PropertyPathExpression::Alternative(lhs, rhs) => {
            let l = build_recursive(lhs, graph, plans)?;
            let r = build_recursive(rhs, graph, plans)?;
            let idx = plans.len();
            let lhs_ptr = unsafe { plans.as_mut_ptr().add(l) };
            let rhs_ptr = unsafe { plans.as_mut_ptr().add(r) };
            plans.push(RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_LOR,
                lhs: lhs_ptr,
                rhs: rhs_ptr,
                mat: std::ptr::null_mut(),
                res_mat: std::ptr::null_mut(),
            });
            Ok(idx)
        }

        PropertyPathExpression::ZeroOrMore(inner) => {
            let r = build_recursive(inner, graph, plans)?;
            let idx = plans.len();
            let rhs_ptr = unsafe { plans.as_mut_ptr().add(r) };
            plans.push(RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE,
                lhs: std::ptr::null_mut(),
                rhs: rhs_ptr,
                mat: std::ptr::null_mut(),
                res_mat: std::ptr::null_mut(),
            });
            Ok(idx)
        }

        PropertyPathExpression::OneOrMore(inner) => {
            // Plus(e) = Seq(e, Star(e)) — build e once, share via index
            let e = build_recursive(inner, graph, plans)?;
            let star_idx = plans.len();
            let e_ptr_for_star = unsafe { plans.as_mut_ptr().add(e) };
            plans.push(RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE,
                lhs: std::ptr::null_mut(),
                rhs: e_ptr_for_star,
                mat: std::ptr::null_mut(),
                res_mat: std::ptr::null_mut(),
            });
            let idx = plans.len();
            let e_ptr = unsafe { plans.as_mut_ptr().add(e) };
            let star_ptr = unsafe { plans.as_mut_ptr().add(star_idx) };
            plans.push(RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_CONCAT,
                lhs: e_ptr,
                rhs: star_ptr,
                mat: std::ptr::null_mut(),
                res_mat: std::ptr::null_mut(),
            });
            Ok(idx)
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
