//! Core graph abstractions for pathrex.

pub mod inmemory;
pub mod wrappers;

pub use inmemory::{InMemory, InMemoryBuilder, InMemoryGraph};
pub use wrappers::{GraphblasMatrix, GraphblasVector, LagraphGraph, load_mm_file};
pub(crate) use wrappers::{ThreadScope, compute_outer_inner, ensure_grb_init};

use std::marker::PhantomData;
use std::sync::Arc;

use crate::lagraph_sys::GrB_Info;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphError {
    /// A GraphBLAS C call returned a non-SUCCESS info code.
    #[error("GraphBLAS error: info code {0}")]
    GraphBlas(GrB_Info),

    /// LAGraph C call returned a non-SUCCESS info code with msg.
    #[error("GraphBLAS error: info code {0}; msg: {1}")]
    LAGraph(GrB_Info, String),

    /// GraphBLAS/LAGraph initialisation failed.
    #[error("LAGraph initialization failed")]
    InitFailed,

    /// [`GraphDecomposition::get_graph`] was called with an unknown label.
    #[error("Label not found: '{0}'")]
    LabelNotFound(String),

    /// A format-layer error propagated through [`GraphBuilder::load`].
    #[error("Format error: {0}")]
    Format(#[from] crate::formats::FormatError),
}

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
    fn test_compute_outer_inner_product_bounded_by_cores() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        for num_tasks in [0usize, 1, 2, 4, 8, 16, 64, 1024] {
            let (outer, inner) = compute_outer_inner(num_tasks);
            assert!(outer >= 1, "outer must be >= 1 for num_tasks={num_tasks}");
            assert!(inner >= 1, "inner must be >= 1 for num_tasks={num_tasks}");
            let product = (outer as usize) * (inner as usize);
            assert!(
                product <= cores.max(1),
                "outer*inner ({outer}*{inner}={product}) must not exceed cores ({cores}) for num_tasks={num_tasks}"
            );
        }
    }

    #[test]
    fn test_compute_outer_inner_caps_outer_at_tasks() {
        // With a very small number of tasks, outer should never exceed that.
        let (outer, _inner) = compute_outer_inner(1);
        assert_eq!(outer, 1);
        let (outer, _inner) = compute_outer_inner(2);
        assert!(outer <= 2);
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
