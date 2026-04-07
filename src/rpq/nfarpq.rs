//! NFA-based RPQ evaluation using `LAGraph_RegularPathQuery`.

use crate::graph::{GraphDecomposition, GraphblasVector, LagraphGraph, ensure_grb_init};
use crate::grb_ok;
use crate::la_ok;
use crate::lagraph_sys::*;
use crate::lagraph_sys::{GrB_BOOL, GrB_LOR, GrB_Matrix_build_BOOL, GrB_Matrix_new, LAGraph_Kind};
use crate::rpq::{Endpoint, PathExpr, RpqError, RpqEvaluator, RpqQuery};
use rustfst::algorithms::closure::{ClosureType, closure};
use rustfst::algorithms::concat::concat;
use rustfst::algorithms::rm_epsilon::rm_epsilon;
use rustfst::algorithms::union::union;
use rustfst::prelude::*;
use rustfst::semirings::TropicalWeight;
use rustfst::utils::{acceptor, epsilon_machine};
use std::collections::HashMap;

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

struct SymbolTable {
    label_to_id: HashMap<String, Label>,
    id_to_label: HashMap<Label, String>,
    next_id: Label,
}

impl SymbolTable {
    fn new() -> Self {
        Self {
            label_to_id: HashMap::new(),
            id_to_label: HashMap::new(),
            next_id: 1,
        }
    }

    fn get_or_insert(&mut self, label: &str) -> Label {
        if let Some(&id) = self.label_to_id.get(label) {
            id
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.label_to_id.insert(label.to_string(), id);
            self.id_to_label.insert(id, label.to_string());
            id
        }
    }

    fn get_label(&self, id: Label) -> Option<&str> {
        self.id_to_label.get(&id).map(|s| s.as_str())
    }
}

fn map_fst_error<E: std::fmt::Display>(operation: &'static str, e: E) -> RpqError {
    RpqError::UnsupportedPath(format!("{} failed: {}", operation, e))
}

impl Nfa {
    /// Build an NFA from a path expression.
    pub fn from_path_expr(path: &PathExpr) -> Result<Self, RpqError> {
        let mut symbols = SymbolTable::new();

        let mut fst = build_fst(path, &mut symbols)?;

        rm_epsilon(&mut fst).map_err(|e| map_fst_error("rm_epsilon", e))?;

        extract_nfa(&fst, &symbols)
    }

    /// Convert NFA transitions to LAGraph matrices for RPQ evaluation.
    pub fn build_lagraph_matrices(&self) -> Result<Vec<(String, LagraphGraph)>, RpqError> {
        ensure_grb_init()?;
        let n = self.num_states as GrB_Index;
        let mut result = Vec::with_capacity(self.transitions.len());

        for trans in &self.transitions {
            let mut mat: GrB_Matrix = std::ptr::null_mut();
            grb_ok!(GrB_Matrix_new(&mut mat, GrB_BOOL, n, n))?;

            if !trans.rows.is_empty() {
                let vals: Vec<bool> = vec![true; trans.rows.len()];
                grb_ok!(GrB_Matrix_build_BOOL(
                    mat,
                    trans.rows.as_ptr(),
                    trans.cols.as_ptr(),
                    vals.as_ptr(),
                    trans.rows.len() as u64,
                    GrB_LOR,
                ))?;
            }

            let lg = LagraphGraph::new(mat, LAGraph_Kind::LAGraph_ADJACENCY_DIRECTED)?;
            result.push((trans.label.clone(), lg));
        }

        Ok(result)
    }
}

/// Build a VectorFst from a PathExpr using Thompson-like construction.
fn build_fst(
    path: &PathExpr,
    symbols: &mut SymbolTable,
) -> Result<VectorFst<TropicalWeight>, RpqError> {
    match path {
        PathExpr::Label(label) => {
            let label_id = symbols.get_or_insert(label);
            Ok(acceptor(&[label_id], TropicalWeight::one()))
        }

        PathExpr::Sequence(lhs, rhs) => {
            let mut fst_l = build_fst(lhs, symbols)?;
            let fst_r = build_fst(rhs, symbols)?;
            concat(&mut fst_l, &fst_r).map_err(|e| map_fst_error("concat", e))?;
            Ok(fst_l)
        }

        PathExpr::Alternative(lhs, rhs) => {
            let mut fst_l = build_fst(lhs, symbols)?;
            let fst_r = build_fst(rhs, symbols)?;
            union(&mut fst_l, &fst_r).map_err(|e| map_fst_error("union", e))?;
            Ok(fst_l)
        }

        PathExpr::ZeroOrMore(inner) => {
            let mut fst = build_fst(inner, symbols)?;
            closure(&mut fst, ClosureType::ClosureStar);
            Ok(fst)
        }

        PathExpr::OneOrMore(inner) => {
            let mut fst = build_fst(inner, symbols)?;
            closure(&mut fst, ClosureType::ClosurePlus);
            Ok(fst)
        }

        PathExpr::ZeroOrOne(inner) => {
            let mut fst_inner = build_fst(inner, symbols)?;
            let fst_eps = epsilon_machine::<TropicalWeight, VectorFst<TropicalWeight>>()
                .map_err(|e| map_fst_error("epsilon_machine", e))?;

            union(&mut fst_inner, &fst_eps).map_err(|e| map_fst_error("union", e))?;
            Ok(fst_inner)
        }
    }
}

fn extract_nfa(fst: &VectorFst<TropicalWeight>, symbols: &SymbolTable) -> Result<Nfa, RpqError> {
    let num_states = fst.num_states();

    let mut label_transitions: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

    for state in fst.states_iter() {
        for tr in fst.get_trs(state).unwrap().trs() {
            if tr.ilabel == EPS_LABEL {
                continue;
            }

            if let Some(label) = symbols.get_label(tr.ilabel) {
                label_transitions
                    .entry(label.to_string())
                    .or_default()
                    .push((state as usize, tr.nextstate as usize));
            }
        }
    }

    let start_states: Vec<GrB_Index> = fst
        .start()
        .map(|s| vec![s as GrB_Index])
        .unwrap_or_default();

    let final_states: Vec<GrB_Index> = fst
        .states_iter()
        .filter(|&s| fst.is_final(s).unwrap_or(false))
        .map(|s| s as GrB_Index)
        .collect();

    let transitions: Vec<NfaLabelTransitions> = label_transitions
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

    Ok(Nfa {
        num_states,
        start_states,
        final_states,
        transitions,
    })
}

#[derive(Debug)]
pub struct NfaRpqResult {
    pub reachable: GraphblasVector,
}

/// Evaluates RPQs using `LAGraph_RegularPathQuery`.
pub struct NfaRpqEvaluator;

impl RpqEvaluator for NfaRpqEvaluator {
    type Result = NfaRpqResult;

    fn evaluate<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<NfaRpqResult, RpqError> {
        let nfa = Nfa::from_path_expr(&query.path)?;
        let nfa_matrices = nfa.build_lagraph_matrices()?;

        let src_id = resolve_endpoint(&query.subject, graph)?;
        let _dst_id = resolve_endpoint(&query.object, graph)?;

        let n = graph.num_nodes();

        let source_vertices: Vec<GrB_Index> = match src_id {
            Some(id) => vec![id as GrB_Index],
            None => (0..n as GrB_Index).collect(),
        };

        let mut nfa_graph_ptrs: Vec<LAGraph_Graph> =
            nfa_matrices.iter().map(|(_, lg)| lg.inner).collect();

        let mut data_graph_ptrs: Vec<LAGraph_Graph> = Vec::with_capacity(nfa_matrices.len());
        for (label, _) in &nfa_matrices {
            let lg = graph.get_graph(label)?;
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
        ))?;

        let result_vec = GraphblasVector { inner: reachable };

        Ok(NfaRpqResult {
            reachable: result_vec,
        })
    }
}

fn resolve_endpoint<G: GraphDecomposition>(
    term: &Endpoint,
    graph: &G,
) -> Result<Option<usize>, RpqError> {
    match term {
        Endpoint::Variable(_) => Ok(None),
        Endpoint::Named(id) => graph
            .get_node_id(id)
            .map(Some)
            .ok_or_else(|| RpqError::VertexNotFound(id.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(s: &str) -> PathExpr {
        PathExpr::Label(s.to_owned())
    }

    #[test]
    fn test_single_label() {
        let nfa = Nfa::from_path_expr(&label("knows")).unwrap();
        assert!(nfa.num_states >= 2, "NFA should have at least 2 states");
        assert!(!nfa.start_states.is_empty(), "should have start states");
        assert!(!nfa.final_states.is_empty(), "should have final states");
        assert_eq!(nfa.transitions.len(), 1);
        assert_eq!(nfa.transitions[0].label, "knows");
        assert!(!nfa.transitions[0].rows.is_empty());
    }

    #[test]
    fn test_sequence() {
        let path = PathExpr::Sequence(Box::new(label("a")), Box::new(label("b")));
        let nfa = Nfa::from_path_expr(&path).unwrap();
        let labels: Vec<&str> = nfa.transitions.iter().map(|t| t.label.as_str()).collect();
        assert!(labels.contains(&"a"));
        assert!(labels.contains(&"b"));
    }

    #[test]
    fn test_alternative() {
        let path = PathExpr::Alternative(Box::new(label("a")), Box::new(label("b")));
        let nfa = Nfa::from_path_expr(&path).unwrap();
        let labels: Vec<&str> = nfa.transitions.iter().map(|t| t.label.as_str()).collect();
        assert!(labels.contains(&"a"));
        assert!(labels.contains(&"b"));
    }

    #[test]
    fn test_zero_or_more() {
        let path = PathExpr::ZeroOrMore(Box::new(label("knows")));
        let nfa = Nfa::from_path_expr(&path).unwrap();
        // For zero-or-more, start state should be final (accepts empty string)
        assert!(!nfa.start_states.is_empty());
        assert!(!nfa.final_states.is_empty());
        // After rm_epsilon, the start state should be in final states
        let start_set: std::collections::HashSet<GrB_Index> =
            nfa.start_states.iter().copied().collect();
        let final_set: std::collections::HashSet<GrB_Index> =
            nfa.final_states.iter().copied().collect();
        assert!(
            !start_set.is_disjoint(&final_set),
            "start and final states should overlap for zero-or-more, start={:?}, final={:?}",
            start_set,
            final_set
        );
    }
}
