//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

use std::ptr::null_mut;

use spargebra::algebra::PropertyPathExpression;
use spargebra::term::TermPattern;

use super::plan::{materialize, to_expr};
use crate::graph::{GraphDecomposition, GraphblasVector, ensure_grb_init};
use crate::lagraph_sys::*;
use crate::rpq::{RpqError, RpqEvaluator, RpqResult};
use crate::{grb_ok, la_ok};

unsafe impl Send for RPQMatrixPlan {}

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

        let expr = to_expr(graph, path)?;

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
