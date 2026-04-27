//! RAII wrappers and init helpers for GraphBLAS and LAGraph C handles.

use std::sync::Once;

use crate::{grb_ok, la_ok, lagraph_sys::*};

use super::GraphError;

static GRB_INIT: Once = Once::new();

pub fn ensure_grb_init() -> Result<(), GraphError> {
    let mut result = Ok(());
    GRB_INIT.call_once(|| {
        result = unsafe { la_ok!(LAGraph_Init()) };
    });
    result
}

/// Compute a balanced `(outer, inner)` split for LAGraph's two-level threading.
///
/// `outer` is the number of user-level concurrent tasks (rayon workers);
/// `inner` is the number of GraphBLAS/OpenMP threads per task. The product is
/// kept close to `available_parallelism()` so the OS scheduler does not
/// thrash.
pub(crate) fn compute_outer_inner(num_tasks: usize) -> (i32, i32) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let tasks = num_tasks.max(1);
    let outer = tasks.min(cores).max(1);
    let inner = (cores / outer).max(1);
    (outer as i32, inner as i32)
}

/// RAII guard that temporarily sets LAGraph's `(outer, inner)` thread counts.
///
/// On entry calls `LAGraph_SetNumThreads(outer, inner)`. On drop restores
/// `(1, available_parallelism())` so subsequent callers
/// keep full GraphBLAS parallelism.
pub(crate) struct ThreadScope {
    _private: (),
}

impl ThreadScope {
    pub(crate) fn enter(outer: i32, inner: i32) -> Result<Self, GraphError> {
        ensure_grb_init()?;
        unsafe { la_ok!(LAGraph_SetNumThreads(outer, inner))? };
        Ok(Self { _private: () })
    }
}

impl Drop for ThreadScope {
    fn drop(&mut self) {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as i32;
        if let Err(e) = unsafe { la_ok!(LAGraph_SetNumThreads(1, cores)) } {
            eprintln!("ThreadScope: failed to restore thread counts: {e}");
        }
    }
}

#[derive(Debug)]
pub struct LagraphGraph {
    pub(crate) inner: LAGraph_Graph,
}

impl LagraphGraph {
    pub fn new(mut matrix: GrB_Matrix, kind: LAGraph_Kind) -> Result<Self, GraphError> {
        let mut g: LAGraph_Graph = std::ptr::null_mut();
        unsafe { la_ok!(LAGraph_New(&mut g, &mut matrix, kind,))? };

        Ok(Self { inner: g })
    }

    /// Build a new `LagraphGraph` from coordinate (COO) format.
    ///
    /// Creates a boolean adjacency matrix from parallel arrays of row indices,
    /// column indices, and boolean values, then wraps it in an `LAGraph_Graph`.
    ///
    /// # Parameters
    /// - `rows`: Row indices
    /// - `cols`: Column indices
    /// - `vals`: Boolean values for each edge
    /// - `n`: Number of nodes
    /// - `kind`: Graph kind (e.g., `LAGraph_ADJACENCY_DIRECTED`)
    ///
    /// # Safety
    /// Caller must ensure LAGraph/GraphBLAS has been initialised via
    /// [`ensure_grb_init`].
    ///
    /// # Example
    /// ```ignore
    /// let rows = vec![0, 1, 2];
    /// let cols = vec![1, 2, 0];
    /// let vals = vec![true, true, true];
    /// let graph = unsafe {
    ///     LagraphGraph::from_coo(&rows, &cols, &vals, 3, LAGraph_ADJACENCY_DIRECTED)
    /// }?;
    /// ```
    pub fn from_coo(
        rows: &[GrB_Index],
        cols: &[GrB_Index],
        vals: &[bool],
        n: GrB_Index,
        kind: LAGraph_Kind,
    ) -> Result<Self, GraphError> {
        let nvals = rows.len() as GrB_Index;

        let mut matrix: GrB_Matrix = std::ptr::null_mut();
        unsafe { grb_ok!(GrB_Matrix_new(&mut matrix, GrB_BOOL, n, n))? };

        if let Err(e) = grb_ok!(unsafe { GrB_Matrix_build_BOOL(
            matrix,
            rows.as_ptr(),
            cols.as_ptr(),
            vals.as_ptr(),
            nvals,
            GrB_LOR,
        ) }) {
            let _ = unsafe { grb_ok!(GrB_Matrix_free(&mut matrix)) };
            return Err(e);
        }

        Self::new(matrix, kind)
    }

    pub fn check_graph(&self) -> Result<(), GraphError> {
        unsafe { la_ok!(LAGraph_CheckGraph(self.inner)) }
    }

    /// Number of stored (non-zero) values in the underlying adjacency matrix.
    pub fn nvals(&self) -> Result<GrB_Index, GraphError> {
        if self.inner.is_null() {
            return Ok(0);
        }
        let matrix: GrB_Matrix = unsafe { (*self.inner).A };
        let mut nvals: GrB_Index = 0;
        unsafe { grb_ok!(GrB_Matrix_nvals(&mut nvals, matrix))? };
        Ok(nvals)
    }
}

impl Drop for LagraphGraph {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            let _ = unsafe { la_ok!(LAGraph_Delete(&mut self.inner)) };
        }
    }
}

unsafe impl Send for LagraphGraph {}
unsafe impl Sync for LagraphGraph {}

#[derive(Debug)]
pub struct GraphblasVector {
    pub inner: GrB_Vector,
}

impl GraphblasVector {
    /// Allocate a new N-element boolean `GrB_Vector`.
    ///
    /// # Safety
    /// Caller must ensure LAGraph/GraphBLAS has been initialised via
    /// [`ensure_grb_init`].
    pub fn new_bool(n: GrB_Index) -> Result<Self, GraphError> {
        let mut v: GrB_Vector = std::ptr::null_mut();
        unsafe { grb_ok!(GrB_Vector_new(&mut v, GrB_BOOL, n))? };
        Ok(Self { inner: v })
    }

    /// Returns the number of stored values in this vector.
    pub fn nvals(&self) -> Result<GrB_Index, GraphError> {
        let mut nvals: GrB_Index = 0;
        unsafe { grb_ok!(GrB_Vector_nvals(&mut nvals, self.inner))? };
        Ok(nvals)
    }

    /// Extracts all stored indices from boolean vector.
    pub fn indices(&self) -> Result<Vec<GrB_Index>, GraphError> {
        let nvals = self.nvals()?;
        if nvals == 0 {
            return Ok(Vec::new());
        }

        let mut indices = vec![0u64; nvals as usize];
        let mut values = vec![false; nvals as usize];
        let mut actual_nvals = nvals;

        unsafe { grb_ok!(GrB_Vector_extractTuples_BOOL(
            indices.as_mut_ptr(),
            values.as_mut_ptr(),
            &mut actual_nvals,
            self.inner,
        ))? };

        indices.truncate(actual_nvals as usize);
        Ok(indices)
    }
}

impl Drop for GraphblasVector {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            let _ = unsafe { grb_ok!(GrB_Vector_free(&mut self.inner)) };
        }
    }
}

unsafe impl Send for GraphblasVector {}
unsafe impl Sync for GraphblasVector {}

#[derive(Debug)]
pub struct GraphblasMatrix {
    pub inner: GrB_Matrix,
}

impl Drop for GraphblasMatrix {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            let _ = unsafe { grb_ok!(GrB_Matrix_free(&mut self.inner)) };
        }
    }
}

unsafe impl Send for GraphblasMatrix {}
unsafe impl Sync for GraphblasMatrix {}
