use std::sync::Arc;
use std::{collections::HashMap, io::Read};

use rayon::prelude::*;

use crate::formats::mm::{apply_base_iri, load_mm_file, parse_index_map};
use crate::formats::{Csv, MatrixMarket, Rdf};
use crate::{
    graph::GraphSource,
    lagraph_sys::{GrB_Index, GrB_Matrix_free, LAGraph_Kind},
};

use super::{
    compute_outer_inner, ensure_grb_init, Backend, Edge, GraphBuilder, GraphDecomposition,
    GraphError, LagraphGraph, ThreadScope,
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
    node_to_id: HashMap<String, usize>,
    id_to_node: HashMap<usize, String>,
    next_id: usize,
    label_buffers: HashMap<String, Vec<(usize, usize)>>,
    prebuilt: HashMap<String, LagraphGraph>,
}

impl InMemoryBuilder {
    pub fn new() -> Self {
        Self {
            node_to_id: HashMap::new(),
            id_to_node: HashMap::new(),
            next_id: 0,
            label_buffers: HashMap::new(),
            prebuilt: HashMap::new(),
        }
    }

    fn insert_node(&mut self, node: &str) -> usize {
        if let Some(&id) = self.node_to_id.get(node) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.id_to_node.insert(id, node.to_owned());
        self.node_to_id.insert(node.to_owned(), id);
        id
    }

    /// Bulk-install the node mapping. Replaces any previously inserted nodes.
    pub fn set_node_map(
        &mut self,
        by_idx: HashMap<usize, String>,
        by_name: HashMap<String, usize>,
    ) {
        self.id_to_node = by_idx;
        self.node_to_id = by_name;
        self.next_id = self
            .id_to_node
            .keys()
            .copied()
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
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
}

impl GraphBuilder for InMemoryBuilder {
    type Graph = InMemoryGraph;
    type Error = GraphError;

    fn build(self) -> Result<InMemoryGraph, GraphError> {
        ensure_grb_init()?;

        let n: GrB_Index = self
            .id_to_node
            .keys()
            .copied()
            .max()
            .map(|m| m + 1)
            .unwrap_or(0) as GrB_Index;

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
                    let rows: Vec<GrB_Index> =
                        pairs.iter().map(|(r, _)| *r as GrB_Index).collect();
                    let cols: Vec<GrB_Index> =
                        pairs.iter().map(|(_, c)| *c as GrB_Index).collect();
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

        Ok(InMemoryGraph {
            node_to_id: self.node_to_id,
            id_to_node: self.id_to_node,
            graphs,
        })
    }
}

/// Immutable, read-only Boolean-decomposed graph backed by LAGraph graphs.
pub struct InMemoryGraph {
    node_to_id: HashMap<String, usize>,
    id_to_node: HashMap<usize, String>,
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
        self.node_to_id.get(string_id).copied()
    }

    fn get_node_name(&self, mapped_id: usize) -> Option<String> {
        self.id_to_node.get(&mapped_id).cloned()
    }

    fn num_nodes(&self) -> usize {
        self.id_to_node.len()
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

        ensure_grb_init()?;
        let (outer, inner) = compute_outer_inner(edge_by_idx.len());
        let _scope = ThreadScope::enter(outer, inner)?;

        let mm_dir = self.dir.clone();
        let loaded: Vec<(String, LagraphGraph)> = edge_by_idx
            .into_par_iter()
            .map(
                |(idx, label)| -> Result<(String, LagraphGraph), GraphError> {
                    let path = mm_dir.join(format!("{}.txt", idx));
                    let mut matrix = load_mm_file(&path)?;
                    match LagraphGraph::new(matrix, LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED) {
                        Ok(lg) => Ok((label, lg)),
                        Err(e) => {
                            if !matrix.is_null() {
                                unsafe { GrB_Matrix_free(&mut matrix) };
                            }
                            Err(e)
                        }
                    }
                },
            )
            .collect::<Result<Vec<_>, GraphError>>()?;

        builder.extend_prebuilt(loaded);

        Ok(builder)
    }
}

impl GraphSource<InMemoryBuilder> for Rdf {
    fn apply_to(self, mut builder: InMemoryBuilder) -> Result<InMemoryBuilder, GraphError> {
        for result in self.parse() {
            match result {
                Ok(edge) => builder.push_edge(edge)?,
                Err(_) => {}
            }
        }
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
