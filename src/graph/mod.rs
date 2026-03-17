//! Core graph abstractions for pathrex.

pub mod inmemory;

pub use inmemory::{InMemory, InMemoryBuilder, InMemoryGraph};

use std::marker::PhantomData;
use std::sync::{Arc, Once};

use crate::lagraph_sys::{
    GrB_BOOL, GrB_Index, GrB_Info, GrB_Vector, GrB_Vector_free, GrB_Vector_new,
    LAGraph_Delete, LAGraph_Graph, LAGraph_Init,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphError {
    /// A GraphBLAS / LAGraph C call returned a non-SUCCESS info code.
    #[error("GraphBLAS error: info code {0}")]
    GraphBlas(i32),

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
        let mut msg = [0i8; 256];
        let info = unsafe { LAGraph_Init(msg.as_mut_ptr()) };
        if info != GrB_Info::GrB_SUCCESS as i32 {
            result = Err(GraphError::InitFailed);
        }
    });
    result
}

#[inline]
pub fn grb_ok(info: i32) -> Result<(), GraphError> {
    if info == GrB_Info::GrB_SUCCESS as i32 {
        Ok(())
    } else {
        Err(GraphError::GraphBlas(info))
    }
}

pub struct LagraphGraph {
    pub(crate) inner: LAGraph_Graph,
}

impl Drop for LagraphGraph {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            let mut msg = [0i8; 256];
            unsafe { LAGraph_Delete(&mut self.inner, msg.as_mut_ptr()) };
        }
    }
}

unsafe impl Send for LagraphGraph {}
unsafe impl Sync for LagraphGraph {}

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
        grb_ok(unsafe { GrB_Vector_new(&mut v, GrB_BOOL, n) } as i32)?;
        Ok(Self { inner: v })
    }
}

impl Drop for GraphblasVector {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            unsafe { GrB_Vector_free(&mut self.inner) };
        }
    }
}

unsafe impl Send for GraphblasVector {}
unsafe impl Sync for GraphblasVector {}

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
    type Error: std::error::Error;

    /// Returns the [`LagraphGraph`] for `label`.
    fn get_graph(&self, label: &str) -> Result<Arc<LagraphGraph>, Self::Error>;

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
    use crate::utils::{CountingBuilder, CountOutput, VecSource};

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
        let output: CountOutput<std::convert::Infallible> =
            CountingBuilder::default().load(source).unwrap().build().unwrap();
        assert_eq!(output.num_nodes(), 3);
    }

    #[test]
    fn test_graph_try_from() {
        struct TestBackend;
        impl Backend for TestBackend {
            type Graph = CountOutput<std::convert::Infallible>;
            type Builder = CountingBuilder<std::convert::Infallible>;
        }

        let source = VecSource(edges(&[
            ("X", "Y", "rel"),
            ("Y", "Z", "rel"),
        ]));
        let g = Graph::<TestBackend>::try_from(source).unwrap();
        assert_eq!(g.num_nodes(), 2);
    }
}
