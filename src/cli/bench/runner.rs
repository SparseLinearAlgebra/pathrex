use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use criterion::{Criterion, black_box};

use crate::cli::args::{Algo, BenchArgs};
use crate::cli::bench::error::BenchError;
use crate::cli::bench::estimates::read_algo_timing;
use crate::cli::checkpoint::Checkpointer;
use crate::cli::loader::LoadedQuery;
use crate::cli::output::{AlgoResult, QueryResult};
use crate::eval::{Evaluator, PreparedEvaluator, ResultCount};
use crate::graph::InMemoryGraph;
use crate::rpq::{RpqError, RpqQuery};

/// Build a criterion instance from CLI bench args.
pub(crate) fn build_criterion(args: &BenchArgs) -> Criterion {
    let c = Criterion::default()
        .sample_size(args.sample_size)
        .warm_up_time(Duration::from_secs(args.warm_up))
        .measurement_time(Duration::from_secs(args.measurement))
        .output_directory(Path::new(&args.criterion_dir));
    if args.plots {
        c.with_plots()
    } else {
        c.without_plots()
    }
}

fn group_name(query_index: usize, algo_id: &str) -> String {
    format!("query_{query_index}_{algo_id}")
}

fn run_benchmark_group<E>(
    criterion: &mut Criterion,
    args: &BenchArgs,
    algo_name: &str,
    evaluator: E,
    query: &RpqQuery,
    graph: &InMemoryGraph,
    query_index: usize,
) -> Result<Result<crate::cli::output::AlgoTiming, BenchError>, RpqError>
where
    E: Evaluator<Query = RpqQuery, Error = RpqError> + Copy,
    E::Result: ResultCount,
{
    let mut prepared = evaluator.prepare(query, graph)?;
    let group = group_name(query_index, algo_name);

    {
        let mut g = criterion.benchmark_group(&group);

        g.bench_function("eval_total", |b| {
            b.iter(|| {
                let _ = black_box(evaluator.evaluate(query, graph));
            });
        });

        g.bench_function("eval_ffi_only", |b| {
            b.iter(|| {
                let _ = black_box(prepared.execute());
            });
        });

        g.finish();
    }

    Ok(read_algo_timing(Path::new(&args.criterion_dir), &group))
}

/// Run the bench loop for every query in `queries` for one evaluator.
///
/// One criterion group is created per `(query, algo)` pair; checkpoint persists
/// after every algo completion.
pub fn run_bench_for_evaluator<E>(
    args: &BenchArgs,
    algo: &Algo,
    algo_name: &str,
    evaluator: E,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
    checkpointer: &mut Checkpointer,
    criterion: &mut Criterion,
) -> Result<Vec<QueryResult>, BenchError>
where
    E: Evaluator<Query = RpqQuery, Error = RpqError> + Copy,
    E::Result: ResultCount,
{
    let mut results = Vec::with_capacity(queries.len());

    for (idx, loaded) in queries.iter().enumerate() {
        if checkpointer.is_algo_done(idx, algo) {
            eprintln!(
                "  [skip] query #{} id={} algo={algo_name} already done",
                idx, loaded.id
            );
            continue;
        }

        let mut algorithms: HashMap<String, AlgoResult> = HashMap::new();

        let query = match &loaded.parsed {
            Ok(q) => q,
            Err(e) => {
                let msg = e.to_string();
                eprintln!(
                    "  [error] query #{} (id={}) algo={} parse error: {}",
                    idx, loaded.id, algo_name, msg
                );
                algorithms.insert(algo_name.to_string(), AlgoResult::error(msg));
                checkpointer.mark_and_save(idx, algo)?;
                results.push(QueryResult {
                    query_index: idx,
                    query_id: loaded.id.clone(),
                    query_text: loaded.text.clone(),
                    algorithms,
                });
                continue;
            }
        };

        eprintln!("[query #{}] id={}", idx, loaded.id);
        eprintln!("  [bench] algo={algo_name}");

        match run_benchmark_group(criterion, args, algo_name, evaluator, query, graph, idx) {
            Ok(Ok(timing)) => {
                algorithms.insert(algo_name.to_string(), AlgoResult::ok(None, Some(timing)));
            }
            Ok(Err(e)) => return Err(e),
            Err(e) => {
                eprintln!(
                    "  [error] query #{} (id={}) algo={} prepare error: {}",
                    idx, loaded.id, algo_name, e
                );
                algorithms.insert(algo_name.to_string(), AlgoResult::error(e.to_string()));
            }
        }

        checkpointer.mark_and_save(idx, algo)?;
        results.push(QueryResult {
            query_index: idx,
            query_id: loaded.id.clone(),
            query_text: loaded.text.clone(),
            algorithms,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_name_is_filesystem_safe() {
        let g = group_name(7, "nfa");
        assert_eq!(g, "query_7_nfa");
        assert!(!g.contains('/'));
        assert!(!g.contains(' '));
    }
}
