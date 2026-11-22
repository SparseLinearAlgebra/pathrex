use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, black_box};

use crate::cli::args::{Algo, BenchArgs, BenchMode};
use crate::cli::bench::error::BenchError;
use crate::cli::bench::estimates::read_algo_timing;
use crate::cli::checkpoint::Checkpointer;
use crate::cli::loader::LoadedQuery;
use crate::cli::output::{AlgoResult, AlgoTiming, AlgoTimingSamples, QueryResult, TimingStats};
use crate::eval::{Evaluator, PreparedEvaluator, ResultCount};
use crate::graph::InMemoryGraph;
use crate::rpq::{RpqError, RpqQuery};

/// Persist outside every native timing interval, before marking a query done.
fn append_query_result(
    args: &BenchArgs,
    query_index: usize,
    loaded: &LoadedQuery,
    algorithm: &str,
    count: Option<usize>,
    timing: &AlgoTiming,
) -> Result<(), BenchError> {
    let Some(path) = &args.results_jsonl else {
        return Ok(());
    };
    let write = || -> std::io::Result<()> {
        let row = serde_json::json!({
            "query_index": query_index,
            "query_id": loaded.id,
            "query_text": loaded.text,
            "algorithm": algorithm,
            "optimizer": args.common.rpqmatrix_optimizer.to_string(),
            "bench_mode": args.bench_mode.to_string(),
            "result_count": count,
            "timing": timing,
            "samples": timing.samples,
        });
        let mut bytes = serde_json::to_vec(&row).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_data()
    };
    write().map_err(|source| BenchError::ResultsIo {
        path: path.clone(),
        source,
    })
}

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
        .sample_size(args.criterion_sample_size())
        .warm_up_time(Duration::from_secs(args.criterion_warm_up_secs()))
        .measurement_time(Duration::from_secs(args.criterion_measurement_secs()))
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
    E: Evaluator<Query = RpqQuery, Error = RpqError>,
    E::Result: ResultCount,
{
    // Validate preparation once so query/graph errors are reported through the
    // normal benchmark error path. The `eval_ffi_only` benchmark below creates a
    // fresh prepared state per measured iteration.
    let _prepared = evaluator.prepare(query, graph)?;
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
            b.iter_batched(
                || {
                    evaluator
                        .prepare(query, graph)
                        .expect("prepare should keep succeeding during benchmark")
                },
                |mut prepared| {
                    let _ = black_box(prepared.execute());
                },
                BatchSize::PerIteration,
            );
        });

        g.finish();
    }

    Ok(read_algo_timing(&output_path, &group))
}

fn timing_stats(samples_ns: &[f64]) -> TimingStats {
    let mut sorted = samples_ns.to_vec();
    sorted.sort_by(f64::total_cmp);

    let len = sorted.len();
    let mean = sorted.iter().sum::<f64>() / len as f64;
    let median = if len.is_multiple_of(2) {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    };
    let variance = sorted
        .iter()
        .map(|sample| {
            let diff = sample - mean;
            diff * diff
        })
        .sum::<f64>()
        / len as f64;

    TimingStats {
        mean_ns: mean,
        median_ns: median,
        stddev_ns: variance.sqrt(),
        iterations: len,
    }
}

fn elapsed_ns(start: Instant) -> f64 {
    start.elapsed().as_nanos() as f64
}

fn run_fixed_group<E>(
    args: &BenchArgs,
    evaluator: E,
    query: &RpqQuery,
    graph: &InMemoryGraph,
) -> Result<(usize, AlgoTiming), RpqError>
where
    E: Evaluator<Query = RpqQuery, Error = RpqError>,
    E::Result: ResultCount,
{
    for _ in 0..args.fixed_warm_up_runs() {
        let _ = black_box(evaluator.evaluate(query, graph)?);
    }

    let mut total_samples = Vec::with_capacity(args.fixed_runs() as usize);
    let mut result_count = None;
    for _ in 0..args.fixed_runs() {
        let start = Instant::now();
        let result = black_box(evaluator.evaluate(query, graph)?);
        total_samples.push(elapsed_ns(start));
        result_count = Some(result.result_count().map_err(RpqError::Graph)?);
    }

    for _ in 0..args.fixed_warm_up_runs() {
        let mut prepared = evaluator.prepare(query, graph)?;
        let _ = black_box(prepared.execute()?);
    }

    let mut ffi_samples = Vec::with_capacity(args.fixed_runs() as usize);
    for _ in 0..args.fixed_runs() {
        let mut prepared = evaluator.prepare(query, graph)?;
        let start = Instant::now();
        let result = black_box(prepared.execute()?);
        ffi_samples.push(elapsed_ns(start));
        drop(result);
    }

    Ok((
        result_count.unwrap_or(0),
        AlgoTiming {
            total: timing_stats(&total_samples),
            ffi_only: timing_stats(&ffi_samples),
            samples: Some(AlgoTimingSamples {
                total_ns: total_samples,
                ffi_only_ns: ffi_samples,
            }),
        },
    ))
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
    E: Evaluator<Query = RpqQuery, Error = RpqError> + Clone,
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

        let bench_result = match args.bench_mode {
            BenchMode::Fixed => run_fixed_group(args, evaluator.clone(), query, graph)
                .map(|(count, timing)| Ok((Some(count), timing))),
            BenchMode::Criterion => {
                run_benchmark_group(args, algo_name, evaluator.clone(), query, graph, idx)
                    .map(|result| result.map(|timing| (None, timing)))
            }
        };

        match bench_result {
            Ok(Ok((count, timing))) => {
                append_query_result(args, idx, loaded, algo_name, count, &timing)?;
                algorithms.insert(algo_name.to_string(), AlgoResult::ok(count, Some(timing)));
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
mod incremental_output_tests {
    use super::*;
    use crate::cli::args::{Cli, Commands};
    use crate::rpq::{Endpoint, PathExpr};
    use clap::Parser;

    #[test]
    fn incremental_results_keep_raw_samples_and_append_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("results.jsonl");
        let cli = Cli::try_parse_from([
            "pathrex",
            "bench",
            "-g",
            "graph",
            "-q",
            "queries",
            "-a",
            "rpqmatrix",
            "--results-jsonl",
            path.to_str().unwrap(),
        ])
        .unwrap();
        let Commands::Bench(mut args) = cli.command else {
            panic!("expected bench");
        };
        let loaded = LoadedQuery {
            id: "q1".into(),
            text: "?x <p> ?y".into(),
            parsed: Ok(RpqQuery {
                subject: Endpoint::Variable("x".into()),
                path: PathExpr::Label("p".into()),
                object: Endpoint::Variable("y".into()),
            }),
        };
        let samples = vec![10.0, 20.0];
        let timing = AlgoTiming {
            total: timing_stats(&samples),
            ffi_only: timing_stats(&samples),
            samples: Some(AlgoTimingSamples {
                total_ns: samples.clone(),
                ffi_only_ns: samples,
            }),
        };
        append_query_result(&args, 0, &loaded, "rpqmatrix", Some(3), &timing).unwrap();
        append_query_result(&args, 1, &loaded, "rpqmatrix", Some(3), &timing).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["query_index"], 0);
        assert_eq!(rows[1]["query_index"], 1);
        assert_eq!(
            rows[0]["samples"]["total_ns"],
            serde_json::json!([10.0, 20.0])
        );
        assert_eq!(
            rows[0]["samples"]["ffi_only_ns"],
            serde_json::json!([10.0, 20.0])
        );
        args.results_jsonl = Some(
            directory
                .path()
                .join("missing/output.jsonl")
                .display()
                .to_string(),
        );
        assert!(matches!(
            append_query_result(&args, 2, &loaded, "rpqmatrix", Some(3), &timing),
            Err(BenchError::ResultsIo { .. })
        ));
    }
}
