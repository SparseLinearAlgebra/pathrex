use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

/// Per-group criterion output destination.
pub(crate) enum GroupOutput {
    Temp(tempfile::TempDir),
    Persistent(PathBuf),
}

impl GroupOutput {
    pub(crate) fn for_group(args: &BenchArgs) -> Result<Self, BenchError> {
        match &args.criterion_dir {
            Some(p) => Ok(Self::Persistent(PathBuf::from(p))),
            None => {
                let td = tempfile::tempdir().map_err(BenchError::TempDir)?;
                Ok(Self::Temp(td))
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Temp(td) => td.path(),
            Self::Persistent(p) => p.as_path(),
        }
    }
}

pub(crate) fn build_criterion(args: &BenchArgs, output_dir: &Path) -> Criterion {
    let c = Criterion::default()
        .sample_size(args.sample_size)
        .warm_up_time(Duration::from_secs(args.warm_up))
        .measurement_time(Duration::from_secs(args.measurement))
        .output_directory(output_dir);
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

    let output = match GroupOutput::for_group(args) {
        Ok(o) => o,
        Err(e) => return Ok(Err(e)),
    };
    let output_path = output.path().to_path_buf();

    let mut criterion = build_criterion(args, &output_path);

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

    Ok(read_algo_timing(&output_path, &group))
}

/// Run the bench loop for every query in `queries` for one evaluator.
pub fn run_bench_for_evaluator<E>(
    args: &BenchArgs,
    algo: &Algo,
    algo_name: &str,
    evaluator: E,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
    checkpointer: &mut Checkpointer,
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

        match run_benchmark_group(args, algo_name, evaluator, query, graph, idx) {
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
