use std::ptr::null_mut;

use crate::eval::{PreparedEvaluator, ResultCount};
use crate::graph::wrappers::ReduceType::ByCols;
use crate::graph::{GraphError, GraphblasMatrix};
use crate::lagraph_sys::*;

use crate::rpq::RpqError;
use crate::{grb_ok, la_ok};

/// Output of [`RpqMatrixEvaluator`]: full path relation matrix and its nnz.
#[derive(Debug)]
pub struct RpqMatrixResult {
    pub nnz: u64,
    pub matrix: GraphblasMatrix,
}

impl RpqMatrixResult {
    /// Count distinct reachable target vertices by reducing the path relation
    /// matrix to its non-empty columns.
    pub fn reachable_target_count(&self) -> Result<u64, crate::graph::GraphError> {
        let mut count: GrB_Index = 0;
        unsafe {
            grb_ok!(LAGraph_RPQMatrix_reduce(
                &mut count,
                self.matrix.inner,
                ByCols as u8,
            ))?
        };
        Ok(count as u64)
    }
}

impl ResultCount for RpqMatrixResult {
    fn result_count(&self) -> Result<usize, GraphError> {
        Ok(self.reachable_target_count()? as usize)
    }
}

pub struct PreparedRpqMatrix {
    pub(super) plans: Vec<RPQMatrixPlan>,
    pub(super) owned_matrices: Vec<GrB_Matrix>,
}

impl PreparedEvaluator for PreparedRpqMatrix {
    type Result = RpqMatrixResult;
    type Error = RpqError;

    fn execute(&mut self) -> Result<RpqMatrixResult, RpqError> {
        let root_ptr = unsafe { self.plans.as_mut_ptr().add(self.plans.len() - 1) };

        let mut nnz: GrB_Index = 0;
        unsafe { la_ok!(LAGraph_RPQMatrix(&mut nnz, root_ptr))? };

        let mut matrix_inner: GrB_Matrix = null_mut();
        unsafe { grb_ok!(GrB_Matrix_dup(&mut matrix_inner, (*root_ptr).res_mat))? };
        let matrix = GraphblasMatrix {
            inner: matrix_inner,
        };

        unsafe { grb_ok!(LAGraph_DestroyRpqMatrixPlan(root_ptr))? };

        Ok(RpqMatrixResult {
            nnz: nnz as u64,
            matrix,
        })
    }
}

impl Drop for PreparedRpqMatrix {
    fn drop(&mut self) {
        for mat in &mut self.owned_matrices {
            unsafe {
                LAGraph_RPQMatrix_Free(mat);
            }
        }
    }
}
