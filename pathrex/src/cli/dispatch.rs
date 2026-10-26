//! Typed dispatch from CLI algorithm choices to concrete evaluators.
use crate::cli::args::{Algo, BenchArgs, CommonArgs, QueryArgs, RpqMatrixOptimizer};
use crate::cli::bench::error::BenchError;
use crate::cli::bench::runner::run_bench_for_evaluator;
use crate::cli::checkpoint::Checkpointer;
use crate::cli::loader::LoadedQuery;
use crate::cli::output::QueryResult;
use crate::cli::query::run_query_for_evaluator;
use crate::graph::InMemoryGraph;

use crate::rpq::nfarpq::NfaRpqEvaluator;
use crate::rpq::rpqmatrix::{LinearCostModel, LinearModelError, RpqMatrixEvaluator};

fn matrix_evaluator(
    args: &CommonArgs,
    model: Option<&LinearCostModel>,
) -> Result<RpqMatrixEvaluator, LinearModelError> {
    if args.rpqmatrix_optimizer == RpqMatrixOptimizer::Stochastic {
        let model = model.ok_or_else(|| {
            LinearModelError::InvalidModel(
                "a loaded linear model is required for stochastic".into(),
            )
        })?;
        Ok(RpqMatrixEvaluator::with_linear_model(model.clone()))
    } else {
        Ok(RpqMatrixEvaluator::optimized(
            args.rpqmatrix_optimizer.into(),
        ))
    }
}

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
    model: Option<&LinearCostModel>,
) -> Result<Vec<QueryResult>, LinearModelError> {
    let mut all = Vec::new();
    for algo in &args.common.algo {
        let name = algo.to_string();
        let per_algo = match algo {
            Algo::NfaRpq => run_query_for_evaluator(&name, NfaRpqEvaluator, graph, queries),
            Algo::Rpqmatrix => run_query_for_evaluator(
                &name,
                matrix_evaluator(&args.common, model)?,
                graph,
                queries,
            ),
        };
        merge_results(&mut all, per_algo);
    }

    Ok(all)
}

pub fn dispatch_bench(
    args: &BenchArgs,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
    checkpointer: &mut Checkpointer,
    model: Option<&LinearCostModel>,
) -> Result<Vec<QueryResult>, BenchError> {
    checkpointer.validate_linear_model(model)?;
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
            )?,
            Algo::Rpqmatrix => run_bench_for_evaluator(
                args,
                algo,
                &name,
                matrix_evaluator(&args.common, model)?,
                graph,
                queries,
                checkpointer,
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
    fn evaluator_uses_the_loaded_snapshot_without_reopening_the_model_file() {
        use crate::cli::args::{Cli, Commands};
        use clap::Parser;
        let Commands::Query(args) = Cli::parse_from([
            "pathrex",
            "query",
            "-g",
            "graph",
            "-q",
            "queries",
            "-a",
            "rpqmatrix",
            "-p",
            "stochastic",
            "--linear-model",
            "nonexistent.model",
        ])
        .command
        else {
            panic!("query");
        };
        let model = LinearCostModel::from_coefficients([1.0, 2.0, 3.0]).unwrap();
        assert!(matrix_evaluator(&args.common, Some(&model)).is_ok());
        assert!(matrix_evaluator(&args.common, None).is_err());
    }

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
