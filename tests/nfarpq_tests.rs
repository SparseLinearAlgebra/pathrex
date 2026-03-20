use pathrex::graph::GraphDecomposition;
use pathrex::lagraph_sys::{GrB_Index, GrB_Vector_extractTuples_BOOL, GrB_Vector_nvals};
use pathrex::rpq::nfarpq::NfaRpqEvaluator;
use pathrex::rpq::{RpqError, RpqEvaluator, RpqResult};
use pathrex::utils::build_graph;
use spargebra::algebra::PropertyPathExpression;
use spargebra::term::{NamedNode, TermPattern, Variable};

fn named(iri: &str) -> PropertyPathExpression {
    PropertyPathExpression::NamedNode(NamedNode::new_unchecked(iri))
}

fn var(name: &str) -> TermPattern {
    TermPattern::Variable(Variable::new_unchecked(name))
}

fn named_term(iri: &str) -> TermPattern {
    TermPattern::NamedNode(NamedNode::new_unchecked(iri))
}

fn reachable_indices(result: &RpqResult) -> Vec<GrB_Index> {
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

fn reachable_count(result: &RpqResult) -> u64 {
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
        .evaluate(&var("x"), &named("knows"), &var("y"), &graph)
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
        .evaluate(&named_term("A"), &named("knows"), &var("y"), &graph)
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

    let path = PropertyPathExpression::Sequence(Box::new(named("knows")), Box::new(named("likes")));

    let result = evaluator
        .evaluate(&var("x"), &path, &var("y"), &graph)
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

    let path = PropertyPathExpression::Sequence(Box::new(named("knows")), Box::new(named("likes")));

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
        .expect("evaluate should succeed");

    let indices = reachable_indices(&result);
    let c_id = graph.get_node_id("C").expect("C should exist");
    assert!(
        indices.contains(&(c_id as GrB_Index)),
        "C (id={c_id}) should be reachable from A via knows/likes, got indices: {indices:?}"
    );
}

/// Graph: A --knows--> B, A --likes--> C
/// Query: ?x <knows>|<likes> ?y
#[test]
fn test_alternative_path() {
    let graph = build_graph(&[("A", "B", "knows"), ("A", "C", "likes")]);
    let evaluator = NfaRpqEvaluator;

    let path =
        PropertyPathExpression::Alternative(Box::new(named("knows")), Box::new(named("likes")));

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
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

    let path = PropertyPathExpression::ZeroOrMore(Box::new(named("knows")));

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
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

    let path = PropertyPathExpression::OneOrMore(Box::new(named("knows")));

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
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

    let path = PropertyPathExpression::ZeroOrOne(Box::new(named("knows")));

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
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

    let result = evaluator.evaluate(&var("x"), &named("nonexistent"), &var("y"), &graph);

    assert!(
        matches!(result, Err(RpqError::LabelNotFound(ref l)) if l == "nonexistent"),
        "expected LabelNotFound error, got: {result:?}"
    );
}

#[test]
fn test_vertex_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator.evaluate(&named_term("Z"), &named("knows"), &var("y"), &graph);

    assert!(
        matches!(result, Err(RpqError::VertexNotFound(ref v)) if v == "Z"),
        "expected VertexNotFound error, got: {result:?}"
    );
}

#[test]
fn test_object_vertex_not_found() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let result = evaluator.evaluate(&var("x"), &named("knows"), &named_term("Z"), &graph);

    assert!(
        matches!(result, Err(RpqError::VertexNotFound(ref v)) if v == "Z"),
        "expected VertexNotFound error for object, got: {result:?}"
    );
}

#[test]
fn test_reverse_path_unsupported() {
    let graph = build_graph(&[("A", "B", "knows")]);
    let evaluator = NfaRpqEvaluator;

    let path = PropertyPathExpression::Reverse(Box::new(named("knows")));
    let result = evaluator.evaluate(&var("x"), &path, &var("y"), &graph);

    assert!(
        matches!(result, Err(RpqError::UnsupportedPath(_))),
        "expected UnsupportedPath error, got: {result:?}"
    );
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

    let path = PropertyPathExpression::ZeroOrMore(Box::new(named("knows")));

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
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
    let path = PropertyPathExpression::Sequence(
        Box::new(PropertyPathExpression::Sequence(
            Box::new(named("knows")),
            Box::new(PropertyPathExpression::ZeroOrMore(Box::new(named("likes")))),
        )),
        Box::new(named("knows")),
    );

    let result = evaluator
        .evaluate(&named_term("A"), &path, &var("y"), &graph)
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

    let path = PropertyPathExpression::Sequence(Box::new(named("knows")), Box::new(named("likes")));

    let result = evaluator.evaluate(&var("x"), &path, &var("y"), &graph);

    assert!(
        matches!(result, Err(RpqError::LabelNotFound(ref l)) if l == "likes"),
        "expected LabelNotFound for 'likes', got: {result:?}"
    );
}
