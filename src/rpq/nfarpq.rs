//! NFA-based RPQ evaluation using `LAGraph_RegularPathQuery`.

use crate::graph::{
    GraphDecomposition, GraphError, GraphblasVector, LagraphGraph, ensure_grb_init,
};
use crate::grb_ok;
use crate::la_ok;
use crate::lagraph_sys::*;
use crate::lagraph_sys::{GrB_BOOL, GrB_LOR, GrB_Matrix_build_BOOL, GrB_Matrix_new, LAGraph_Kind};
use crate::rpq::{RpqError, RpqEvaluator, RpqResult};
use spargebra::algebra::PropertyPathExpression;
use spargebra::term::TermPattern;
use std::collections::{HashMap, HashSet, VecDeque};

/// Transitions for a single edge label in the NFA.
///
/// `rows[i]` and `cols[i]` form a parallel pair: there is a transition from
/// state `rows[i]` to state `cols[i]` on `label`.
#[derive(Debug, Clone)]
pub struct NfaLabelTransitions {
    pub label: String,
    pub rows: Vec<GrB_Index>,
    pub cols: Vec<GrB_Index>,
}

#[derive(Debug, Clone)]
pub struct Nfa {
    pub num_states: usize,
    pub start_states: Vec<GrB_Index>,
    pub final_states: Vec<GrB_Index>,
    pub transitions: Vec<NfaLabelTransitions>,
}

impl Nfa {
    pub fn from_property_path(path: &PropertyPathExpression) -> Result<Self, RpqError> {
        let mut builder = NfaBuilder::new();
        let (start, end) = builder.build(path)?;
        builder.mark_start(start);
        builder.mark_final(end);
        Ok(builder.into_nfa())
    }

    pub fn build_lagraph_matrices(&self) -> Result<Vec<(String, LagraphGraph)>, RpqError> {
        ensure_grb_init().map_err(|e: GraphError| RpqError::GraphBlas(format!("{e}")))?;
        let n = self.num_states as GrB_Index;
        let mut result = Vec::with_capacity(self.transitions.len());

        for trans in &self.transitions {
            let mut mat: GrB_Matrix = std::ptr::null_mut();
            grb_ok!(GrB_Matrix_new(&mut mat, GrB_BOOL, n, n))
                .map_err(|e: GraphError| RpqError::GraphBlas(format!("{e}")))?;

            if !trans.rows.is_empty() {
                let vals: Vec<bool> = vec![true; trans.rows.len()];
                grb_ok!(GrB_Matrix_build_BOOL(
                    mat,
                    trans.rows.as_ptr(),
                    trans.cols.as_ptr(),
                    vals.as_ptr(),
                    trans.rows.len() as u64,
                    GrB_LOR,
                ))
                .map_err(|e: GraphError| RpqError::GraphBlas(format!("{e}")))?;
            }

            let lg = LagraphGraph::new(mat, LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED)
                .map_err(|e| RpqError::GraphBlas(format!("{e}")))?;
            result.push((trans.label.clone(), lg));
        }

        Ok(result)
    }
}

#[derive(Debug, Clone)]
struct Transition {
    from: usize,
    to: usize,
    label: Option<String>,
}

struct NfaBuilder {
    num_states: usize,
    transitions: Vec<Transition>,
    start_states: Vec<usize>,
    final_states: Vec<usize>,
}

impl NfaBuilder {
    fn new() -> Self {
        Self {
            num_states: 0,
            transitions: Vec::new(),
            start_states: Vec::new(),
            final_states: Vec::new(),
        }
    }

    fn new_state(&mut self) -> usize {
        let s = self.num_states;
        self.num_states += 1;
        s
    }

    fn add_epsilon(&mut self, from: usize, to: usize) {
        self.transitions.push(Transition {
            from,
            to,
            label: None,
        });
    }

    fn add_label(&mut self, from: usize, to: usize, label: String) {
        self.transitions.push(Transition {
            from,
            to,
            label: Some(label),
        });
    }

    fn mark_start(&mut self, s: usize) {
        self.start_states.push(s);
    }

    fn mark_final(&mut self, s: usize) {
        self.final_states.push(s);
    }

    fn build(&mut self, path: &PropertyPathExpression) -> Result<(usize, usize), RpqError> {
        match path {
            PropertyPathExpression::NamedNode(nn) => {
                let s = self.new_state();
                let e = self.new_state();
                self.add_label(s, e, nn.as_str().to_owned());
                Ok((s, e))
            }

            PropertyPathExpression::Sequence(lhs, rhs) => {
                let (ls, le) = self.build(lhs)?;
                let (rs, re) = self.build(rhs)?;
                self.add_epsilon(le, rs);
                Ok((ls, re))
            }

            PropertyPathExpression::Alternative(lhs, rhs) => {
                let s = self.new_state();
                let e = self.new_state();
                let (ls, le) = self.build(lhs)?;
                let (rs, re) = self.build(rhs)?;
                self.add_epsilon(s, ls);
                self.add_epsilon(s, rs);
                self.add_epsilon(le, e);
                self.add_epsilon(re, e);
                Ok((s, e))
            }

            PropertyPathExpression::ZeroOrMore(inner) => {
                let s = self.new_state();
                let e = self.new_state();
                let (is, ie) = self.build(inner)?;
                self.add_epsilon(s, is);
                self.add_epsilon(ie, is);
                self.add_epsilon(ie, e);
                self.add_epsilon(s, e);
                Ok((s, e))
            }

            PropertyPathExpression::OneOrMore(inner) => {
                let s = self.new_state();
                let e = self.new_state();
                let (is, ie) = self.build(inner)?;
                self.add_epsilon(s, is);
                self.add_epsilon(ie, is);
                self.add_epsilon(ie, e);
                Ok((s, e))
            }

            PropertyPathExpression::ZeroOrOne(inner) => {
                let s = self.new_state();
                let e = self.new_state();
                let (is, ie) = self.build(inner)?;
                self.add_epsilon(s, is);
                self.add_epsilon(ie, e);
                self.add_epsilon(s, e);
                Ok((s, e))
            }

            PropertyPathExpression::Reverse(_) => Err(RpqError::UnsupportedPath(
                "Reverse paths are not supported".into(),
            )),

            PropertyPathExpression::NegatedPropertySet(_) => Err(RpqError::UnsupportedPath(
                "NegatedPropertySet paths are not supported".into(),
            )),
        }
    }

    fn epsilon_closure(&self, states: &[usize]) -> HashSet<usize> {
        let mut closure: HashSet<usize> = states.iter().copied().collect();
        let mut queue: VecDeque<usize> = states.iter().copied().collect();
        while let Some(s) = queue.pop_front() {
            for t in &self.transitions {
                if t.from == s && t.label.is_none() && !closure.contains(&t.to) {
                    closure.insert(t.to);
                    queue.push_back(t.to);
                }
            }
        }
        closure
    }

    fn into_nfa(self) -> Nfa {
        let n = self.num_states;

        let closures: Vec<HashSet<usize>> = (0..n).map(|s| self.epsilon_closure(&[s])).collect();

        let mut label_map: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for from in 0..n {
            for t in &self.transitions {
                if t.from == from {
                    if let Some(label) = &t.label {
                        for &cf in &closures[from] {
                            for &ct in &closures[t.to] {
                                label_map.entry(label.clone()).or_default().push((cf, ct));
                            }
                        }
                    }
                }
            }
        }

        let start_closure = self.epsilon_closure(&self.start_states);
        let start_states: Vec<GrB_Index> =
            start_closure.into_iter().map(|s| s as GrB_Index).collect();

        let final_set: HashSet<usize> = self.final_states.iter().copied().collect();
        let final_states: Vec<GrB_Index> = (0..n)
            .filter(|s| closures[*s].iter().any(|c| final_set.contains(c)))
            .map(|s| s as GrB_Index)
            .collect();

        let transitions: Vec<NfaLabelTransitions> = label_map
            .into_iter()
            .map(|(label, pairs)| {
                let mut rows = Vec::with_capacity(pairs.len());
                let mut cols = Vec::with_capacity(pairs.len());
                for (r, c) in pairs {
                    rows.push(r as GrB_Index);
                    cols.push(c as GrB_Index);
                }
                NfaLabelTransitions { label, rows, cols }
            })
            .collect();

        Nfa {
            num_states: n,
            start_states,
            final_states,
            transitions,
        }
    }
}

/// Evaluates RPQs using `LAGraph_RegularPathQuery`.
pub struct NfaRpqEvaluator;

impl RpqEvaluator for NfaRpqEvaluator {
    fn evaluate<G: GraphDecomposition>(
        &self,
        subject: &TermPattern,
        path: &PropertyPathExpression,
        object: &TermPattern,
        graph: &G,
    ) -> Result<RpqResult, RpqError> {
        let nfa = Nfa::from_property_path(path)?;
        let nfa_matrices = nfa.build_lagraph_matrices()?;

        let src_id = resolve_vertex(subject, graph, true)?;
        let _dst_id = resolve_vertex(object, graph, false)?;

        let n = graph.num_nodes();

        let source_vertices: Vec<GrB_Index> = match src_id {
            Some(id) => vec![id as GrB_Index],
            None => (0..n as GrB_Index).collect(),
        };

        let mut nfa_graph_ptrs: Vec<LAGraph_Graph> =
            nfa_matrices.iter().map(|(_, lg)| lg.inner).collect();

        let mut data_graph_ptrs: Vec<LAGraph_Graph> = Vec::with_capacity(nfa_matrices.len());
        for (label, _) in &nfa_matrices {
            let lg = graph
                .get_graph(label)
                .map_err(|_| RpqError::LabelNotFound(label.clone()))?;
            data_graph_ptrs.push(lg.inner);
        }

        let mut reachable: GrB_Vector = std::ptr::null_mut();

        la_ok!(LAGraph_RegularPathQuery(
            &mut reachable,
            nfa_graph_ptrs.as_mut_ptr(),
            nfa_matrices.len(),
            nfa.start_states.as_ptr(),
            nfa.start_states.len(),
            nfa.final_states.as_ptr(),
            nfa.final_states.len(),
            data_graph_ptrs.as_mut_ptr(),
            source_vertices.as_ptr(),
            source_vertices.len(),
        ))
        .map_err(|e: GraphError| RpqError::GraphBlas(format!("{e}")))?;

        let result_vec = GraphblasVector { inner: reachable };

        Ok(RpqResult {
            reachable: result_vec,
        })
    }
}

fn resolve_vertex<G: GraphDecomposition>(
    term: &TermPattern,
    graph: &G,
    is_subject: bool,
) -> Result<Option<usize>, RpqError> {
    match term {
        TermPattern::Variable(_) => Ok(None),
        TermPattern::NamedNode(nn) => {
            let iri = nn.as_str();
            graph
                .get_node_id(iri)
                .map(Some)
                .ok_or_else(|| RpqError::VertexNotFound(iri.to_owned()))
        }
        other => {
            let msg = format!("{other}");
            if is_subject {
                Err(RpqError::VertexNotFound(format!(
                    "unsupported subject term: {msg}"
                )))
            } else {
                Err(RpqError::VertexNotFound(format!(
                    "unsupported object term: {msg}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spargebra::algebra::PropertyPathExpression;
    use spargebra::term::NamedNode;

    fn named(iri: &str) -> PropertyPathExpression {
        PropertyPathExpression::NamedNode(NamedNode::new_unchecked(iri))
    }

    #[test]
    fn test_single_label() {
        let nfa = Nfa::from_property_path(&named("knows")).unwrap();
        assert_eq!(nfa.num_states, 2);
        assert_eq!(nfa.start_states.len(), 1);
        assert_eq!(nfa.final_states.len(), 1);
        assert_eq!(nfa.transitions.len(), 1);
        assert_eq!(nfa.transitions[0].label, "knows");
        assert_eq!(nfa.transitions[0].rows.len(), 1);
    }

    #[test]
    fn test_sequence() {
        let path = PropertyPathExpression::Sequence(Box::new(named("a")), Box::new(named("b")));
        let nfa = Nfa::from_property_path(&path).unwrap();
        let labels: Vec<&str> = nfa.transitions.iter().map(|t| t.label.as_str()).collect();
        assert!(labels.contains(&"a"));
        assert!(labels.contains(&"b"));
    }

    #[test]
    fn test_alternative() {
        let path = PropertyPathExpression::Alternative(Box::new(named("a")), Box::new(named("b")));
        let nfa = Nfa::from_property_path(&path).unwrap();
        let labels: Vec<&str> = nfa.transitions.iter().map(|t| t.label.as_str()).collect();
        assert!(labels.contains(&"a"));
        assert!(labels.contains(&"b"));
    }

    #[test]
    fn test_zero_or_more() {
        let path = PropertyPathExpression::ZeroOrMore(Box::new(named("knows")));
        let nfa = Nfa::from_property_path(&path).unwrap();
        // Start state should also be a final state (zero matches).
        let start_set: HashSet<GrB_Index> = nfa.start_states.iter().copied().collect();
        let final_set: HashSet<GrB_Index> = nfa.final_states.iter().copied().collect();
        assert!(!start_set.is_disjoint(&final_set));
    }

    #[test]
    fn test_reverse_unsupported() {
        let path = PropertyPathExpression::Reverse(Box::new(named("knows")));
        assert!(matches!(
            Nfa::from_property_path(&path),
            Err(RpqError::UnsupportedPath(_))
        ));
    }
}
