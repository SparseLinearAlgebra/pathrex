use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::LazyLock;

use pathrex::formats::mm::MatrixMarket;
use pathrex::graph::{Graph, GraphDecomposition, GraphError, InMemory, InMemoryGraph};
use pathrex::lagraph_sys::GrB_Index;
use pathrex::rpq::nfarpq::NfaRpqEvaluator;
use pathrex::rpq::{Endpoint, PathExpr, RpqError, RpqEvaluator, RpqQuery};
use pathrex::sparql::parse_rpq;
use pathrex::utils::build_graph;

const GRAPH_DIR: &str = "tests/testdata/mm_graph";
const CASES_DIR: &str = "tests/testdata/cases";
const BASE_IRI: &str = "http://example.org/";

static LA_N_EGG_GRAPH: LazyLock<InMemoryGraph> = LazyLock::new(|| {
    let mm = MatrixMarket::from_dir(GRAPH_DIR).with_base_iri(BASE_IRI);
    Graph::<InMemory>::try_from(mm).expect("Failed to load la-n-egg-rpq graph")
});

fn convert_query_line(line: &str) -> RpqQuery {
    let query_str = line.split_once(',').map(|x| x.1)
        .unwrap_or_else(|| panic!("query line has no comma: {line:?}"))
        .trim();

    let sparql = format!("BASE <{BASE_IRI}> SELECT * WHERE {{ {query_str} . }}");

    
    parse_rpq(&sparql).unwrap_or_else(|e| panic!("failed to parse query {line:?}: {e}"))
}

fn load_queries(case_dir: &Path) -> Vec<RpqQuery> {
    let path = case_dir.join("queries.txt");
    let reader = BufReader::new(
        File::open(&path).unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display())),
    );
    reader
        .lines()
        .map(|l| l.expect("I/O error reading queries.txt"))
        .filter(|l| !l.trim().is_empty())
        .map(|l| convert_query_line(&l))
        .collect()
}

fn load_expected_nnz(case_dir: &Path) -> Vec<u64> {
    let path = case_dir.join("expected.txt");
    let reader = BufReader::new(
        File::open(&path).unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display())),
    );
    reader
        .lines()
        .map(|l| l.expect("I/O error reading expected.txt"))
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            // Format: "<id>;<nnz>;<cost>"
            let mut parts = l.splitn(3, ';');
            let _id = parts.next().expect("missing id field");
            parts
                .next()
                .expect("missing nnz field")
                .parse::<u64>()
                .unwrap_or_else(|e| panic!("bad nnz in {l:?}: {e}"))
        })
        .collect()
}

fn run_la_n_egg_case(case_name: &str) {
    let case_dir = Path::new(CASES_DIR).join(case_name);
    let queries = load_queries(&case_dir);
    let expected = load_expected_nnz(&case_dir);

    assert_eq!(
        queries.len(),
        expected.len(),
        "case '{case_name}': queries.txt and expected.txt have different line counts"
    );

    let graph = &*LA_N_EGG_GRAPH;
    let evaluator = NfaRpqEvaluator;

    for (i, (query, expected_nnz)) in queries.iter().zip(expected.iter()).enumerate() {
        let result = evaluator.evaluate(query, graph).unwrap_or_else(|e| {
            panic!("case '{case_name}' query #{i} evaluation failed: {e}\n  query: {query:?}")
        });

        let actual_nnz = result.reachable.nvals().expect("failed to get nvals");
        assert_eq!(
            actual_nnz, *expected_nnz,
            "case '{case_name}' query #{i} nnz mismatch\n  query:    {query:?}\n  expected: {expected_nnz}\n  actual:   {actual_nnz}",
        );
    }
}

fn label(s: &str) -> PathExpr {
    PathExpr::Label(s.to_string())
}

fn var(name: &str) -> Endpoint {
    Endpoint::Variable(name.to_string())
}

fn named_ep(s: &str) -> Endpoint {
    Endpoint::Named(s.to_string())
}

fn rq(subject: Endpoint, path: PathExpr, object: Endpoint) -> RpqQuery {
    RpqQuery {
        subject,
        path,
        object,
    }
}

/// Graph: A --knows--> B --knows--> C
/// Query: ?x <knows> ?y
#[test]
fn test_single_label_variable_variable() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator
        .evaluate(&rq(var("x"), label("knows"), var("y")), &graph)
        .expect("evaluate should succeed");

    let count = result.reachable.nvals().expect("failed to get nvals");
    assert_eq!(count, 2);
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows> ?y
#[test]
fn test_single_label_named_source() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator
        .evaluate(&rq(named_ep("A"), label("knows"), var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let b_id = graph.get_node_id("B").expect("B should exist");
    assert!(
        indices.contains(&(b_id as GrB_Index)),
        "B (id={b_id}) should be reachable from A via 'knows', got indices: {indices:?}"
    );
}

/// Graph: A --knows--> B --likes--> C
/// Query: ?x <knows>/<likes> ?y  (two-hop sequence)
#[test]
fn test_sequence_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "likes")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(var("x"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let count = result.reachable.nvals().expect("failed to get nvals");
    assert_eq!(count, 1);
}

/// Graph: A --knows--> B --likes--> C
/// Query: <A> <knows>/<likes> ?y
#[test]
fn test_sequence_path_named_source() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "likes")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let c_id = graph.get_node_id("C").expect("C should exist");
    assert!(
        indices.contains(&(c_id as GrB_Index)),
        "C (id={c_id}) should be reachable from A via knows/likes, got indices: {indices:?}"
    );
}

/// Graph: A --knows--> B, A --likes--> C
/// Query: <A> <knows>|<likes> ?y
#[test]
fn test_alternative_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("A", "C", "likes")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::Alternative(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let b_id = graph.get_node_id("B").expect("B should exist");
    let c_id = graph.get_node_id("C").expect("C should exist");
    assert!(
        indices.contains(&(b_id as GrB_Index)),
        "B should be reachable via knows|likes"
    );
    assert!(
        indices.contains(&(c_id as GrB_Index)),
        "C should be reachable via knows|likes"
    );
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows>* ?y
#[test]
fn test_zero_or_more_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let a_id = graph.get_node_id("A").expect("A should exist");
    let b_id = graph.get_node_id("B").expect("B should exist");
    let c_id = graph.get_node_id("C").expect("C should exist");

    assert!(
        indices.contains(&(a_id as GrB_Index)),
        "A should be reachable (zero hops)"
    );
    assert!(
        indices.contains(&(b_id as GrB_Index)),
        "B should be reachable (one hop)"
    );
    assert!(
        indices.contains(&(c_id as GrB_Index)),
        "C should be reachable (two hops)"
    );
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows>+ ?y
#[test]
fn test_one_or_more_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::OneOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let a_id = graph.get_node_id("A").expect("A should exist");
    let b_id = graph.get_node_id("B").expect("B should exist");
    let c_id = graph.get_node_id("C").expect("C should exist");

    assert!(
        !indices.contains(&(a_id as GrB_Index)),
        "A shouldn't be reachable"
    );
    assert!(
        indices.contains(&(b_id as GrB_Index)),
        "B should be reachable (one hop)"
    );
    assert!(
        indices.contains(&(c_id as GrB_Index)),
        "C should be reachable (two hops)"
    );
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows>? ?y
#[test]
fn test_zero_or_one_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::ZeroOrOne(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let a_id = graph.get_node_id("A").expect("A should exist");
    let b_id = graph.get_node_id("B").expect("B should exist");
    let c_id = graph.get_node_id("C").expect("C should exist");

    assert!(
        indices.contains(&(a_id as GrB_Index)),
        "A should be reachable (zero hops)"
    );
    assert!(
        indices.contains(&(b_id as GrB_Index)),
        "B should be reachable (one hop)"
    );
    assert!(
        !indices.contains(&(c_id as GrB_Index)),
        "C should NOT be reachable (two hops, but path is ?)"
    );
}

#[test]
fn test_label_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator.evaluate(&rq(var("x"), label("nonexistent"), var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::Graph(GraphError::LabelNotFound(ref l))) if l == "nonexistent"),
        "expected LabelNotFound error, got: {result:?}"
    );
}

#[test]
fn test_vertex_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator.evaluate(&rq(named_ep("Z"), label("knows"), var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::VertexNotFound(ref v)) if v == "Z"),
        "expected VertexNotFound error, got: {result:?}"
    );
}

#[test]
fn test_object_vertex_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator.evaluate(&rq(var("x"), label("knows"), named_ep("Z")), &graph);

    assert!(
        matches!(result, Err(RpqError::VertexNotFound(ref v)) if v == "Z"),
        "expected VertexNotFound error for object, got: {result:?}"
    );
}

#[test]
fn test_negated_property_set_rejected_by_sparql_conversion() {
    let sparql = "BASE <http://example.org/> SELECT ?x ?y WHERE { ?x !(<knows>) ?y . }";
    let r = pathrex::sparql::parse_rpq(sparql);
    assert!(matches!(r, Err(RpqError::UnsupportedPath(_))));
}

/// Graph: A --knows--> B --knows--> C --knows--> A  (cycle)
/// Query: <A> <knows>* ?y
#[test]
fn test_cycle_graph_star() {
    let graph = build_graph(&[
        ("A", "B", "knows"),
        ("B", "C", "knows"),
        ("C", "A", "knows"),
    ]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let count = result.reachable.nvals().expect("failed to get nvals");
    assert_eq!(count, 3, "all 3 nodes should be reachable in a cycle");
}

/// Graph: A --knows--> B --likes--> C --knows--> D
/// Query: ?x <knows>/<likes>*/<knows> ?y
#[test]
fn test_complex_path() {
    let graph = build_graph(&[
        ("A", "B", "knows"),
        ("B", "C", "likes"),
        ("C", "D", "knows"),
    ]);
    let evaluator = NfaRpqEvaluator;

    // knows / likes* / knows
    let path = PathExpr::Sequence(
        Box::new(PathExpr::Sequence(
            Box::new(label("knows")),
            Box::new(PathExpr::ZeroOrMore(Box::new(label("likes")))),
        )),
        Box::new(label("knows")),
    );

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let indices = result
        .reachable
        .indices()
        .expect("failed to extract indices");
    let d_id = graph.get_node_id("D").expect("D should exist");
    assert!(
        indices.contains(&(d_id as GrB_Index)),
        "D should be reachable via knows/likes*/knows, got indices: {indices:?}"
    );
}

#[test]
fn test_no_matching_path() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator.evaluate(&rq(var("x"), path, var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::Graph(GraphError::LabelNotFound(ref l))) if l == "likes"),
        "expected LabelNotFound for 'likes', got: {result:?}"
    );
}

#[test]
fn test_la_n_egg_con_any() {
    run_la_n_egg_case("con-any");
}
