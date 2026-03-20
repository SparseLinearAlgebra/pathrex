use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::LazyLock;

use pathrex::formats::mm::MatrixMarket;
use pathrex::graph::{Graph, GraphDecomposition, GraphError, InMemory, InMemoryGraph};
use pathrex::lagraph_sys::{GrB_Index, GrB_Info, GrB_Matrix_extractElement_BOOL};
use pathrex::rpq::rpqmatrix::{RpqMatrixEvaluator, RpqMatrixResult};
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
    let query_str = line
        .splitn(2, ',')
        .nth(1)
        .unwrap_or_else(|| panic!("query line has no comma: {line:?}"))
        .trim();

    let sparql = format!("BASE <{BASE_IRI}> SELECT * WHERE {{ {query_str} . }}");

    let query =
        parse_rpq(&sparql).unwrap_or_else(|e| panic!("failed to parse query {line:?}: {e}"));
    query
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
    let evaluator = RpqMatrixEvaluator;

    for (i, (query, expected_nnz)) in queries.iter().zip(expected.iter()).enumerate() {
        let result = evaluator.evaluate(query, graph).unwrap_or_else(|e| {
            panic!("case '{case_name}' query #{i} evaluation failed: {e}\n  query: {query:?}")
        });

        assert_eq!(
            result.nnz,
            *expected_nnz,
            "case '{case_name}' query #{i} nnz mismatch\n  query:    {query:?}\n  expected: {expected_nnz}\n  actual:   {nnz}",
            nnz = result.nnz,
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

fn matrix_entry_set(result: &RpqMatrixResult, row: GrB_Index, col: GrB_Index) -> bool {
    unsafe {
        let mut x = false;
        let info = GrB_Matrix_extractElement_BOOL(&mut x, result.matrix.inner, row, col);
        info == GrB_Info::GrB_SUCCESS && x
    }
}

/// Graph: A --knows--> B --knows--> C
/// Query: ?x <knows> ?y
#[test]
fn test_single_label_variable_variable() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator
        .evaluate(&rq(var("x"), label("knows"), var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 2);
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows> ?y  → only A→B, nnz=1
#[test]
fn test_single_label_named_source() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator
        .evaluate(&rq(named_ep("A"), label("knows"), var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 1, "only A→B should be in result");
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let b_id = graph.get_node_id("B").expect("B should exist") as GrB_Index;
    assert!(
        matrix_entry_set(&result, a_id, b_id),
        "B should be reachable from A via 'knows'"
    );
}

/// Graph: A --knows--> B --likes--> C
/// Query: ?x <knows>/<likes> ?y  (two-hop sequence)
#[test]
fn test_sequence_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "likes")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(var("x"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 1);
}

/// Graph: A --knows--> B --likes--> C
/// Query: <A> <knows>/<likes> ?y  → only A→C, nnz=1
#[test]
fn test_sequence_path_named_source() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "likes")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 1, "only A→C should be in result");
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let c_id = graph.get_node_id("C").expect("C should exist") as GrB_Index;
    assert!(
        matrix_entry_set(&result, a_id, c_id),
        "C should be reachable from A via knows/likes"
    );
}

/// Graph: A --knows--> B, A --likes--> C
/// Query: <A> <knows>|<likes> ?y  → A→B and A→C, nnz=2
#[test]
fn test_alternative_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("A", "C", "likes")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::Alternative(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 2, "A→B and A→C should be in result");
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let b_id = graph.get_node_id("B").expect("B should exist") as GrB_Index;
    let c_id = graph.get_node_id("C").expect("C should exist") as GrB_Index;
    assert!(
        matrix_entry_set(&result, a_id, b_id),
        "B should be reachable via knows|likes"
    );
    assert!(
        matrix_entry_set(&result, a_id, c_id),
        "C should be reachable via knows|likes"
    );
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows>* ?y  → A→A, A→B, A→C, nnz=3
#[test]
fn test_zero_or_more_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 3, "A, B, C all reachable from A via knows*");
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let b_id = graph.get_node_id("B").expect("B should exist") as GrB_Index;
    let c_id = graph.get_node_id("C").expect("C should exist") as GrB_Index;

    assert!(
        matrix_entry_set(&result, a_id, a_id),
        "A should be reachable (zero hops)"
    );
    assert!(
        matrix_entry_set(&result, a_id, b_id),
        "B should be reachable (one hop)"
    );
    assert!(
        matrix_entry_set(&result, a_id, c_id),
        "C should be reachable (two hops)"
    );
}

/// Graph: A --knows--> B --knows--> C
/// Query: <A> <knows>+ ?y  → A→B, A→C (not A→A), nnz=2
#[test]
fn test_one_or_more_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::OneOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(result.nnz, 2, "B and C reachable from A via knows+");
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let b_id = graph.get_node_id("B").expect("B should exist") as GrB_Index;
    let c_id = graph.get_node_id("C").expect("C should exist") as GrB_Index;

    assert!(
        !matrix_entry_set(&result, a_id, a_id),
        "A shouldn't be reachable (non-zero length)"
    );
    assert!(
        matrix_entry_set(&result, a_id, b_id),
        "B should be reachable (one hop)"
    );
    assert!(
        matrix_entry_set(&result, a_id, c_id),
        "C should be reachable (two hops)"
    );
}

#[test]
fn test_zero_or_one_unsupported() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::ZeroOrOne(Box::new(label("knows")));
    let result = evaluator.evaluate(&rq(var("x"), path, var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::UnsupportedPath(_))),
        "expected UnsupportedPath for ZeroOrOne, got: {result:?}"
    );
}

#[test]
fn test_label_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator.evaluate(&rq(var("x"), label("nonexistent"), var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::Graph(GraphError::LabelNotFound(ref l))) if l == "nonexistent"),
        "expected LabelNotFound error, got: {result:?}"
    );
}

#[test]
fn test_vertex_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator.evaluate(&rq(named_ep("Z"), label("knows"), var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::VertexNotFound(ref v)) if v == "Z"),
        "expected VertexNotFound error, got: {result:?}"
    );
}

/// Graph: A --knows--> B, C --knows--> D
/// Query: ?x <knows> <B>  → only A→B, nnz=1
#[test]
fn test_bound_object() {
    let graph = build_graph(&[("A", "B", "knows"), ("C", "D", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator
        .evaluate(&rq(var("x"), label("knows"), named_ep("B")), &graph)
        .expect("bound object should be supported");

    assert_eq!(result.nnz, 1, "only A→B should be in result");
}

/// Graph: A --knows--> B, C --knows--> D
/// Query: <A> <knows> <B>  → nnz=1
#[test]
fn test_bound_subject_and_object() {
    let graph = build_graph(&[("A", "B", "knows"), ("C", "D", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator
        .evaluate(&rq(named_ep("A"), label("knows"), named_ep("B")), &graph)
        .expect("bound subject+object should be supported");

    assert_eq!(result.nnz, 1, "only A→B should be in result");
}

#[test]
fn test_negated_property_set_rejected_by_sparql_conversion() {
    let sparql = "BASE <http://example.org/> SELECT ?x ?y WHERE { ?x !(<knows>) ?y . }";
    let r = pathrex::sparql::parse_rpq(sparql);
    assert!(matches!(r, Err(RpqError::UnsupportedPath(_))));
}

/// Graph: A --knows--> B --knows--> C --knows--> A  (cycle)
/// Query: <A> <knows>* ?y  → all 3 nodes reachable, nnz=3
#[test]
fn test_cycle_graph_star() {
    let graph = build_graph(&[
        ("A", "B", "knows"),
        ("B", "C", "knows"),
        ("C", "A", "knows"),
    ]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    assert_eq!(
        result.nnz, 3,
        "all 3 nodes should be reachable from A in a cycle"
    );
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let targets = {
        let n = graph.num_nodes();
        let mut out = Vec::new();
        for j in 0..n as GrB_Index {
            if matrix_entry_set(&result, a_id, j) {
                out.push(j);
            }
        }
        out
    };
    assert_eq!(
        targets.len(),
        3,
        "all 3 nodes should be reachable from A in a cycle"
    );
}

/// Graph: A --knows--> B --likes--> C --knows--> D
/// Query: <A> <knows>/<likes>*/<knows> ?y  → A→D, nnz=1
#[test]
fn test_complex_path() {
    let graph = build_graph(&[
        ("A", "B", "knows"),
        ("B", "C", "likes"),
        ("C", "D", "knows"),
    ]);
    let evaluator = RpqMatrixEvaluator;

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

    assert_eq!(result.nnz, 1, "only A→D should be in result");
    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let d_id = graph.get_node_id("D").expect("D should exist") as GrB_Index;
    assert!(
        matrix_entry_set(&result, a_id, d_id),
        "D should be reachable via knows/likes*/knows"
    );
}

#[test]
fn test_no_matching_path() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator.evaluate(&rq(var("x"), path, var("y")), &graph);

    assert!(
        matches!(result, Err(RpqError::Graph(GraphError::LabelNotFound(ref l))) if l == "likes"),
        "expected LabelNotFound for 'likes', got: {result:?}"
    );
}

#[test]
fn test_la_n_egg_any_any() {
    run_la_n_egg_case("any-any");
}

#[test]
fn test_la_n_egg_any_con() {
    run_la_n_egg_case("any-con");
}

#[test]
fn test_la_n_egg_con_any() {
    run_la_n_egg_case("con-any");
}
