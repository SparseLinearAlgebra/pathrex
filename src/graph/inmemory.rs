use std::sync::Arc;
use std::{collections::HashMap, io::Read};

use crate::formats::mm::{load_mm_file, parse_index_map};
use crate::formats::{Csv, MatrixMarket};
use crate::{
    graph::GraphSource,
    lagraph_sys::{GrB_Index, GrB_Matrix, GrB_Matrix_free, LAGraph_Kind},
};

use super::{
    Backend, Edge, GraphBuilder, GraphDecomposition, GraphError, LagraphGraph, ensure_grb_init,
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

    /// Accept a pre-built [`GrB_Matrix`] for `label`, wrapping it in an
    /// [`LAGraph_Graph`] immediately.
    pub fn push_grb_matrix(
        &mut self,
        label: impl Into<String>,
        mut matrix: GrB_Matrix,
    ) -> Result<(), GraphError> {
        ensure_grb_init()?;
        let lg: LagraphGraph =
            match LagraphGraph::new(matrix, LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED) {
                Ok(g) => g,
                Err(e) => {
                    if !matrix.is_null() {
                        unsafe { GrB_Matrix_free(&mut matrix) };
                    }
                    return Err(e);
                }
            };
        self.prebuilt.insert(label.into(), lg);
        Ok(())
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

        for (label, pairs) in &self.label_buffers {
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

            graphs.insert(label.clone(), Arc::new(lg));
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
    type Error = GraphError;

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
        let vertices_path = self.dir.join("vertices.txt");
        let (vert_by_idx, vert_by_name) = parse_index_map(&vertices_path)?;
        let vert_by_idx  =
            vert_by_idx.into_iter().map(|(i, n)| (i - 1, n)).collect();
        let vert_by_name =
            vert_by_name.into_iter().map(|(n, i)| (n, i - 1)).collect();

        let (edge_by_idx, _) = parse_index_map(&self.dir.join("edges.txt"))?;

        builder.set_node_map(vert_by_idx, vert_by_name);

        for (idx, label) in edge_by_idx {
            let path = self.mm_path(idx);
            let matrix = load_mm_file(&path)?;
            builder.push_grb_matrix(label, matrix)?;
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
}
