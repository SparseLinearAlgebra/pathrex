use egg::{Extractor, RecExpr, Runner};
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use super::cost::{HybridCostFn, JoinCostFn, MetaAcCostFn, MncCostFn};
use super::plan::{RpqPlan, make_rules};
use super::stats::LabelCountVectors;
use crate::{graph::GraphDecomposition, rpq::RpqError};

static RULES: LazyLock<Vec<egg::Rewrite<RpqPlan, ()>>> = LazyLock::new(make_rules);

#[derive(Clone, Copy, Debug)]
pub enum OptimizationStrategy {
    NoOpt,
    Join,
    MetaAc,
    Mnc,
    Hybrid,
    RandomOpt, // TODO: should be same as random from la-n-egg-rpq: https://github.com/SparseLinearAlgebra/la-n-egg-rpq/blob/main/src/main.rs#L75
    Simple,    // TODO
    Wander,    // TODO
}

struct CachedLabel {
    matrix_id: usize,
    counts: LabelCountVectors,
}

struct CachedVertex {
    vertex: usize,
    n: usize,
    counts: LabelCountVectors,
}

#[derive(Default)]
pub(super) struct OptimizerCache {
    labels: HashMap<String, CachedLabel>,
    vertices: HashMap<String, CachedVertex>,
}

fn runner(expr: &RecExpr<RpqPlan>) -> Runner<RpqPlan, ()> {
    Runner::default()
        .with_explanations_disabled()
        .with_expr(expr)
        .run(&*RULES)
}

fn extract_with<C: egg::CostFunction<RpqPlan>>(
    expr: &RecExpr<RpqPlan>,
    cost: C,
) -> RecExpr<RpqPlan> {
    let runner = runner(expr);
    Extractor::new(&runner.egraph, cost)
        .find_best(runner.roots[0])
        .1
}

pub(super) fn optimize_expr_join(expr: RecExpr<RpqPlan>, graph_size: usize) -> RecExpr<RpqPlan> {
    extract_with(
        &expr,
        JoinCostFn {
            n: graph_size as f64,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        },
    )
}

pub(super) fn optimize_expr_metaac(expr: RecExpr<RpqPlan>, n: usize) -> RecExpr<RpqPlan> {
    extract_with(
        &expr,
        MetaAcCostFn {
            n: n as f64,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        },
    )
}

pub(super) fn optimize_expr_mnc<G: GraphDecomposition>(
    expr: RecExpr<RpqPlan>,
    graph: &G,
    cache: &Mutex<OptimizerCache>,
) -> Result<RecExpr<RpqPlan>, RpqError> {
    let labels = cached_label_data(&labels(&expr), graph, cache)?;
    let vertices = vertices(&expr, graph)?;
    let vertex_counts = vertex_counts(&vertices, graph.num_nodes(), cache)?;
    Ok(extract_with(
        &expr,
        MncCostFn::new(graph.num_nodes() as f64, labels, vertex_counts),
    ))
}

pub(super) fn optimize_expr_hybrid(expr: RecExpr<RpqPlan>, n: usize) -> RecExpr<RpqPlan> {
    extract_with(
        &expr,
        HybridCostFn {
            n: n as f64,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        },
    )
}

fn labels(expr: &RecExpr<RpqPlan>) -> HashSet<String> {
    expr.as_ref()
        .iter()
        .filter_map(|node| match node {
            RpqPlan::Label(meta) => Some(meta.name.clone()),
            _ => None,
        })
        .collect()
}

fn cached_label_data<G: GraphDecomposition>(
    names: &HashSet<String>,
    graph: &G,
    cache: &Mutex<OptimizerCache>,
) -> Result<HashMap<String, LabelCountVectors>, RpqError> {
    let mut cache = cache.lock().expect("RPQ optimizer cache poisoned");
    let mut counts = HashMap::with_capacity(names.len());
    for name in names {
        let matrix = graph.get_graph(name)?.matrix();
        let matrix_id = matrix as usize;
        let stale = cache
            .labels
            .get(name)
            .is_none_or(|entry| entry.matrix_id != matrix_id);
        if stale {
            let vectors = LabelCountVectors::from_matrix(matrix)
                .ok_or_else(|| RpqError::UnsupportedPath("unable to build count vectors".into()))?;
            cache.labels.insert(
                name.clone(),
                CachedLabel {
                    matrix_id,
                    counts: vectors,
                },
            );
        }
        counts.insert(
            name.clone(),
            cache
                .labels
                .get(name)
                .expect("cached label was inserted")
                .counts
                .clone(),
        );
    }
    Ok(counts)
}

fn vertices<G: GraphDecomposition>(
    expr: &RecExpr<RpqPlan>,
    graph: &G,
) -> Result<HashMap<String, usize>, RpqError> {
    expr.as_ref()
        .iter()
        .filter_map(|node| match node {
            RpqPlan::NamedVertex(name) => Some(name),
            _ => None,
        })
        .map(|name| {
            graph
                .get_node_id(name)
                .map(|id| (name.clone(), id))
                .ok_or_else(|| RpqError::VertexNotFound(name.clone()))
        })
        .collect()
}

fn vertex_counts(
    vertices: &HashMap<String, usize>,
    n: usize,
    cache: &Mutex<OptimizerCache>,
) -> Result<HashMap<String, LabelCountVectors>, RpqError> {
    let mut cache = cache.lock().expect("RPQ optimizer cache poisoned");
    vertices
        .iter()
        .map(|(name, &vertex)| {
            let stale = cache
                .vertices
                .get(name)
                .is_none_or(|entry| entry.vertex != vertex || entry.n != n);
            if stale {
                let counts = LabelCountVectors::from_vertex(vertex, n).ok_or_else(|| {
                    RpqError::UnsupportedPath("unable to build endpoint statistics".into())
                })?;
                cache
                    .vertices
                    .insert(name.clone(), CachedVertex { vertex, n, counts });
            }
            Ok((
                name.clone(),
                cache
                    .vertices
                    .get(name)
                    .expect("vertex was cached")
                    .counts
                    .clone(),
            ))
        })
        .collect()
}
