//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

use std::ptr::null_mut;
use std::{cmp::Ordering, fmt::Display, str::FromStr};

use egg::{CostFunction, Id, RecExpr, define_language, rewrite};

use crate::eval::{Evaluator, PreparedEvaluator, ResultCount};
use crate::graph::wrappers::ReduceType::ByCols;
use crate::graph::{GraphDecomposition, GraphError, GraphblasMatrix};
use crate::lagraph_sys::*;
use crate::rpq::{Endpoint, PathExpr, RpqError, RpqQuery};
use crate::{grb_ok, la_ok};

#[derive(Clone, Hash, Ord, Eq, PartialEq, PartialOrd, Debug)]
struct LabelMeta {
    pub name: String,
    pub nvals: usize,
    pub nonzero_rows: usize,
    pub nonzero_cols: usize,
}

impl FromStr for LabelMeta {
    type Err = <usize as FromStr>::Err;
    // This is needed for the builtin egg parser. Only used in tests.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(LabelMeta {
            name: "-".to_string(),
            nvals: s.parse()?,
            nonzero_rows: s.parse()?,
            nonzero_cols: s.parse()?,
        })
    }
}

impl Display for LabelMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.name, self.nvals)
    }
}

define_language! {
    pub enum RpqPlan {
        Label(LabelMeta),
        NamedVertex(String),
        "/" = Seq([Id; 2]),
        "|" = Alt([Id; 2]),
        "*" = Star([Id; 1]),
        "l*" = LStar([Id; 2]),
        "*r" = RStar([Id; 2]),
    }
}

pub fn make_rules() -> Vec<egg::Rewrite<RpqPlan, ()>> {
    vec![
        rewrite!("assoc-sec-1"; "(/ ?a (/ ?b ?c))" => "(/ (/ ?a ?b) ?c)"),
        rewrite!("assoc-sec-2"; "(/ (/ ?a ?b) ?c)" => "(/ ?a (/ ?b ?c))"),
        rewrite!("commute-alt"; "(| ?a ?b)" => "(| ?b ?a)"),
        rewrite!("assoc-alt"; "(| ?a (| ?b ?c))" => "(| (| ?a ?b) ?c)"),
        rewrite!("distribute-1"; "(/ ?a (| ?b ?c))" => "(| (/ ?a ?b) (/ ?a ?c))"),
        rewrite!("distribute-2"; "(/ (| ?a ?b) ?c)" => "(| (/ ?a ?c) (/ ?b ?c))"),
        rewrite!("distribute-3"; "(| (/ ?a ?b) (/ ?a ?c))" => "(/ ?a (| ?b ?c))"),
        rewrite!("distribute-4"; "(| (/ ?a ?c) (/ ?b ?c))" => "(/ (| ?a ?b) ?c)"),
        rewrite!("build-lstar"; "(/ (* ?a) ?b)" => "(l* ?a ?b)"),
        rewrite!("build-rstar"; "(/ ?a (* ?b))" => "(*r ?a ?b)"),
    ]
}

pub struct RandomCostFn;
impl CostFunction<RpqPlan> for RandomCostFn {
    type Cost = f64;
    fn cost<C>(&mut self, _enode: &RpqPlan, _costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        rand::random()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardCost {
    pub score: f64,
    pub nnz: f64,
    pub nnz_r: f64,
    pub nnz_c: f64,
}

impl Eq for CardCost {}

impl PartialOrd for CardCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CardCost {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.score.total_cmp(&other.score) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.nnz.total_cmp(&other.nnz) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.nnz_r.total_cmp(&other.nnz_r) {
            Ordering::Equal => {}
            ord => return ord,
        }
        self.nnz_c.total_cmp(&other.nnz_c)
    }
}

pub struct CardinalityCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
}

// TODO: check value intervals
impl CostFunction<RpqPlan> for CardinalityCostFn {
    type Cost = CardCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::NamedVertex(_name) => CardCost {
                score: 0.0,
                nnz: 1 as f64,
                nnz_r: 1 as f64,
                nnz_c: 1 as f64,
            },
            RpqPlan::Label(meta) => CardCost {
                score: 0.0,
                nnz: meta.nvals as f64,
                nnz_r: meta.nonzero_rows as f64,
                nnz_c: meta.nonzero_cols as f64,
            },

            RpqPlan::Seq([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = (ca.nnz * cb.nnz) / denom;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let overlap = (ca.nnz * cb.nnz) / (self.n * self.n);
                let op_cost = ca.nnz + cb.nnz - overlap;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = (ca.nnz + cb.nnz - overlap).min(self.n * self.n).max(0.0);

                let nnz_r_est = (ca.nnz_r + cb.nnz_r - (ca.nnz_r * cb.nnz_r) / self.n)
                    .min(self.n)
                    .max(0.0);

                let nnz_c_est = (ca.nnz_c + cb.nnz_c - (ca.nnz_c * cb.nnz_c) / self.n)
                    .min(self.n)
                    .max(0.0);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: nnz_r_est,
                    nnz_c: nnz_c_est,
                }
            }

            RpqPlan::Star([a]) => {
                let ca = costs(*a);

                let penalty = self.star_penalty * ca.nnz.max(1.0);
                let score = ca.score + penalty;

                CardCost {
                    score,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                }
            }

            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;
                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;

                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }
        }
    }
}

fn label_meta<G: GraphDecomposition>(label: &str, graph: &G) -> Result<LabelMeta, RpqError> {
    if let Some(metadata) = graph.get_metadata().and_then(|m| m.matrix(label)) {
        return Ok(LabelMeta {
            name: label.to_owned(),
            nvals: metadata.nvals,
            nonzero_rows: metadata.nonzero_rows,
            nonzero_cols: metadata.nonzero_cols,
        });
    }

    // TODO: maybe create optimized (for mm format) and nonoptimized (for other formats) plans
    let lg = graph.get_graph(label)?;
    let nvals = lg.nvals()? as usize;
    Ok(LabelMeta {
        name: label.to_owned(),
        nvals,
        nonzero_rows: nvals,
        nonzero_cols: nvals,
    })
}

fn to_expr_aux<G: GraphDecomposition>(
    path: &PathExpr,
    expr: &mut RecExpr<RpqPlan>,
    graph: &G,
) -> Result<Id, RpqError> {
    match path {
        PathExpr::Label(label) => Ok(expr.add(RpqPlan::Label(label_meta(label, graph)?))),

        PathExpr::Sequence(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr, graph)?;
            let r = to_expr_aux(rhs, expr, graph)?;
            Ok(expr.add(RpqPlan::Seq([l, r])))
        }

        PathExpr::Alternative(lhs, rhs) => {
            let l = to_expr_aux(lhs, expr, graph)?;
            let r = to_expr_aux(rhs, expr, graph)?;
            Ok(expr.add(RpqPlan::Alt([l, r])))
        }

        PathExpr::ZeroOrMore(inner) => {
            let i = to_expr_aux(inner, expr, graph)?;
            Ok(expr.add(RpqPlan::Star([i])))
        }

        PathExpr::OneOrMore(inner) => {
            let e = to_expr_aux(inner, expr, graph)?;
            let s = expr.add(RpqPlan::Star([e]));
            Ok(expr.add(RpqPlan::Seq([e, s])))
        }

        PathExpr::ZeroOrOne(_) => Err(RpqError::UnsupportedPath(
            "ZeroOrOne (?) is not supported by RPQMatrix".into(),
        )),
    }
}

/// Compile a [`RpqQuery`]  into
/// [`RecExpr<RpqPlan>`].
pub fn query_to_expr<G: GraphDecomposition>(
    query: &RpqQuery,
    graph: &G,
) -> Result<RecExpr<RpqPlan>, RpqError> {
    let mut expr = RecExpr::default();
    let path_root = to_expr_aux(&query.path, &mut expr, graph)?;

    let _root = match (&query.subject, &query.object) {
        (Endpoint::Variable(_), Endpoint::Variable(_)) => path_root,
        (Endpoint::Named(name), Endpoint::Variable(_)) => {
            let diag = expr.add(RpqPlan::NamedVertex(name.clone()));
            expr.add(RpqPlan::Seq([diag, path_root]))
        }
        (Endpoint::Variable(_), Endpoint::Named(name)) => {
            let diag = expr.add(RpqPlan::NamedVertex(name.clone()));
            expr.add(RpqPlan::Seq([path_root, diag]))
        }
        (Endpoint::Named(sub), Endpoint::Named(obj)) => {
            let diag_sub = expr.add(RpqPlan::NamedVertex(sub.clone()));
            let seq1 = expr.add(RpqPlan::Seq([diag_sub, path_root]));
            let diag_obj = expr.add(RpqPlan::NamedVertex(obj.clone()));
            expr.add(RpqPlan::Seq([seq1, diag_obj]))
        }
    };

    Ok(expr)
}

/// Convert a [`RecExpr<RpqPlan>`] into the flat [`RPQMatrixPlan`] array that
/// `LAGraph_RPQMatrix` expects.
///
/// Returns the plan array and a list of owned diagonal matrices that must be
/// freed after evaluation.
pub fn materialize<G: GraphDecomposition>(
    expr: &RecExpr<RpqPlan>,
    graph: &G,
) -> Result<(Vec<RPQMatrixPlan>, Vec<GrB_Matrix>), RpqError> {
    let null_plan = RPQMatrixPlan {
        op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
        lhs: null_mut(),
        rhs: null_mut(),
        mat: null_mut(),
        res_mat: null_mut(),
    };
    let mut plans = vec![null_plan; expr.len()];
    let mut owned_matrices: Vec<GrB_Matrix> = Vec::new();
    let n = graph.num_nodes() as GrB_Index;

    for (id, node) in expr.as_ref().iter().enumerate() {
        plans[id] = match node {
            RpqPlan::Label(label) => {
                let lg = graph.get_graph(&label.name)?;
                let mat = unsafe { (*lg.inner).A };
                RPQMatrixPlan {
                    op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
                    lhs: null_mut(),
                    rhs: null_mut(),
                    mat,
                    res_mat: null_mut(),
                }
            }

            RpqPlan::NamedVertex(name) => {
                let vertex_id = graph
                    .get_node_id(name)
                    .ok_or_else(|| RpqError::VertexNotFound(name.clone()))?
                    as GrB_Index;
                let mut mat: GrB_Matrix = null_mut();
                unsafe {
                    crate::graph::ensure_grb_init()?;
                    grb_ok!(LAGraph_RPQMatrix_label(&mut mat, vertex_id, n, n,))?
                };
                if mat.is_null() {
                    return Err(RpqError::Graph(crate::graph::GraphError::GraphBlas(
                        GrB_Info::GrB_INVALID_VALUE,
                    )));
                }
                owned_matrices.push(mat);
                RPQMatrixPlan {
                    op: RPQMatrixOp::RPQ_MATRIX_OP_LABEL,
                    lhs: null_mut(),
                    rhs: null_mut(),
                    mat,
                    res_mat: null_mut(),
                }
            }

            RpqPlan::Seq([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_CONCAT,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::Alt([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_LOR,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::Star([i]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE,
                lhs: null_mut(),
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*i)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::LStar([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE_L,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },

            RpqPlan::RStar([l, r]) => RPQMatrixPlan {
                op: RPQMatrixOp::RPQ_MATRIX_OP_KLEENE_R,
                lhs: unsafe { plans.as_mut_ptr().add(usize::from(*l)) },
                rhs: unsafe { plans.as_mut_ptr().add(usize::from(*r)) },
                mat: null_mut(),
                res_mat: null_mut(),
            },
        };
    }

    Ok((plans, owned_matrices))
}

/// Output of [`RpqMatrixEvaluator`]: full path relation matrix and its nnz.
#[derive(Debug)]
pub struct RpqMatrixResult {
    pub nnz: u64,
    pub matrix: GraphblasMatrix,
}

impl RpqMatrixResult {
    /// Count distinct reachable target vertices by reducing the path relation
    /// matrix to its non-empty columns.
    pub fn reachable_target_count(&self) -> Result<u64, crate::graph::GraphError> {
        let mut count: GrB_Index = 0;
        unsafe {
            grb_ok!(LAGraph_RPQMatrix_reduce(
                &mut count,
                self.matrix.inner,
                ByCols as u8,
            ))?
        };
        Ok(count as u64)
    }
}

impl ResultCount for RpqMatrixResult {
    fn result_count(&self) -> Result<usize, GraphError> {
        Ok(self.reachable_target_count()? as usize)
    }
}

pub struct PreparedRpqMatrix {
    plans: Vec<RPQMatrixPlan>,
    owned_matrices: Vec<GrB_Matrix>,
}

impl PreparedEvaluator for PreparedRpqMatrix {
    type Result = RpqMatrixResult;
    type Error = RpqError;

    fn execute(&mut self) -> Result<RpqMatrixResult, RpqError> {
        let root_ptr = unsafe { self.plans.as_mut_ptr().add(self.plans.len() - 1) };

        let mut nnz: GrB_Index = 0;
        unsafe { la_ok!(LAGraph_RPQMatrix(&mut nnz, root_ptr))? };

        let mut matrix_inner: GrB_Matrix = null_mut();
        unsafe { grb_ok!(GrB_Matrix_dup(&mut matrix_inner, (*root_ptr).res_mat))? };
        let matrix = GraphblasMatrix {
            inner: matrix_inner,
        };

        unsafe { grb_ok!(LAGraph_DestroyRpqMatrixPlan(root_ptr))? };

        Ok(RpqMatrixResult {
            nnz: nnz as u64,
            matrix,
        })
    }
}

impl Drop for PreparedRpqMatrix {
    fn drop(&mut self) {
        for mat in &mut self.owned_matrices {
            unsafe {
                LAGraph_RPQMatrix_Free(mat);
            }
        }
    }
}

/// RPQ evaluator backed by `LAGraph_RPQMatrix`.
#[derive(Clone, Copy)]
pub struct RpqMatrixEvaluator;

impl Evaluator for RpqMatrixEvaluator {
    type Query = RpqQuery;
    type Result = RpqMatrixResult;
    type Error = RpqError;
    type Prepared = PreparedRpqMatrix;

    fn prepare<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<PreparedRpqMatrix, RpqError> {
        let expr = query_to_expr(query, graph)?;
        let (plans, owned_matrices) = materialize(&expr, graph)?;

        Ok(PreparedRpqMatrix {
            plans,
            owned_matrices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpq::{Endpoint, PathExpr, RpqQuery};
    use crate::utils::build_graph;

    #[test]
    fn evaluate_single_edge_nnz() {
        let graph = build_graph(&[("A", "B", "p")]);
        let q = RpqQuery {
            subject: Endpoint::Variable("x".into()),
            path: PathExpr::Label("p".into()),
            object: Endpoint::Variable("y".into()),
        };
        let result = RpqMatrixEvaluator.evaluate(&q, &graph).expect("evaluate");
        assert_eq!(result.nnz, 1);
    }

    #[test]
    fn evaluate_named_subject_no_match_nnz() {
        // Graph: A --p--> B
        // Query: <C> p ?y  -> C has no outgoing p edges, nnz=0
        let graph = build_graph(&[("A", "B", "p"), ("C", "D", "q")]);
        let q = RpqQuery {
            subject: Endpoint::Named("C".into()),
            path: PathExpr::Label("p".into()),
            object: Endpoint::Variable("y".into()),
        };
        let result = RpqMatrixEvaluator.evaluate(&q, &graph).expect("evaluate");
        assert_eq!(result.nnz, 0, "C has no outgoing p edges");
    }
}
