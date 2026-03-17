use std::{collections::HashMap, io::Read};
use std::sync::Arc;

use crate::formats::Csv;
use crate::{graph::GraphSource, lagraph_sys::{
    GrB_BOOL, GrB_Index, GrB_Info, GrB_LOR, GrB_Matrix, GrB_Matrix_build_BOOL, GrB_Matrix_free, GrB_Matrix_new, LAGraph_Graph, LAGraph_Kind, LAGraph_New
}};

use super::{
    Backend, Edge, GraphBuilder, GraphDecomposition, GraphError, LagraphGraph,
    ensure_grb_init, grb_ok,
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
pub struct InMemoryBuilder {
    node_to_id: HashMap<String, u64>,
    id_to_node: Vec<String>,
    label_buffers: HashMap<String, Vec<(u64, u64)>>,
    prebuilt: HashMap<String, LagraphGraph>,
}

impl InMemoryBuilder {
    pub fn new() -> Self {
        Self {
            node_to_id: HashMap::new(),
            id_to_node: Vec::new(),
            label_buffers: HashMap::new(),
            prebuilt: HashMap::new(),
        }
    }

    fn insert_node(&mut self, node: &str) -> u64 {
        if let Some(&id) = self.node_to_id.get(node) {
            return id;
        }
        let id = self.id_to_node.len() as u64;
        self.id_to_node.push(node.to_owned());
        self.node_to_id.insert(node.to_owned(), id);
        id
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

    pub fn with_stream<I, E>(&mut self, stream: I) -> Result<(), GraphError>
    where
        I: IntoIterator<Item = Result<Edge, E>>,
        GraphError: From<E>,
    {
        for item in stream {
            self.push_edge(item?)?;
        }
        Ok(())
    }

    /// Accept a pre-built [`GrB_Matrix`] for `label`, wrapping it in an
    /// [`LAGraph_Graph`] immediately.
    pub fn push_grb_matrix(
        &mut self,
        label: impl Into<String>,
        mut matrix: GrB_Matrix,
    ) -> Result<(), GraphError> {
        ensure_grb_init()?;
        let lg: LagraphGraph = unsafe {
            let mut g: LAGraph_Graph = std::ptr::null_mut();
            let mut msg = [0i8; 256];
            let info = LAGraph_New(
                &mut g,
                &mut matrix,
                LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED,
                msg.as_mut_ptr(),
            );
            if info != GrB_Info::GrB_SUCCESS as i32 {
                if !matrix.is_null() {
                    GrB_Matrix_free(&mut matrix);
                }
                return Err(GraphError::GraphBlas(info));
            }
            LagraphGraph { inner: g }
        };
        self.prebuilt.insert(label.into(), lg);
        Ok(())
    }
}

impl Default for InMemoryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder for InMemoryBuilder {
    type Graph = InMemoryGraph;
    type Error = GraphError;

    fn build(self) -> Result<InMemoryGraph, GraphError> {
        ensure_grb_init()?;

        let n = self.id_to_node.len() as GrB_Index;

        let mut graphs: HashMap<String, Arc<LagraphGraph>> =
            HashMap::with_capacity(self.label_buffers.len() + self.prebuilt.len());

        for (label, lg) in self.prebuilt {
            graphs.insert(label, Arc::new(lg));
        }

        for (label, pairs) in &self.label_buffers {
            let rows: Vec<GrB_Index> = pairs.iter().map(|(r, _)| *r).collect();
            let cols: Vec<GrB_Index> = pairs.iter().map(|(_, c)| *c).collect();
            let vals: Vec<bool> = vec![true; pairs.len()];
            let nvals = pairs.len() as GrB_Index;

            let grb_matrix: GrB_Matrix = unsafe {
                let mut m: GrB_Matrix = std::ptr::null_mut();
                grb_ok(GrB_Matrix_new(&mut m, GrB_BOOL, n, n) as i32)?;
                grb_ok(GrB_Matrix_build_BOOL(
                    m,
                    rows.as_ptr(),
                    cols.as_ptr(),
                    vals.as_ptr(),
                    nvals,
                    GrB_LOR,
                ) as i32)?;
                m
            };

            let lg: LagraphGraph = unsafe {
                let mut g: LAGraph_Graph = std::ptr::null_mut();
                let mut a = grb_matrix;
                let mut msg = [0i8; 256];
                let info = LAGraph_New(
                    &mut g,
                    &mut a,
                    LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED,
                    msg.as_mut_ptr(),
                );
                if info != GrB_Info::GrB_SUCCESS as i32 {
                    if !a.is_null() {
                        GrB_Matrix_free(&mut a);
                    }
                    return Err(GraphError::GraphBlas(info));
                }
                LagraphGraph { inner: g }
            };

            graphs.insert(label.clone(), Arc::new(lg));
        }

        let node_to_id: HashMap<String, usize> = self
            .node_to_id
            .into_iter()
            .map(|(k, v)| (k, v as usize))
            .collect();

        Ok(InMemoryGraph {
            node_to_id,
            id_to_node: self.id_to_node,
            graphs,
        })
    }
}

/// Immutable, read-only Boolean-decomposed graph backed by LAGraph graphs.
pub struct InMemoryGraph {
    node_to_id: HashMap<String, usize>,
    id_to_node: Vec<String>,
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
        self.id_to_node.get(mapped_id).cloned()
    }

    fn num_nodes(&self) -> usize {
        self.id_to_node.len()
    }
}

impl<R: Read> GraphSource<InMemoryBuilder> for Csv<R> {
    fn apply_to(self, mut builder: InMemoryBuilder) -> Result<InMemoryBuilder, crate::graph::GraphError> {
        for item in self {
            builder.push_edge(item?)?;
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
        let graph = InMemoryBuilder::new().build().expect("build should succeed");
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
