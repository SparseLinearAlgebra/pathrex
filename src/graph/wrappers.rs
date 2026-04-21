//! RAII wrappers and init helpers for GraphBLAS and LAGraph C handles.
//!
//! GraphBLAS initialisation is performed lazily inside the RAII-wrapped constructors
//! here (`LagraphGraph::from_coo`, `LagraphGraph::from_matrix`) and inside
//! `ThreadScope::enter`. Consumers of these wrappers — including format loaders,
//! builders, and RPQ evaluators — do not need (and should not) call init themselves.

use std::ffi::CString;
use std::fs::File;
use std::os::fd::IntoRawFd;
use std::path::Path;
use std::sync::Once;

use crate::{grb_ok, la_ok, lagraph_sys::*};

use super::GraphError;

static GRB_INIT: Once = Once::new();

pub(crate) fn ensure_grb_init() -> Result<(), GraphError> {
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
    /// Build a `LagraphGraph` from an RAII-wrapped [`GraphblasMatrix`].
    ///
    /// On success, ownership of the underlying `GrB_Matrix` is transferred
    /// into the `LAGraph_Graph` and the [`GraphblasMatrix`] guard is forgotten
    ///
    /// On failure, the [`GraphblasMatrix`] is dropped normally, freeing the
    /// matrix.
    pub fn from_matrix(matrix: GraphblasMatrix, kind: LAGraph_Kind) -> Result<Self, GraphError> {
        ensure_grb_init()?;
        let mut raw = matrix.inner;
        let mut g: LAGraph_Graph = std::ptr::null_mut();
        match unsafe { la_ok!(LAGraph_New(&mut g, &mut raw, kind)) } {
            Ok(()) => {
                std::mem::forget(matrix);
                Ok(Self { inner: g })
            }
            Err(e) => Err(e),
        }
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
    /// # Example
    /// ```ignore
    /// let rows = vec![0, 1, 2];
    /// let cols = vec![1, 2, 0];
    /// let vals = vec![true, true, true];
    /// let graph = LagraphGraph::from_coo(&rows, &cols, &vals, 3, LAGraph_ADJACENCY_DIRECTED)?;
    /// ```
    pub fn from_coo(
        rows: &[GrB_Index],
        cols: &[GrB_Index],
        vals: &[bool],
        n: GrB_Index,
        kind: LAGraph_Kind,
    ) -> Result<Self, GraphError> {
        ensure_grb_init()?;
        let nvals = rows.len() as GrB_Index;

        let mut matrix: GrB_Matrix = std::ptr::null_mut();
        unsafe { grb_ok!(GrB_Matrix_new(&mut matrix, GrB_BOOL, n, n))? };

        let owned = GraphblasMatrix::from_raw(matrix);

        grb_ok!(unsafe {
            GrB_Matrix_build_BOOL(
                owned.inner,
                rows.as_ptr(),
                cols.as_ptr(),
                vals.as_ptr(),
                nvals,
                GrB_LOR,
            )
        })?;

        Self::from_matrix(owned, kind)
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

        unsafe {
            grb_ok!(GrB_Vector_extractTuples_BOOL(
                indices.as_mut_ptr(),
                values.as_mut_ptr(),
                &mut actual_nvals,
                self.inner,
            ))?
        };

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

impl GraphblasMatrix {
    /// Wrap a raw [`GrB_Matrix`] pointer in an RAII guard.
    ///
    /// The caller must ensure the pointer is either null or a valid,
    /// live `GrB_Matrix` that is not shared with any other owner.
    /// [`Drop`] will call `GrB_Matrix_free` when the guard is dropped.
    pub fn from_raw(raw: GrB_Matrix) -> Self {
        Self { inner: raw }
    }
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

/// Read a single MatrixMarket file and return a RAII-wrapped [`GraphblasMatrix`].
///
/// Initialises GraphBLAS on first call. The file must be in MatrixMarket
/// coordinate format as produced by LAGraph.
pub fn load_mm_file(path: impl AsRef<Path>) -> Result<GraphblasMatrix, GraphError> {
    ensure_grb_init()?;

    let file = File::open(path.as_ref())
        .map_err(|e| GraphError::Format(crate::formats::FormatError::Io(e)))?;
    let fd = file.into_raw_fd();

    let c_mode = CString::new("r").unwrap();
    let f = unsafe { libc::fdopen(fd, c_mode.as_ptr()) };
    if f.is_null() {
        unsafe { libc::close(fd) };
        return Err(GraphError::Format(crate::formats::FormatError::Io(
            std::io::Error::last_os_error(),
        )));
    }

    let mut matrix = std::ptr::null_mut();
    let err = unsafe { la_ok!(LAGraph_MMRead(&mut matrix, f as *mut FILE)) };
    unsafe { libc::fclose(f) };
    err?;

    Ok(GraphblasMatrix::from_raw(matrix))
}
