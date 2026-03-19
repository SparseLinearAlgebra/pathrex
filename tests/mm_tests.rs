use pathrex::formats::mm::MatrixMarket;
use pathrex::graph::{Backend, Graph, GraphDecomposition, GraphSource, InMemory, InMemoryGraph};

const GRAPH_DIR: &str = "tests/testdata/mm_graph";
static INMEMORY_GRAPH: std::sync::LazyLock<InMemoryGraph> =
    std::sync::LazyLock::new(|| load_graph_from_mm::<InMemory>(GRAPH_DIR));

fn load_graph_from_mm<B: Backend>(dir: &str) -> B::Graph
where
    MatrixMarket: GraphSource<B::Builder>,
{
    let mm = MatrixMarket::from_dir(dir);
    Graph::<B>::try_from(mm).expect("Failed to load graph")
}

#[test]
fn test_load_mm_graph_basic() {
    let graph = &INMEMORY_GRAPH;
    let expected_nodes_number = 24225;

    assert_eq!(graph.num_nodes(), expected_nodes_number);
}

#[test]
fn test_mm_graph_node_mapping() {
    let graph = &INMEMORY_GRAPH;

    let test_nodes = vec![
        ("<Article1>", 0),
        ("<1940>", 1),
        ("<Adamanta_Schlitt>", 2),
        ("<Paul_Erdoes>", 3),
    ];

    for (name, expected_id) in test_nodes {
        let id = graph.get_node_id(name).expect("Node should exist");
        assert_eq!(id, expected_id);

        let retrieved_name = graph
            .get_node_name(id)
            .expect("ID should map back to a name");
        assert_eq!(retrieved_name, name);
    }
}

#[test]
fn test_mm_graph_edge_labels() {
    let graph = &INMEMORY_GRAPH;

    let expected_labels = vec![
        "<journal>",
        "<creator>",
        "<references>",
        "<cite>",
        "<editor>",
        "<coauthor>",
        "<partOf>",
        "<record>",
        "<predecessor>",
    ];

    for label in expected_labels {
        let result = graph.get_graph(label);
        assert!(result.is_ok());
    }
}

#[test]
fn test_mm_graph_nonexistent_label() {
    let graph = &INMEMORY_GRAPH;

    let result = graph.get_graph("<nonexistent_label>");
    assert!(result.is_err());
}

#[test]
fn test_mm_graph_nonexistent_node() {
    let graph = &INMEMORY_GRAPH;

    assert!(graph.get_node_id("<NonexistentNode>").is_none());

    assert!(graph.get_node_name(999999).is_none());
}

#[test]
fn test_mm_graph_specific_nodes_exist() {
    let graph = &INMEMORY_GRAPH;

    let nodes_to_check = vec![
        "<Article1>",
        "<Article20>",
        "<Article100>",
        "<Paul_Erdoes>",
        "<1940>",
        "<1950>",
        "<1960>",
        "<Inproceeding1>",
        "<Incollection1>",
    ];

    for node in nodes_to_check {
        assert!(graph.get_node_id(node).is_some());
    }
}

#[test]
fn test_mm_graph_matrix_dimensions() {
    let graph = &INMEMORY_GRAPH;

    let expected_labels = vec![
        "<journal>",
        "<creator>",
        "<references>",
        "<cite>",
        "<editor>",
        "<coauthor>",
        "<partOf>",
        "<record>",
        "<predecessor>",
    ];

    for label in expected_labels {
        let matrix = graph
            .get_graph(label)
            .expect(&format!("Should have matrix for label {}", label));
        matrix
            .check_graph()
            .expect(&format!("Matrix for {} should be valid", label));
    }
}

#[test]
fn test_mm_graph_load_from_nonexistent_dir() {
    let mm = MatrixMarket::from_dir("tests/testdata/nonexistent_dir");
    let result = Graph::<InMemory>::try_from(mm);

    assert!(
        result.is_err(),
        "Loading from nonexistent directory should fail"
    );
}

#[test]
fn test_mm_graph_edge_label_mapping() {
    let mm = MatrixMarket::from_dir("tests/testdata/mm_graph");
    let graph = Graph::<InMemory>::try_from(mm).expect("Failed to load graph");

    // Test that edge labels map correctly to their index files
    // From edges.txt:
    // <journal> 1 -> 1.txt
    // <creator> 2 -> 2.txt
    // <references> 3 -> 3.txt
    // etc.

    let label_to_file = vec![
        ("<journal>", "1.txt"),
        ("<creator>", "2.txt"),
        ("<references>", "3.txt"),
        ("<cite>", "4.txt"),
        ("<editor>", "5.txt"),
        ("<coauthor>", "6.txt"),
        ("<partOf>", "7.txt"),
        ("<record>", "8.txt"),
        ("<predecessor>", "9.txt"),
    ];

    for (label, _file) in label_to_file {
        let result = graph.get_graph(label);
        assert!(
            result.is_ok(),
            "Label {} should be loaded from its corresponding matrix file",
            label
        );
    }
}

#[test]
fn test_mm_graph_handles_large_indices() {
    let mm = MatrixMarket::from_dir("tests/testdata/mm_graph");
    let graph = Graph::<InMemory>::try_from(mm).expect("Failed to load graph");

    let num_nodes = graph.num_nodes();

    let high_index = num_nodes - 1;
    let name = graph.get_node_name(high_index);
    assert!(
        name.is_some(),
        "Should be able to retrieve node at last matrix index {}",
        high_index
    );
}

#[test]
fn test_mm_graph_empty_label_handling() {
    let mm = MatrixMarket::from_dir("tests/testdata/mm_graph");
    let graph = Graph::<InMemory>::try_from(mm).expect("Failed to load graph");

    // Test that empty string label is handled correctly
    let result = graph.get_graph("");
    assert!(result.is_err(), "Empty label should not exist in the graph");
}
