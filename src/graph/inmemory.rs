use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use lasso::{Key, Rodeo, RodeoReader, Spur};
use rayon::prelude::*;

use crate::formats::mm::{apply_base_iri, parse_index_map};
use crate::formats::{Csv, MatrixMarket, Rdf};
use crate::{
    graph::GraphSource,
    lagraph_sys::{GrB_Index, LAGraph_Kind},
};

use super::{
    Backend, Edge, GraphBuilder, GraphDecomposition, GraphError, LagraphGraph, ThreadScope,
    compute_outer_inner, load_mm_file,
};

/// Marker type for the in-memory GraphBLAS-backed backend.
///
/// ```no_run
/// use pathrex::graph::{Graph, InMemory, GraphDecomposition};
/// use pathrex::formats::csv::Csv;
/// use std::fs::File;
///
/// let graph = Graph::<InMemory>::try_from(
///     Csv::from_reader(File::open("edges.csv").unwrap()).unwrap()
/// ).unwrap();
/// println!("Nodes: {}", graph.num_nodes());
/// ```
pub struct InMemory;

impl Backend for InMemory {
    type Graph = InMemoryGraph;
    type Builder = InMemoryBuilder;
}

/// Accumulates edges in RAM and compiles them into an [`InMemoryGraph`].
#[derive(Default)]
pub struct InMemoryBuilder {
    nodes: Rodeo,
    label_buffers: HashMap<String, Vec<(usize, usize)>>,
    prebuilt: HashMap<String, LagraphGraph>,
}

impl InMemoryBuilder {
    pub fn new() -> Self {
        Self {
            nodes: Rodeo::new(),
            label_buffers: HashMap::new(),
            prebuilt: HashMap::new(),
        }
    }

    fn insert_node(&mut self, node: &str) -> usize {
        self.nodes.get_or_intern(node).into_usize()
    }

    /// Bulk-install the node mapping. Replaces any previously inserted nodes.
    pub fn set_node_map(
        &mut self,
        by_idx: HashMap<usize, String>,
        _by_name: HashMap<String, usize>,
    ) {
        self.nodes = Rodeo::new();
        let mut pairs: Vec<(usize, String)> = by_idx.into_iter().collect();
        pairs.sort_unstable_by_key(|(id, _)| *id);
        for (expected_id, name) in pairs {
            let spur = self.nodes.get_or_intern(name);
            debug_assert_eq!(
                spur.into_usize(),
                expected_id,
                "Spur must match pre-assigned node id"
            );
        }
    }

    pub fn push_edge(&mut self, edge: Edge) -> Result<(), GraphError> {
        let src = self.insert_node(&edge.source);
        let tgt = self.insert_node(&edge.target);
        self.label_buffers
            .entry(edge.label)
            .or_default()
            .push((src, tgt));
        Ok(())
    }

    pub fn with_stream<I, E>(mut self, stream: I) -> Result<Self, GraphError>
    where
        I: IntoIterator<Item = Result<Edge, E>>,
        GraphError: From<E>,
    {
        for item in stream {
            self.push_edge(item?)?;
        }
        Ok(self)
    }

    /// Bulk-install pre-wrapped `(label, LagraphGraph)` pairs into `prebuilt`.
    pub(crate) fn extend_prebuilt<I: IntoIterator<Item = (String, LagraphGraph)>>(
        &mut self,
        iter: I,
    ) {
        self.prebuilt.extend(iter);
    }

    pub(crate) fn set_node_rodeo(&mut self, rodeo: lasso::Rodeo) {
        self.nodes = rodeo;
    }
}

impl GraphBuilder for InMemoryBuilder {
    type Graph = InMemoryGraph;
    type Error = GraphError;

    fn build(self) -> Result<InMemoryGraph, GraphError> {
        let n: GrB_Index = self.nodes.len() as GrB_Index;

        let mut graphs: HashMap<String, Arc<LagraphGraph>> =
            HashMap::with_capacity(self.label_buffers.len() + self.prebuilt.len());

        for (label, lg) in self.prebuilt {
            graphs.insert(label, Arc::new(lg));
        }

        let label_buffers: Vec<(String, Vec<(usize, usize)>)> =
            self.label_buffers.into_iter().collect();

        let (outer, inner) = compute_outer_inner(label_buffers.len());
        let _scope = ThreadScope::enter(outer, inner)?;

        let built: Vec<(String, LagraphGraph)> = label_buffers
            .into_par_iter()
            .map(
                |(label, pairs)| -> Result<(String, LagraphGraph), GraphError> {
                    let rows: Vec<GrB_Index> = pairs.iter().map(|(r, _)| *r as GrB_Index).collect();
                    let cols: Vec<GrB_Index> = pairs.iter().map(|(_, c)| *c as GrB_Index).collect();
                    let vals: Vec<bool> = vec![true; pairs.len()];

                    let lg = LagraphGraph::from_coo(
                        &rows,
                        &cols,
                        &vals,
                        n,
                        LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED,
                    )?;
                    Ok((label, lg))
                },
            )
            .collect::<Result<Vec<_>, GraphError>>()?;

        for (label, lg) in built {
            graphs.insert(label, Arc::new(lg));
        }

        let nodes = self.nodes.into_reader();

        Ok(InMemoryGraph { nodes, graphs })
    }
}

/// Immutable, read-only Boolean-decomposed graph backed by LAGraph graphs.
pub struct InMemoryGraph {
    nodes: RodeoReader,
    graphs: HashMap<String, Arc<LagraphGraph>>,
}

impl GraphDecomposition for InMemoryGraph {
    fn get_graph(&self, label: &str) -> Result<Arc<LagraphGraph>, GraphError> {
        self.graphs
            .get(label)
            .cloned()
            .ok_or_else(|| GraphError::LabelNotFound(label.to_owned()))
    }

    fn get_node_id(&self, string_id: &str) -> Option<usize> {
        self.nodes.get(string_id).map(Spur::into_usize)
    }

    fn get_node_name(&self, mapped_id: usize) -> Option<String> {
        let spur = Spur::try_from_usize(mapped_id)?;
        self.nodes.try_resolve(&spur).map(str::to_owned)
    }

    fn num_nodes(&self) -> usize {
        self.nodes.len()
    }
}

impl InMemoryGraph {
    /// Returns the number of distinct edge labels in the graph.
    pub fn num_labels(&self) -> usize {
        self.graphs.len()
    }
}

impl<R: Read> GraphSource<InMemoryBuilder> for Csv<R> {
    fn apply_to(
        self,
        mut builder: InMemoryBuilder,
    ) -> Result<InMemoryBuilder, crate::graph::GraphError> {
        for item in self {
            builder.push_edge(item?)?;
        }
        Ok(builder)
    }
}

impl GraphSource<InMemoryBuilder> for MatrixMarket {
    fn apply_to(self, mut builder: InMemoryBuilder) -> Result<InMemoryBuilder, GraphError> {
        let base = self.base_iri.as_deref();
        let vertices_path = self.dir.join("vertices.txt");
        let (vert_by_idx, vert_by_name) = parse_index_map(&vertices_path)?;
        let vert_by_idx = vert_by_idx
            .into_iter()
            .map(|(i, n)| (i - 1, apply_base_iri(n, base)))
            .collect();
        let vert_by_name = vert_by_name
            .into_iter()
            .map(|(n, i)| (apply_base_iri(n, base), i - 1))
            .collect();

        let (edge_by_idx, _) = parse_index_map(&self.dir.join("edges.txt"))?;
        let edge_by_idx: Vec<(usize, String)> = edge_by_idx
            .into_iter()
            .map(|(i, label)| (i, apply_base_iri(label, base)))
            .collect();

        builder.set_node_map(vert_by_idx, vert_by_name);

        let (outer, inner) = compute_outer_inner(edge_by_idx.len());
        let _scope = ThreadScope::enter(outer, inner)?;

        let mm_dir = self.dir.clone();
        let loaded: Vec<(String, LagraphGraph)> = edge_by_idx
            .into_par_iter()
            .map(
                |(idx, label)| -> Result<(String, LagraphGraph), GraphError> {
                    let path = mm_dir.join(format!("{}.txt", idx));
                    let matrix = load_mm_file(&path)?;
                    let lg = LagraphGraph::from_matrix(
                        matrix,
                        LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED,
                    )?;
                    Ok((label, lg))
                },
            )
            .collect::<Result<Vec<_>, GraphError>>()?;

        builder.extend_prebuilt(loaded);

        Ok(builder)
    }
}

impl GraphSource<InMemoryBuilder> for Rdf {
    fn apply_to(self, mut builder: InMemoryBuilder) -> Result<InMemoryBuilder, GraphError> {
        use crate::formats::rdf::{merge_shards, parse_shards};

        let n_shards = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let shards = parse_shards(self.as_bytes(), self.format(), n_shards);
        let merged = merge_shards(shards);
        dbg!(merged.skipped_total);

        let n: crate::lagraph_sys::GrB_Index = merged.nodes.len() as crate::lagraph_sys::GrB_Index;
        let n_labels = merged.label_names.len();

        let (outer, inner) = compute_outer_inner(n_labels);
        let _scope = ThreadScope::enter(outer, inner)?;

        let built: Vec<(String, LagraphGraph)> = merged
            .label_names
            .into_par_iter()
            .zip(merged.edges_by_global_label.into_par_iter())
            .map(
                |(label, pairs)| -> Result<(String, LagraphGraph), GraphError> {
                    let rows: Vec<crate::lagraph_sys::GrB_Index> =
                        pairs.iter().map(|&(r, _)| r).collect();
                    let cols: Vec<crate::lagraph_sys::GrB_Index> =
                        pairs.iter().map(|&(_, c)| c).collect();
                    let lg = LagraphGraph::from_coo_bool_scalar(
                        &rows,
                        &cols,
                        n,
                        crate::lagraph_sys::LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED,
                    )?;
                    Ok((label, lg))
                },
            )
            .collect::<Result<Vec<_>, GraphError>>()?;

        builder.set_node_rodeo(merged.nodes);
        builder.extend_prebuilt(built);
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphBuilder, GraphDecomposition};

    fn make_graph(edges: &[(&str, &str, &str)]) -> InMemoryGraph {
        let mut builder = InMemoryBuilder::new();
        for &(src, tgt, lbl) in edges {
            builder
                .push_edge(Edge {
                    source: src.to_owned(),
                    target: tgt.to_owned(),
                    label: lbl.to_owned(),
                })
                .expect("push_edge should not fail");
        }
        builder.build().expect("build should succeed")
    }

    #[test]
    fn test_node_dictionary_round_trip() {
        let graph = make_graph(&[("Alice", "Bob", "knows"), ("Bob", "Charlie", "knows")]);

        assert_eq!(graph.num_nodes(), 3);

        for name in &["Alice", "Bob", "Charlie"] {
            let id = graph.get_node_id(name).expect("node should exist");
            assert!(id < 3);
            assert_eq!(graph.get_node_name(id).as_deref(), Some(*name));
        }

        assert!(graph.get_node_id("NonExistent").is_none());
        assert!(graph.get_node_name(999).is_none());
    }

    #[test]
    fn test_graph_exists_for_each_label() {
        let graph = make_graph(&[
            ("A", "B", "knows"),
            ("B", "C", "knows"),
            ("A", "C", "likes"),
        ]);

        assert!(graph.get_graph("knows").is_ok());
        assert!(graph.get_graph("likes").is_ok());
        assert!(matches!(
            graph.get_graph("nonexistent"),
            Err(GraphError::LabelNotFound(_))
        ));
    }

    #[test]
    fn test_empty_builder_produces_empty_graph() {
        let graph = InMemoryBuilder::new()
            .build()
            .expect("build should succeed");
        assert_eq!(graph.num_nodes(), 0);
        assert!(matches!(
            graph.get_graph("anything"),
            Err(GraphError::LabelNotFound(_))
        ));
    }

    #[test]
    fn test_self_loop_edge() {
        let graph = make_graph(&[("A", "A", "self")]);
        assert_eq!(graph.num_nodes(), 1);
        assert!(graph.get_graph("self").is_ok());
    }

    #[test]
    fn test_with_stream_from_csv() {
        use crate::formats::csv::Csv;

        let csv = "source,target,label\nA,B,knows\nB,C,likes\nC,A,knows\n";
        let iter = Csv::from_reader(csv.as_bytes()).unwrap();

        let graph = InMemoryBuilder::new()
            .load(iter)
            .expect("load should succeed")
            .build()
            .expect("build should succeed");

        assert_eq!(graph.num_nodes(), 3);
        assert!(graph.get_graph("knows").is_ok());
        assert!(graph.get_graph("likes").is_ok());
    }

    #[test]
    fn test_rdf_skip_bad_syntax_lines() {
        use crate::formats::rdf::{Rdf, RdfFormat};

        let nt = b"<http://example.org/A> <http://example.org/knows> <http://example.org/B> .\n\
                   THIS IS NOT VALID RDF SYNTAX .\n\
                   <http://example.org/B> <http://example.org/knows> <http://example.org/C> .\n";

        let graph = InMemoryBuilder::new()
            .load(Rdf::new(nt.as_ref(), RdfFormat::NTriples))
            .expect("load should succeed despite bad line")
            .build()
            .expect("build should succeed");

        assert_eq!(graph.num_nodes(), 3, "A, B, C must all be present");
        assert!(
            graph.get_graph("http://example.org/knows").is_ok(),
            "label matrix must exist"
        );
    }

    #[test]
    fn test_with_stream_from_rdf() {
        use crate::formats::rdf::{Rdf, RdfFormat};

        let nt = b"<http://example.org/A> <http://example.org/knows> <http://example.org/B> .\n\
                  <http://example.org/B> <http://example.org/knows> <http://example.org/C> .\n\
                  <http://example.org/A> <http://example.org/likes> <http://example.org/C> .\n";

        let graph = InMemoryBuilder::new()
            .load(Rdf::new(nt.as_ref(), RdfFormat::NTriples))
            .expect("load should succeed")
            .build()
            .expect("build should succeed");

        assert_eq!(graph.num_nodes(), 3);
        assert!(graph.get_graph("http://example.org/knows").is_ok());
        assert!(graph.get_graph("http://example.org/likes").is_ok());
    }
}
