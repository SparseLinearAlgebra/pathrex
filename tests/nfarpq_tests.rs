use pathrex::graph::GraphDecomposition;
use pathrex::lagraph_sys::{GrB_Index, GrB_Vector_extractTuples_BOOL, GrB_Vector_nvals};
use pathrex::rpq::nfarpq::{NfaRpqEvaluator, NfaRpqResult};
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

fn reachable_indices(result: &NfaRpqResult) -> Vec<GrB_Index> {
    unsafe {
        let mut nvals: GrB_Index = 0;
        GrB_Vector_nvals(&mut nvals, result.reachable.inner);
        if nvals == 0 {
            return Vec::new();
        }
        let mut indices = vec![0u64; nvals as usize];
        let mut values = vec![false; nvals as usize];
        GrB_Vector_extractTuples_BOOL(
            indices.as_mut_ptr(),
            values.as_mut_ptr(),
            &mut nvals,
            result.reachable.inner,
        );
        indices.truncate(nvals as usize);
        indices
    }
}

fn reachable_count(result: &NfaRpqResult) -> u64 {
    unsafe {
        let mut nvals: GrB_Index = 0;
        GrB_Vector_nvals(&mut nvals, result.reachable.inner);
        nvals
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

    let count = reachable_count(&result);
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

    let indices = reachable_indices(&result);
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

    let count = reachable_count(&result);
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

    let indices = reachable_indices(&result);
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

    let indices = reachable_indices(&result);
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

    let indices = reachable_indices(&result);
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

    let indices = reachable_indices(&result);
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

    let indices = reachable_indices(&result);
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
        matches!(result, Err(RpqError::LabelNotFound(ref l)) if l == "nonexistent"),
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
    let evaluator = NfaRpqEvaluator;

    let path = PathExpr::ZeroOrMore(Box::new(label("knows")));

    let result = evaluator
        .evaluate(&rq(named_ep("A"), path, var("y")), &graph)
        .expect("evaluate should succeed");

    let count = reachable_count(&result);
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

    let indices = reachable_indices(&result);
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
        matches!(result, Err(RpqError::LabelNotFound(ref l)) if l == "likes"),
        "expected LabelNotFound for 'likes', got: {result:?}"
    );
}
