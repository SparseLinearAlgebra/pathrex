use pathrex::formats::csv::Csv;
use pathrex::graph::{
    Edge, Graph, GraphBuilder, GraphDecomposition, GraphError, InMemory, InMemoryBuilder,
};
use pathrex::utils::build_graph;
use std::collections::HashMap;

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

/// get_node_id and get_node_name must be inverses of each other for every node.
#[test]
fn dictionary_round_trip_for_every_node() {
    let names = ["Alice", "Bob", "Charlie", "Dave", "Eve"];
    let mut builder = InMemoryBuilder::new();
    for (i, &name) in names.iter().enumerate() {
        builder
            .push_edge(Edge {
                source: name.to_owned(),
                target: names[(i + 1) % names.len()].to_owned(),
                label: "r".to_owned(),
            })
            .unwrap();
    }
    let graph = builder.build().unwrap();

    assert_eq!(graph.num_nodes(), names.len());

    for name in &names {
        let id = graph.get_node_id(name).expect("must resolve name -> id");
        let resolved = graph.get_node_name(id).expect("must resolve id -> name");
        assert_eq!(resolved, *name, "round-trip must preserve name");
    }
}

/// After set_node_map (the MatrixMarket code path) the dictionary must still
/// resolve every installed node by name and by id.
#[test]
fn set_node_map_preserves_dictionary_round_trip() {
    // Simulate what the MatrixMarket loader does: build two parallel maps with
    // 0-based ids and call set_node_map, then build without pushing edges.
    let nodes: Vec<(&str, usize)> = vec![
        ("http://example.org/A", 0),
        ("http://example.org/B", 1),
        ("http://example.org/C", 2),
    ];

    let mut id_to_name: HashMap<usize, String> = HashMap::new();
    let mut name_to_id: HashMap<String, usize> = HashMap::new();
    for (name, id) in &nodes {
        id_to_name.insert(*id, name.to_string());
        name_to_id.insert(name.to_string(), *id);
    }

    let mut builder = InMemoryBuilder::new();
    builder.set_node_map(id_to_name, name_to_id);
    let graph = builder.build().unwrap();

    assert_eq!(graph.num_nodes(), nodes.len());

    for (name, expected_id) in &nodes {
        let id = graph
            .get_node_id(name)
            .expect("must resolve name after set_node_map");
        assert_eq!(id, *expected_id, "id must match what was installed");
        let resolved = graph
            .get_node_name(id)
            .expect("must resolve id after set_node_map");
        assert_eq!(resolved, *name, "name round-trip must hold");
    }
}

/// Interning the same node name via multiple push_edge calls must not increase
/// num_nodes beyond the number of distinct names.
#[test]
fn repeated_push_edge_does_not_inflate_num_nodes() {
    let mut builder = InMemoryBuilder::new();
    for _ in 0..100 {
        builder
            .push_edge(Edge {
                source: "X".to_owned(),
                target: "Y".to_owned(),
                label: "r".to_owned(),
            })
            .unwrap();
    }
    let graph = builder.build().unwrap();
    // Only two distinct nodes: X and Y.
    assert_eq!(graph.num_nodes(), 2);
    assert!(graph.get_node_id("X").is_some());
    assert!(graph.get_node_id("Y").is_some());
    assert!(graph.get_node_id("Z").is_none());
}

/// Node IDs must be contiguous in [0, num_nodes) so that they are valid matrix
/// indices for GraphBLAS.
#[test]
fn node_ids_are_in_range() {
    let graph = build_graph(&[
        ("n0", "n1", "r"),
        ("n1", "n2", "r"),
        ("n2", "n3", "r"),
        ("n3", "n0", "r"),
    ]);
    let n = graph.num_nodes();
    assert_eq!(n, 4);
    let mut ids: Vec<usize> = ["n0", "n1", "n2", "n3"]
        .iter()
        .map(|name| graph.get_node_id(name).unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 2, 3], "IDs must cover 0..num_nodes exactly");
}
