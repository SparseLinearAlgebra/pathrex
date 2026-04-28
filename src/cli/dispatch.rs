//! Typed dispatch from CLI algorithm choices to concrete evaluators.

use crate::cli::args::{Algo, BenchArgs, QueryArgs};
use crate::cli::bench::error::BenchError;
use crate::cli::bench::runner::{build_criterion, run_bench_for_evaluator};
use crate::cli::checkpoint::Checkpointer;
use crate::cli::loader::LoadedQuery;
use crate::cli::output::QueryResult;
use crate::cli::query::run_query_for_evaluator;
use crate::graph::InMemoryGraph;
use crate::rpq::nfarpq::NfaRpqEvaluator;
use crate::rpq::rpqmatrix::RpqMatrixEvaluator;

fn merge_results(all: &mut Vec<QueryResult>, per_algo: Vec<QueryResult>) {
    for result in per_algo {
        if let Some(existing) = all
            .iter_mut()
            .find(|existing| existing.query_index == result.query_index)
        {
            existing.algorithms.extend(result.algorithms);
        } else {
            all.push(result);
        }
    }
}

pub fn dispatch_query(
    args: &QueryArgs,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
) -> Vec<QueryResult> {
    let mut all = Vec::new();

    for algo in &args.common.algo {
        let name = algo.to_string();
        let per_algo = match algo {
            Algo::NfaRpq => run_query_for_evaluator(&name, NfaRpqEvaluator, graph, queries),
            Algo::Rpqmatrix => run_query_for_evaluator(&name, RpqMatrixEvaluator, graph, queries),
        };
        merge_results(&mut all, per_algo);
    }

    all
}

pub fn dispatch_bench(
    args: &BenchArgs,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
    checkpointer: &mut Checkpointer,
) -> Result<Vec<QueryResult>, BenchError> {
    let mut criterion = build_criterion(args);
    let mut all = Vec::new();

    for algo in &args.common.algo {
        let name = algo.to_string();
        let per_algo = match algo {
            Algo::NfaRpq => run_bench_for_evaluator(
                args,
                algo,
                &name,
                NfaRpqEvaluator,
                graph,
                queries,
                checkpointer,
                &mut criterion,
            )?,
            Algo::Rpqmatrix => run_bench_for_evaluator(
                args,
                algo,
                &name,
                RpqMatrixEvaluator,
                graph,
                queries,
                checkpointer,
                &mut criterion,
            )?,
        };
        merge_results(&mut all, per_algo);
    }

    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::output::AlgoResult;
    use std::collections::HashMap;

    #[test]
    fn merge_results_combines_algorithms_by_query_index() {
        let mut all = vec![QueryResult {
            query_index: 0,
            query_id: "q0".into(),
            query_text: "query".into(),
            algorithms: HashMap::from([("nfarpq".into(), AlgoResult::ok(Some(1), None))]),
        }];
        let per_algo = vec![QueryResult {
            query_index: 0,
            query_id: "q0".into(),
            query_text: "query".into(),
            algorithms: HashMap::from([("rpqmatrix".into(), AlgoResult::ok(Some(1), None))]),
        }];

        merge_results(&mut all, per_algo);

        assert_eq!(all.len(), 1);
        assert_eq!(all[0].algorithms.len(), 2);
    }
}
