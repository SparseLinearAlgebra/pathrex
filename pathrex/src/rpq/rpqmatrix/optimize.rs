use egg::{Extractor, RecExpr, Runner};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex, Weak},
};

use super::cost::{
    HybridCostFn, JoinCostFn, MetaAcCostFn, MncCostFn, PangHybridCostFn, SamplingCostFn,
};
use super::plan::{RpqPlan, make_rules};
use super::sampling::SamplingConfig;
use super::stats::LabelCountVectors;
use crate::{graph::{GraphDecomposition, LagraphGraph}, rpq::RpqError};

static RULES: LazyLock<Vec<egg::Rewrite<RpqPlan, ()>>> = LazyLock::new(make_rules);

#[derive(Clone, Copy, Debug)]
pub enum OptimizationStrategy {
    NoOpt,
    Join,
    MetaAc,
    Mnc,
    Hybrid,
    PangHybrid,
    Sampling,
    RandomOpt, // TODO: should be same as random from la-n-egg-rpq: https://github.com/SparseLinearAlgebra/la-n-egg-rpq/blob/main/src/main.rs#L75
    Simple,    // TODO
    Wander,    // TODO
}

fn runner(expr: &RecExpr<RpqPlan>) -> Runner<RpqPlan, ()> {
    Runner::default()
        .with_explanations_disabled()
        .with_expr(expr)
        .run(&*RULES)
}

fn extract_with<C: egg::CostFunction<RpqPlan>>(expr: &RecExpr<RpqPlan>, cost: C) -> RecExpr<RpqPlan> {
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

struct CachedLabel {
    graph: Weak<LagraphGraph>,
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

pub(super) fn optimize_expr_mnc<G: GraphDecomposition>(expr: RecExpr<RpqPlan>, graph: &G, cache: &Mutex<OptimizerCache>) -> Result<RecExpr<RpqPlan>, RpqError> {
    let labels = cached_label_data(&labels(&expr), graph, cache)?;
    let vertices = vertices(&expr, graph)?;
    let vertex_counts = vertex_counts(&vertices, graph.num_nodes(), cache)?;
    Ok(extract_with(
        &expr,
        MncCostFn::new(graph.num_nodes() as f64, labels, vertex_counts),
    ))
}

fn cached_label_data<G: GraphDecomposition>(names: &HashSet<String>, graph: &G, cache: &Mutex<OptimizerCache>) -> Result<HashMap<String, LabelCountVectors>, RpqError> {
    let mut cache = cache.lock().expect("RPQ optimizer cache poisoned");
    let mut counts = HashMap::with_capacity(names.len());
    for name in names {
        let label_graph = graph.get_graph(name)?;
        let matrix = label_graph.matrix();
        let stale = cache
            .labels
            .get(name)
            .and_then(|entry| entry.graph.upgrade())
            .is_none_or(|cached| !Arc::ptr_eq(&cached, &label_graph));
        if stale {
            let precomputed = graph
                .get_metadata()
                .and_then(|metadata| metadata.matrix(name))
                .and_then(|metadata| metadata.counts.as_ref());
            let mut vectors = match precomputed {
                Some(vectors) => vectors.clone(),
                None => LabelCountVectors::from_matrix(matrix).ok_or_else(|| {
                    RpqError::UnsupportedPath("unable to build count vectors".into())
                })?,
            };
            vectors.ensure_extended(matrix).ok_or_else(|| {
                RpqError::UnsupportedPath("unable to build extended count vectors".into())
            })?;
            cache.labels.insert(
                name.clone(),
                CachedLabel {
                    graph: Arc::downgrade(&label_graph),
                    counts: vectors,
                },
            );
        }
        let entry = cache.labels.get(name).expect("cached label was inserted");
        counts.insert(name.clone(), entry.counts.clone());
    }
    Ok(counts)
}

fn vertex_counts(vertices: &HashMap<String, usize>, n: usize, cache: &Mutex<OptimizerCache>) -> Result<HashMap<String, LabelCountVectors>, RpqError> {
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

pub(super) fn optimize_expr_pang_hybrid(expr: RecExpr<RpqPlan>, n: usize) -> RecExpr<RpqPlan> {
    extract_with(&expr, PangHybridCostFn::new(n as f64))
}

pub(super) fn optimize_expr_sampling<G: GraphDecomposition>(expr: RecExpr<RpqPlan>, graph: &G) -> Result<RecExpr<RpqPlan>, RpqError> {
    let names = labels(&expr);
    let matrices = names.iter().map(|name| Ok((name.clone(), graph.get_graph(name)?))).collect::<Result<Vec<_>, RpqError>>()?;
    let vertices = vertices(&expr, graph)?;
    let parse = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let config = SamplingConfig {
        percent: parse("RPQ_SAMPLE_PERCENT", 1).clamp(1, 100),
        seed: std::env::var("RPQ_SAMPLE_SEED")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        max_star_iterations: parse("RPQ_SAMPLE_MAX_STAR_ITERS", 64),
    };
    Ok(extract_with(&expr, SamplingCostFn::new(graph.num_nodes() as f64, matrices, vertices, config)))
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

fn vertices<G: GraphDecomposition>(expr: &RecExpr<RpqPlan>, graph: &G) -> Result<HashMap<String, usize>, RpqError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::MatrixMarket;
    use crate::graph::{Graph, InMemory, MatrixStatsMode};

    #[test]
    fn cached_labels_reuse_statistics_built_during_graph_load() {
        let graph = Graph::<InMemory>::try_from(
            MatrixMarket::from_dir("tests/testdata/mm_small")
                .with_matrix_stats(MatrixStatsMode::Extended),
        )
        .unwrap();
        let names = HashSet::from(["knows".to_string()]);
        let cache = Mutex::new(OptimizerCache::default());
        let precomputed = graph
            .get_metadata()
            .unwrap()
            .matrix("knows")
            .unwrap()
            .counts
            .as_ref()
            .unwrap();
        let cached = cached_label_data(&names, &graph, &cache).unwrap();

        assert_eq!(
            cached["knows"].row_counts.cache_key(),
            precomputed.row_counts.cache_key()
        );
        assert_eq!(
            cached["knows"].col_counts.cache_key(),
            precomputed.col_counts.cache_key()
        );
        assert_eq!(
            cached["knows"].row_extended.as_ref().unwrap().cache_key(),
            precomputed.row_extended.as_ref().unwrap().cache_key()
        );
        assert_eq!(
            cached["knows"].col_extended.as_ref().unwrap().cache_key(),
            precomputed.col_extended.as_ref().unwrap().cache_key()
        );
    }

    #[test]
    fn cached_basic_stats_are_upgraded_for_mnc() {
        let graph = Graph::<InMemory>::try_from(
            MatrixMarket::from_dir("tests/testdata/mm_small")
                .with_matrix_stats(MatrixStatsMode::Basic),
        )
        .unwrap();
        let labels = HashSet::from(["knows".to_string()]);
        let cache = Mutex::new(OptimizerCache::default());

        let extended = cached_label_data(&labels, &graph, &cache).unwrap();
        assert!(extended["knows"].row_extended.is_some());
        assert!(extended["knows"].col_extended.is_some());
    }

    #[test]
    fn cached_labels_are_rebuilt_for_another_graph() {
        let names = HashSet::from(["p".to_string()]);
        let cache = Mutex::new(OptimizerCache::default());
        let first = crate::utils::build_graph(&[("a", "b", "p")]);
        let old = cached_label_data(&names, &first, &cache).unwrap();
        let owner = cache.lock().unwrap().labels["p"].graph.clone();
        drop(first);
        assert!(owner.upgrade().is_none());

        let second = crate::utils::build_graph(&[("a", "b", "p"), ("b", "c", "p")]);
        let new = cached_label_data(&names, &second, &cache).unwrap();
        assert_ne!(old["p"].row_counts.cache_key(), new["p"].row_counts.cache_key());
        assert_eq!(new["p"].row_counts.sum(), 2.0);
    }
}
