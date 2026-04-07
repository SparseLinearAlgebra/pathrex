//! Core graph abstractions for pathrex.

pub mod inmemory;

pub use inmemory::{InMemory, InMemoryBuilder, InMemoryGraph};

use std::marker::PhantomData;
use std::sync::{Arc, Once};

use crate::{grb_ok, la_ok, lagraph_sys::*};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphError {
    /// A GraphBLAS C call returned a non-SUCCESS info code.
    #[error("GraphBLAS error: info code {0}")]
    GraphBlas(GrB_Info),

    /// LAGraph C call returned a non-SUCCESS info code with msg.
    #[error("GraphBLAS error: info code {0}; msg: {1}")]
    LAGraph(GrB_Info, String),

    /// [`GraphDecomposition::get_graph`] was called with an unknown label.
    #[error("Label not found: '{0}'")]
    LabelNotFound(String),

    /// [`ensure_grb_init`] was called but `LAGraph_Init` returned a failure code.
    #[error("LAGraph initialization failed")]
    InitFailed,

    /// A format-layer error propagated through [`GraphBuilder::load`].
    #[error("Format error: {0}")]
    Format(#[from] crate::formats::FormatError),
}

static GRB_INIT: Once = Once::new();

pub fn ensure_grb_init() -> Result<(), GraphError> {
    let mut result = Ok(());
    GRB_INIT.call_once(|| {
        result = la_ok!(LAGraph_Init());
    });
    result
}

#[derive(Debug)]
pub struct LagraphGraph {
    pub(crate) inner: LAGraph_Graph,
}

impl LagraphGraph {
    pub fn new(mut matrix: GrB_Matrix, kind: LAGraph_Kind) -> Result<Self, GraphError> {
        let mut g: LAGraph_Graph = std::ptr::null_mut();
        la_ok!(LAGraph_New(&mut g, &mut matrix, kind,))?;

        return Ok(Self { inner: g });
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
        grb_ok!(GrB_Matrix_new(&mut matrix, GrB_BOOL, n, n))?;

        if let Err(e) = grb_ok!(GrB_Matrix_build_BOOL(
            matrix,
            rows.as_ptr(),
            cols.as_ptr(),
            vals.as_ptr(),
            nvals,
            GrB_LOR,
        )) {
            let _ = grb_ok!(GrB_Matrix_free(&mut matrix));
            return Err(e);
        }

        Self::new(matrix, kind)
    }

    pub fn check_graph(&self) -> Result<(), GraphError> {
        la_ok!(LAGraph_CheckGraph(self.inner))
    }
}

impl Drop for LagraphGraph {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            let _ = la_ok!(LAGraph_Delete(&mut self.inner));
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
    pub unsafe fn new_bool(n: GrB_Index) -> Result<Self, GraphError> {
        let mut v: GrB_Vector = std::ptr::null_mut();
        grb_ok!(GrB_Vector_new(&mut v, GrB_BOOL, n))?;
        Ok(Self { inner: v })
    }
}

impl Drop for GraphblasVector {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            let _ = grb_ok!(GrB_Vector_free(&mut self.inner));
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
            let _ = grb_ok!(GrB_Matrix_free(&mut self.inner));
        }
    }
}

unsafe impl Send for GraphblasMatrix {}
unsafe impl Sync for GraphblasMatrix {}

/// A directed, labelled edge as produced by format parsers.
#[derive(Debug, Clone)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub label: String,
}

/// A data source that knows how to load itself into a [`GraphBuilder`].
///
/// Implement this trait for each format type  to make it usable with [`GraphBuilder::load`] and
/// [`Graph::try_from`].
pub trait GraphSource<B: GraphBuilder> {
    fn apply_to(self, builder: B) -> Result<B, B::Error>;
}

/// Builds a [`GraphDecomposition`] from one or more data sources.
pub trait GraphBuilder: Default + Sized {
    /// The graph representation this builder produces.
    type Graph: GraphDecomposition;
    /// The error type for both loading and building.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load a single data source into this builder.
    ///
    /// This is the primary entry point for feeding data.  The default
    /// implementation delegates to [`GraphSource::apply_to`].
    fn load<S: GraphSource<Self>>(self, source: S) -> Result<Self, Self::Error> {
        source.apply_to(self)
    }

    /// Finalise the build step, consuming the builder.
    fn build(self) -> Result<Self::Graph, Self::Error>;
}

/// An immutable, read-only view of a Boolean-decomposed graph.
pub trait GraphDecomposition {
    /// Returns the [`LagraphGraph`] for `label`.
    fn get_graph(&self, label: &str) -> Result<Arc<LagraphGraph>, GraphError>;

    /// Translates a string ID to a contiguous matrix index.
    fn get_node_id(&self, string_id: &str) -> Option<usize>;

    /// Translates a matrix index back to a string ID.
    fn get_node_name(&self, mapped_id: usize) -> Option<String>;
    fn num_nodes(&self) -> usize;
}

/// Associates a backend marker type with a concrete [`GraphBuilder`] and
/// [`GraphDecomposition`].
///
/// # Example
///
/// ```no_run
/// use pathrex::graph::{Backend, Graph, InMemory, GraphDecomposition};
/// use pathrex::formats::Csv;
/// use std::fs::File;
///
/// let graph = Graph::<InMemory>::try_from(
///     Csv::from_reader(File::open("edges.csv").unwrap()).unwrap()
/// ).unwrap();
/// println!("Nodes: {}", graph.num_nodes());
/// ```
pub trait Backend {
    /// The graph type produced by this backend.
    type Graph: GraphDecomposition;
    /// The builder type for this backend.  Must implement `Default` so
    /// [`Graph::try_from`] can construct it without arguments.
    type Builder: GraphBuilder<Graph = Self::Graph>;
}

/// A zero-sized handle parameterised by a [`Backend`] marker type.
///
/// Use [`Graph::<InMemory>::builder()`] to get a fresh builder, or
/// [`Graph::<InMemory>::try_from(source)`] to build a graph in one call.
pub struct Graph<B: Backend> {
    _marker: PhantomData<B>,
}

impl<B: Backend> Graph<B> {
    pub fn builder() -> B::Builder {
        B::Builder::default()
    }

    pub fn try_from<S>(source: S) -> Result<B::Graph, <B::Builder as GraphBuilder>::Error>
    where
        S: GraphSource<B::Builder>,
    {
        B::Builder::default().load(source)?.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{CountOutput, CountingBuilder, VecSource};

    fn edges(triples: &[(&str, &str, &str)]) -> Vec<Edge> {
        triples
            .iter()
            .map(|&(s, t, l)| Edge {
                source: s.into(),
                target: t.into(),
                label: l.into(),
            })
            .collect()
    }

    #[test]
    fn test_load_and_build() {
        let source = VecSource(edges(&[
            ("A", "B", "knows"),
            ("B", "C", "knows"),
            ("A", "C", "likes"),
        ]));
        let output: CountOutput<std::convert::Infallible> = CountingBuilder::default()
            .load(source)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(output.num_nodes(), 3);
    }

    #[test]
    fn test_graph_try_from() {
        struct TestBackend;
        impl Backend for TestBackend {
            type Graph = CountOutput<std::convert::Infallible>;
            type Builder = CountingBuilder<std::convert::Infallible>;
        }

        let source = VecSource(edges(&[("X", "Y", "rel"), ("Y", "Z", "rel")]));
        let g = Graph::<TestBackend>::try_from(source).unwrap();
        assert_eq!(g.num_nodes(), 2);
    }
}
