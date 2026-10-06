use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::graph::{GraphblasMatrix, GraphblasVector};
use crate::lagraph_sys::{
    GrB_Info, GrB_Matrix, GrB_Vector, GrB_Vector_free, GrB_Vector_nvals,
    LAGraph_RPQMatrix_count_vector_dot, LAGraph_RPQMatrix_count_vector_mnc_add,
    LAGraph_RPQMatrix_count_vector_mnc_matmul_nnz, LAGraph_RPQMatrix_count_vector_scale,
    LAGraph_RPQMatrix_count_vector_sum, LAGraph_RPQMatrix_extended_count_vectors,
    LAGraph_RPQMatrix_label, LAGraph_RPQMatrix_reduce_count_vector,
};

static NEXT_VECTOR_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Debug)]
pub(crate) struct CountVector {
    handle: Arc<GraphblasVector>,
    nvals: usize,
    sum: f64,
    id: usize,
}

impl CountVector {
    fn from_matrix(matrix: GrB_Matrix, by_columns: bool) -> Option<Self> {
        let mut vector = std::ptr::null_mut();
        let code = unsafe {
            LAGraph_RPQMatrix_reduce_count_vector(&mut vector, matrix, u8::from(by_columns))
        };
        (code == GrB_Info::GrB_SUCCESS)
            .then(|| Self::from_owned(vector))
            .flatten()
    }

    fn from_owned(mut vector: GrB_Vector) -> Option<Self> {
        let mut nvals = 0;
        let mut sum = 0.0;
        let ok = unsafe {
            GrB_Vector_nvals(&mut nvals, vector) == GrB_Info::GrB_SUCCESS
                && LAGraph_RPQMatrix_count_vector_sum(&mut sum, vector) == GrB_Info::GrB_SUCCESS
        };
        if ok {
            Some(Self {
                handle: Arc::new(GraphblasVector { inner: vector }),
                nvals: nvals as usize,
                sum,
                id: NEXT_VECTOR_ID.fetch_add(1, Ordering::Relaxed),
            })
        } else {
            unsafe { GrB_Vector_free(&mut vector) };
            None
        }
    }

    fn raw(&self) -> GrB_Vector {
        self.handle.inner
    }

    pub(super) fn cache_key(&self) -> usize {
        self.id
    }

    pub(super) fn sum(&self) -> f64 {
        self.sum
    }

    pub(super) fn dot(&self, other: &Self) -> Option<f64> {
        let mut result = 0.0;
        let code =
            unsafe { LAGraph_RPQMatrix_count_vector_dot(&mut result, self.raw(), other.raw()) };
        (code == GrB_Info::GrB_SUCCESS).then_some(result)
    }

    pub(super) fn mnc_matmul_nnz(
        lhs_rows: &Self,
        lhs_cols: &Self,
        rhs_rows: &Self,
        rhs_cols: &Self,
        lhs_col_extended: Option<&Self>,
        rhs_row_extended: Option<&Self>,
    ) -> Option<f64> {
        let mut result = 0.0;
        let code = unsafe {
            LAGraph_RPQMatrix_count_vector_mnc_matmul_nnz(
                &mut result,
                lhs_rows.raw(),
                lhs_cols.raw(),
                rhs_rows.raw(),
                rhs_cols.raw(),
                lhs_col_extended.map_or(std::ptr::null_mut(), Self::raw),
                rhs_row_extended.map_or(std::ptr::null_mut(), Self::raw),
            )
        };
        (code == GrB_Info::GrB_SUCCESS).then_some(result)
    }

    pub(super) fn mnc_add(&self, other: &Self, lambda: f64, cap: f64) -> Option<Self> {
        let mut result = std::ptr::null_mut();
        let code = unsafe {
            LAGraph_RPQMatrix_count_vector_mnc_add(
                &mut result,
                self.raw(),
                other.raw(),
                lambda,
                cap,
            )
        };
        (code == GrB_Info::GrB_SUCCESS)
            .then(|| Self::from_owned(result))
            .flatten()
    }

    pub(super) fn scale(&self, scale: f64, cap: f64) -> Option<Self> {
        let mut result = std::ptr::null_mut();
        let code =
            unsafe { LAGraph_RPQMatrix_count_vector_scale(&mut result, self.raw(), scale, cap) };
        (code == GrB_Info::GrB_SUCCESS)
            .then(|| Self::from_owned(result))
            .flatten()
    }

    pub(super) fn nonzero_count(&self) -> f64 {
        self.nvals as f64
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LabelCountVectors {
    pub row_counts: CountVector,
    pub col_counts: CountVector,
    pub row_extended: Option<CountVector>,
    pub col_extended: Option<CountVector>,
}

impl LabelCountVectors {
    pub(crate) fn from_matrix(matrix: GrB_Matrix) -> Option<Self> {
        let mut counts = Self::from_matrix_basic(matrix)?;
        counts.ensure_extended(matrix)?;
        Some(counts)
    }

    pub(crate) fn from_matrix_basic(matrix: GrB_Matrix) -> Option<Self> {
        let row_counts = CountVector::from_matrix(matrix, false)?;
        let col_counts = CountVector::from_matrix(matrix, true)?;
        Some(Self {
            row_counts,
            col_counts,
            row_extended: None,
            col_extended: None,
        })
    }

    pub(super) fn ensure_extended(&mut self, matrix: GrB_Matrix) -> Option<()> {
        if self.row_extended.is_some() && self.col_extended.is_some() {
            return Some(());
        }
        let mut row_extended = std::ptr::null_mut();
        let mut col_extended = std::ptr::null_mut();
        let code = unsafe {
            LAGraph_RPQMatrix_extended_count_vectors(
                &mut row_extended,
                &mut col_extended,
                matrix,
                self.row_counts.raw(),
                self.col_counts.raw(),
            )
        };
        if code != GrB_Info::GrB_SUCCESS {
            return None;
        }
        let row = CountVector::from_owned(row_extended);
        let col = CountVector::from_owned(col_extended);
        let (Some(row), Some(col)) = (row, col) else {
            return None;
        };
        self.row_extended = Some(row);
        self.col_extended = Some(col);
        Some(())
    }

    pub(super) fn from_vertex(vertex: usize, n: usize) -> Option<Self> {
        let mut matrix = std::ptr::null_mut();
        let create = unsafe { LAGraph_RPQMatrix_label(&mut matrix, vertex as _, n as _, n as _) };
        let matrix = GraphblasMatrix::from_raw(matrix);
        if create != GrB_Info::GrB_SUCCESS {
            return None;
        }
        Self::from_matrix(matrix.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{graph::GraphDecomposition, utils::build_graph};

    #[test]
    fn mnc_vector_operations_use_graphblas_counts() {
        let graph = build_graph(&[("A", "B", "p"), ("B", "C", "p")]);
        let matrix = graph.get_graph("p").unwrap().matrix();
        let counts = LabelCountVectors::from_matrix(matrix).unwrap();
        assert_eq!(counts.row_counts.sum(), 2.0);
        assert_eq!(counts.col_counts.sum(), 2.0);
        assert_eq!(counts.col_counts.dot(&counts.row_counts), Some(1.0));
        assert_eq!(
            CountVector::mnc_matmul_nnz(
                &counts.row_counts,
                &counts.col_counts,
                &counts.row_counts,
                &counts.col_counts,
                counts.col_extended.as_ref(),
                counts.row_extended.as_ref(),
            ),
            Some(1.0),
        );
        let union = counts
            .row_counts
            .mnc_add(&counts.row_counts, 0.5, 3.0)
            .unwrap();
        assert_eq!(union.sum(), 3.0);
        assert_eq!(union.nonzero_count(), 2.0);
        let empty = counts.row_counts.scale(0.0, 3.0).unwrap();
        assert_eq!(empty.sum(), 0.0);
        assert_eq!(empty.nonzero_count(), 0.0);
        let fractional = counts.row_counts.scale(0.4, 3.0).unwrap();
        assert_eq!(fractional.sum(), 0.8);
        assert_eq!(fractional.nonzero_count(), 2.0);
    }

    #[test]
    fn basic_counts_do_not_build_mnc_extensions_until_requested() {
        let graph = build_graph(&[("A", "B", "p")]);
        let matrix = graph.get_graph("p").unwrap().matrix();
        let mut counts = LabelCountVectors::from_matrix_basic(matrix).unwrap();
        assert!(counts.row_extended.is_none());
        assert!(counts.col_extended.is_none());
        counts.ensure_extended(matrix).unwrap();
        assert!(counts.row_extended.is_some());
        assert!(counts.col_extended.is_some());
    }

    #[test]
    fn mnc_extension_vectors_account_for_singleton_rows_and_columns() {
        let graph = build_graph(&[
            ("u", "a", "A"),
            ("v", "b", "A"),
            ("v", "c", "A"),
            ("a", "x", "B"),
            ("b", "x", "B"),
            ("c", "y", "B"),
        ]);
        let a = LabelCountVectors::from_matrix(graph.get_graph("A").unwrap().matrix()).unwrap();
        let b = LabelCountVectors::from_matrix(graph.get_graph("B").unwrap().matrix()).unwrap();
        assert_eq!(a.row_counts.sum(), 3.0);
        assert_eq!(a.col_counts.sum(), 3.0);
        assert_eq!(b.row_counts.sum(), 3.0);
        assert_eq!(b.col_counts.sum(), 3.0);
        assert_eq!(a.row_counts.nonzero_count(), 2.0);
        assert_eq!(a.col_counts.nonzero_count(), 3.0);
        assert_eq!(b.row_counts.nonzero_count(), 3.0);
        assert_eq!(b.col_counts.nonzero_count(), 2.0);
        assert_eq!(a.row_extended.as_ref().unwrap().nonzero_count(), 2.0);
        assert_eq!(a.col_extended.as_ref().unwrap().nonzero_count(), 1.0);
        assert_eq!(b.row_extended.as_ref().unwrap().nonzero_count(), 1.0);
        assert_eq!(b.col_extended.as_ref().unwrap().nonzero_count(), 2.0);
        assert_eq!(a.row_extended.as_ref().unwrap().sum(), 3.0);
        assert_eq!(a.col_extended.as_ref().unwrap().sum(), 1.0);
        assert_eq!(b.row_extended.as_ref().unwrap().sum(), 1.0);
        assert_eq!(b.col_extended.as_ref().unwrap().sum(), 3.0);

        let estimate = |lhs_ext, rhs_ext| {
            CountVector::mnc_matmul_nnz(
                &a.row_counts,
                &a.col_counts,
                &b.row_counts,
                &b.col_counts,
                lhs_ext,
                rhs_ext,
            )
            .unwrap()
        };
        assert!((estimate(None, None) - 2.3125).abs() < 1e-10);
        let lhs_only = estimate(a.col_extended.as_ref(), None);
        let rhs_only = estimate(None, b.row_extended.as_ref());
        assert!((lhs_only - 2.5).abs() < 1e-10, "lhs-only: {lhs_only}");
        assert!((rhs_only - 2.5).abs() < 1e-10, "rhs-only: {rhs_only}");
        assert_eq!(
            estimate(a.col_extended.as_ref(), b.row_extended.as_ref()),
            3.0
        );
    }

    #[test]
    fn fractional_counts_do_not_trigger_the_exact_singleton_case() {
        let graph = build_graph(&[("A", "A", "p"), ("B", "B", "p")]);
        let counts =
            LabelCountVectors::from_matrix(graph.get_graph("p").unwrap().matrix()).unwrap();
        let fractional = counts.row_counts.scale(0.4, 2.0).unwrap();
        let estimate = CountVector::mnc_matmul_nnz(
            &fractional,
            &fractional,
            &fractional,
            &fractional,
            None,
            None,
        )
        .unwrap();
        assert!((estimate - 0.3136).abs() < 1e-10, "estimate: {estimate}");
    }
}
