use pathrex::formats::csv::Csv;
use pathrex::utils::build_graph;
use pathrex::graph::{
    Edge, Graph, GraphBuilder, GraphDecomposition, GraphError, InMemory, InMemoryBuilder,
};

#[test]
fn node_ids_are_unique() {
    let graph = build_graph(&[
        ("A", "B", "r"),
        ("B", "C", "r"),
        ("C", "D", "r"),
        ("D", "A", "r"),
    ]);

    assert_eq!(graph.num_nodes(), 4);

    let ids: Vec<usize> = ["A", "B", "C", "D"]
        .iter()
        .map(|n| graph.get_node_id(n).expect("node must exist"))
        .collect();

    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 4, "node IDs must be unique: {ids:?}");
}

#[test]
fn multi_label_graph_has_all_labels() {
    let graph = build_graph(&[
        ("A", "B", "knows"),
        ("B", "C", "knows"),
        ("A", "C", "likes"),
        ("C", "D", "hates"),
    ]);

    for label in &["knows", "likes", "hates"] {
        assert!(
            graph.get_graph(label).is_ok(),
            "label '{label}' must be present"
        );
    }
    assert!(matches!(
        graph.get_graph("nonexistent"),
        Err(GraphError::LabelNotFound(_))
    ));
}

#[test]
fn self_loop_edge_produces_one_node() {
    let graph = build_graph(&[("A", "A", "self")]);

    assert_eq!(graph.num_nodes(), 1);
    assert!(graph.get_graph("self").is_ok());

    let id = graph.get_node_id("A").expect("X must exist");
    assert_eq!(graph.get_node_name(id).as_deref(), Some("A"));
}

#[test]
fn duplicate_edges_do_not_increase_node_count() {
    let graph = build_graph(&[("A", "B", "r"), ("A", "B", "r"), ("A", "B", "r")]);

    assert_eq!(graph.num_nodes(), 2);
    assert!(graph.get_graph("r").is_ok());
}

#[test]
fn incremental_push_edge_accumulates_correctly() {
    let mut builder = InMemoryBuilder::new();

    builder
        .push_edge(Edge {
            source: "A".into(),
            target: "B".into(),
            label: "r".into(),
        })
        .expect("first push_edge must succeed");

    builder
        .push_edge(Edge {
            source: "B".into(),
            target: "C".into(),
            label: "r".into(),
        })
        .expect("second push_edge must succeed");

    builder
        .push_edge(Edge {
            source: "A".into(),
            target: "C".into(),
            label: "s".into(),
        })
        .expect("third push_edge must succeed");

    let graph = builder.build().expect("build must succeed");

    assert_eq!(graph.num_nodes(), 3);
    assert!(graph.get_graph("r").is_ok());
    assert!(graph.get_graph("s").is_ok());
}

#[test]
fn with_stream_from_ok_iterator() {
    let edges: Vec<Result<Edge, GraphError>> = vec![
        Ok(Edge {
            source: "A".into(),
            target: "B".into(),
            label: "r".into(),
        }),
        Ok(Edge {
            source: "B".into(),
            target: "C".into(),
            label: "r".into(),
        }),
        Ok(Edge {
            source: "A".into(),
            target: "C".into(),
            label: "s".into(),
        }),
    ];

    let builder = InMemoryBuilder::new();

    let graph = builder
        .with_stream(edges)
        .expect("with_stream must succeed")
        .build()
        .expect("build must succeed");

    assert_eq!(graph.num_nodes(), 3);
    assert!(graph.get_graph("r").is_ok());
    assert!(graph.get_graph("s").is_ok());
}

#[test]
fn load_from_empty_csv_produces_empty_graph() {
    let csv = "source,target,label\n";
    let csv = Csv::from_reader(csv.as_bytes()).expect("Csv::from_reader must succeed");

    let graph = Graph::<InMemory>::try_from(csv).expect("Shoudl build from empty csv");

    assert_eq!(graph.num_nodes(), 0);
}

#[test]
fn two_sequential_csv_loads_merge_into_one_graph() {
    let csv1 = "source,target,label\nA,B,r\n";
    let csv2 = "source,target,label\nC,D,s\n";

    let iter1 = Csv::from_reader(csv1.as_bytes()).expect("csv1 must parse");
    let iter2 = Csv::from_reader(csv2.as_bytes()).expect("csv2 must parse");

    let graph = InMemoryBuilder::default()
        .load(iter1)
        .expect("first load must succeed")
        .load(iter2)
        .expect("second load must succeed")
        .build()
        .expect("build must succeed");

    assert_eq!(graph.num_nodes(), 4);
    assert!(graph.get_graph("r").is_ok());
    assert!(graph.get_graph("s").is_ok());
}

#[test]
fn large_graph_node_and_label_counts() {
    let mut builder = InMemoryBuilder::new();

    for i in 0u32..999 {
        builder
            .push_edge(Edge {
                source: i.to_string(),
                target: (i + 1).to_string(),
                label: "chain".into(),
            })
            .expect("push_edge must succeed");
    }

    for i in 0u32..500 {
        builder
            .push_edge(Edge {
                source: "hub".into(),
                target: format!("spoke_{i}"),
                label: "star".into(),
            })
            .expect("push_edge must succeed");
    }

    let graph = builder.build().expect("build must succeed");

    assert_eq!(graph.num_nodes(), 1_501);
    assert!(graph.get_graph("chain").is_ok());
    assert!(graph.get_graph("star").is_ok());
    assert!(matches!(
        graph.get_graph("missing"),
        Err(GraphError::LabelNotFound(_))
    ));
    graph
        .get_graph("chain")
        .expect("exists")
        .check_graph()
        .expect("valid graph");
    graph
        .get_graph("chain")
        .expect("exists")
        .check_graph()
        .expect("valid graph");
}
