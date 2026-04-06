use pathrex::graph::GraphDecomposition;
use pathrex::lagraph_sys::{GrB_Index, GrB_Info, GrB_Matrix_extractElement_BOOL};
use pathrex::rpq::rpqmatrix::{RpqMatrixEvaluator, RpqMatrixResult};
use pathrex::rpq::{Endpoint, PathExpr, RpqError, RpqEvaluator, RpqQuery};
use pathrex::utils::build_graph;

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

fn row_targets<G: GraphDecomposition>(
    result: &RpqMatrixResult,
    graph: &G,
    row: GrB_Index,
) -> Vec<GrB_Index> {
    let n = graph.num_nodes();
    let mut out = Vec::new();
    for j in 0..n as GrB_Index {
        if matrix_entry_set(result, row, j) {
            out.push(j);
        }
    }
    out
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
/// Query: <A> <knows> ?y
#[test]
fn test_single_label_named_source() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator
        .evaluate(&rq(named_ep("A"), label("knows"), var("y")), &graph)
        .expect("evaluate should succeed");

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
/// Query: <A> <knows>/<likes> ?y
#[test]
fn test_sequence_path_named_source() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "likes")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::Sequence(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let c_id = graph.get_node_id("C").expect("C should exist") as GrB_Index;
    assert!(
        matrix_entry_set(&result, a_id, c_id),
        "C should be reachable from A via knows/likes"
    );
}

/// Graph: A --knows--> B, A --likes--> C
/// Query: <A> <knows>|<likes> ?y
#[test]
fn test_alternative_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("A", "C", "likes")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::Alternative(Box::new(label("knows")), Box::new(label("likes")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

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
/// Query: <A> <knows>* ?y
#[test]
fn test_zero_or_more_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

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
/// Query: <A> <knows>+ ?y
#[test]
fn test_one_or_more_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("B", "C", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::OneOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

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
        matches!(result, Err(RpqError::LabelNotFound(ref l)) if l == "nonexistent"),
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

#[test]
fn test_bound_object_unsupported() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = RpqMatrixEvaluator;

    let result = evaluator.evaluate(&rq(var("x"), label("knows"), named_ep("B")), &graph);

    assert!(
        matches!(result, Err(RpqError::UnsupportedPath(_))),
        "expected UnsupportedPath for bound object, got: {result:?}"
    );
}

#[test]
fn test_negated_property_set_rejected_by_sparql_conversion() {
    let sparql = "BASE <http://example.org/> SELECT ?x ?y WHERE { ?x !(<knows>) ?y . }";
    let r = pathrex::sparql::parse_rpq(sparql);
    assert!(matches!(
        r,
        Err(pathrex::sparql::RpqParseError::UnsupportedPath(_))
    ));
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
    let evaluator = RpqMatrixEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let a_id = graph.get_node_id("A").expect("A should exist") as GrB_Index;
    let targets = row_targets(&result, &graph, a_id);
    assert_eq!(
        targets.len(),
        3,
        "all 3 nodes should be reachable from A in a cycle"
    );
}

/// Graph: A --knows--> B --likes--> C --knows--> D
/// Query: <A> <knows>/<likes>*/<knows> ?y
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
        matches!(result, Err(RpqError::LabelNotFound(ref l)) if l == "likes"),
        "expected LabelNotFound for 'likes', got: {result:?}"
    );
}
