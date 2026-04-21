//! Core benchmark loop and criterion integration for the `bench` subcommand.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::time::Duration;

use criterion::{Criterion, black_box};

use crate::graph::InMemoryGraph;
use crate::rpq::nfarpq::{NfaRpqEvaluator, PreparedNfaRpq};
use crate::rpq::rpqmatrix::{PreparedRpqMatrix, RpqMatrixEvaluator};
use crate::rpq::{RpqError, RpqEvaluator, RpqQuery};

use super::args::{Algo, BenchArgs};
use super::checkpoint::Checkpoint;
use super::loader::LoadedQuery;
use super::output::{AlgoResult, AlgoTiming, BatchResult, QueryResult, TimingStats};

/// Run a single evaluation and return the result count (nnz / reachable nodes).
///
/// Used by both the correctness-check pass before benchmarking and by the
/// `query` subcommand runner.
pub(crate) fn run_once(
    algo: &Algo,
    query: &RpqQuery,
    graph: &InMemoryGraph,
) -> Result<usize, RpqError> {
    match algo {
        Algo::Nfa => {
            let result = NfaRpqEvaluator.evaluate(query, graph)?;
            let count = result
                .reachable
                .nvals()
                .map_err(crate::rpq::RpqError::Graph)? as usize;
            Ok(count)
        }
        Algo::Rpqmatrix => {
            let result = RpqMatrixEvaluator.evaluate(query, graph)?;
            Ok(result.nnz as usize)
        }
    }
}

/// Run a batch of queries for a single algorithm (discards result counts).
///
/// Used inside criterion's measurement loop; returning counts would be
/// optimised away anyway, but we keep the call realistic with `black_box`.
fn run_batch_total(
    algo: &Algo,
    queries: &[&RpqQuery],
    graph: &InMemoryGraph,
) -> Result<(), RpqError> {
    for query in queries {
        let _ = black_box(run_once(algo, query, graph))?;
    }
    Ok(())
}

enum PreparedBatch {
    Nfa(Vec<PreparedNfaRpq>),
    Rpqmatrix(Vec<PreparedRpqMatrix>),
}

fn prepare_batch(
    algo: &Algo,
    queries: &[&RpqQuery],
    graph: &InMemoryGraph,
) -> Result<PreparedBatch, RpqError> {
    match algo {
        Algo::Nfa => Ok(PreparedBatch::Nfa(
            queries
                .iter()
                .map(|query| NfaRpqEvaluator.prepare(query, graph))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Algo::Rpqmatrix => Ok(PreparedBatch::Rpqmatrix(
            queries
                .iter()
                .map(|query| RpqMatrixEvaluator.prepare(query, graph))
                .collect::<Result<Vec<_>, _>>()?,
        )),
    }
}

fn run_prepared_batch(prepared: &mut PreparedBatch) -> Result<(), RpqError> {
    match prepared {
        PreparedBatch::Nfa(items) => {
            for item in items {
                let result = item.execute()?;
                let count = result
                    .reachable
                    .nvals()
                    .map_err(crate::rpq::RpqError::Graph)? as usize;
                let _ = black_box(count);
            }
        }
        PreparedBatch::Rpqmatrix(items) => {
            for item in items {
                let result = item.execute()?;
                let _ = black_box(result.nnz);
            }
        }
    }

    Ok(())
}

/// Read criterion timing estimates from its output directory.
///
/// After `group.finish()`, criterion writes:
/// `<criterion_dir>/<group_name>/<bench_name>/new/estimates.json`
fn read_criterion_estimates(
    criterion_dir: &str,
    group_name: &str,
    bench_name: &str,
) -> Option<TimingStats> {
    let path = Path::new(criterion_dir)
        .join(group_name)
        .join(bench_name)
        .join("new")
        .join("estimates.json");

    let file = File::open(&path).ok()?;
    let data: serde_json::Value = serde_json::from_reader(file).ok()?;

    let mean_ns = data["mean"]["point_estimate"].as_f64()?;
    let median_ns = data["median"]["point_estimate"].as_f64()?;
    let stddev_ns = data["std_dev"]["point_estimate"].as_f64()?;

    // Read sample count from sample.json if available.
    let sample_path = Path::new(criterion_dir)
        .join(group_name)
        .join(bench_name)
        .join("new")
        .join("sample.json");

    let iterations = File::open(&sample_path)
        .ok()
        .and_then(|f| serde_json::from_reader::<_, serde_json::Value>(f).ok())
        .and_then(|v| v["iters"].as_array().map(|a| a.len()))
        .unwrap_or(0);

    Some(TimingStats {
        mean_ns,
        median_ns,
        stddev_ns,
        iterations,
    })
}

fn read_algo_timing_estimates(criterion_dir: &str, group_name: &str) -> Option<AlgoTiming> {
    let total = read_criterion_estimates(criterion_dir, group_name, "eval_total")?;
    let ffi_only = read_criterion_estimates(criterion_dir, group_name, "eval_ffi_only")?;

    Some(AlgoTiming { total, ffi_only })
}

/// Run the full benchmark loop, processing queries in batches.
///
/// Queries are grouped into batches of `batch_size`. For each batch and
/// algorithm, criterion benchmarks the entire batch as a single unit
/// (all queries run sequentially per iteration).
/// After each batch the checkpoint is saved.
pub fn run_benchmarks(
    args: &BenchArgs,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
    checkpoint: &mut Checkpoint,
    checkpoint_path: &Path,
) -> Vec<BatchResult> {
    let criterion = Criterion::default()
        .sample_size(args.sample_size)
        .warm_up_time(Duration::from_secs(args.warm_up))
        .measurement_time(Duration::from_secs(args.measurement))
        .output_directory(Path::new(&args.criterion_dir));

    let mut criterion = if args.plots {
        criterion.with_plots()
    } else {
        criterion.without_plots()
    };

    let batch_size = args.batch_size.max(1);
    let mut batch_results: Vec<BatchResult> = Vec::new();

    // Collect queries that still need work.
    let active_queries: Vec<(usize, &LoadedQuery)> = queries
        .iter()
        .enumerate()
        .filter(|(idx, loaded)| {
            if checkpoint.is_fully_done(*idx, &args.common.algo) {
                eprintln!(
                    "[skip] query #{} (id={}) — all algorithms done",
                    idx, loaded.id
                );
                false
            } else {
                true
            }
        })
        .collect();

    for (batch_index, batch) in active_queries.chunks(batch_size).enumerate() {
        let batch_indices: Vec<usize> = batch.iter().map(|(idx, _)| *idx).collect();
        let batch_ids: Vec<&str> = batch.iter().map(|(_, l)| l.id.as_str()).collect();

        eprintln!(
            "\n[batch {}] queries {:?} (ids: {:?})",
            batch_index, batch_indices, batch_ids
        );

        // ── First pass: correctness check + collect valid queries per algo ──
        let mut per_query_results: Vec<QueryResult> = Vec::new();
        // algo key → list of (query_index, query_ref, result_count)
        let mut valid_queries_per_algo: HashMap<String, Vec<(usize, &RpqQuery, usize)>> =
            HashMap::new();

        for &(idx, loaded) in batch {
            let mut algo_results: HashMap<String, AlgoResult> = HashMap::new();

            let query = match &loaded.parsed {
                Ok(q) => q,
                Err(e) => {
                    eprintln!(
                        "  [error] query #{} (id={}) parse error: {}",
                        idx, loaded.id, e
                    );
                    for algo in &args.common.algo {
                        if !checkpoint.is_algo_done(idx, algo) {
                            algo_results.insert(algo.to_string(), AlgoResult::error(e.to_string()));
                            checkpoint.mark_algo_done(idx, &loaded.id, algo);
                        }
                    }
                    per_query_results.push(QueryResult {
                        query_index: idx,
                        query_id: loaded.id.clone(),
                        query_text: loaded.text.clone(),
                        algorithms: algo_results,
                    });
                    continue;
                }
            };

            for algo in &args.common.algo {
                if checkpoint.is_algo_done(idx, algo) {
                    continue;
                }

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_once(algo, query, graph)
                }));

                match result {
                    Ok(Ok(count)) => {
                        valid_queries_per_algo
                            .entry(algo.to_string())
                            .or_default()
                            .push((idx, query, count));
                    }
                    Ok(Err(e)) => {
                        eprintln!(
                            "  [error] query #{} (id={}) algo={}: {}",
                            idx, loaded.id, algo, e
                        );
                        algo_results.insert(algo.to_string(), AlgoResult::error(e.to_string()));
                        checkpoint.mark_algo_done(idx, &loaded.id, algo);
                    }
                    Err(panic_info) => {
                        let msg = format!("{:?}", panic_info);
                        eprintln!(
                            "  [panic] query #{} (id={}) algo={}: {}",
                            idx, loaded.id, algo, msg
                        );
                        algo_results.insert(algo.to_string(), AlgoResult::panic(msg));
                        checkpoint.mark_algo_done(idx, &loaded.id, algo);
                    }
                }
            }

            per_query_results.push(QueryResult {
                query_index: idx,
                query_id: loaded.id.clone(),
                query_text: loaded.text.clone(),
                algorithms: algo_results,
            });
        }

        // ── Second pass: criterion benchmark per algo over valid queries ──
        let mut batch_algo_timing: HashMap<String, Option<AlgoTiming>> = HashMap::new();

        for algo in &args.common.algo {
            let algo_key = algo.to_string();
            let Some(valid) = valid_queries_per_algo.get(&algo_key) else {
                continue;
            };
            if valid.is_empty() {
                continue;
            }

            eprintln!(
                "  [bench] algo={} — benchmarking {} queries as batch...",
                algo,
                valid.len()
            );

            let group_name = format!("batch{}_{}", batch_index, algo);
            let mut group = criterion.benchmark_group(&group_name);

            let algo_clone = algo.clone();
            let queries_clone: Vec<RpqQuery> = valid.iter().map(|(_, q, _)| (*q).clone()).collect();

            group.bench_function("eval_total", |b| {
                b.iter(|| {
                    let refs: Vec<&RpqQuery> = queries_clone.iter().collect();
                    let _ = black_box(run_batch_total(&algo_clone, &refs, graph));
                });
            });

            group.bench_function("eval_ffi_only", |b| {
                let refs: Vec<&RpqQuery> = queries_clone.iter().collect();
                let mut prepared =
                    prepare_batch(&algo_clone, &refs, graph).expect("prepare benchmark batch");
                b.iter(|| {
                    let _ = black_box(run_prepared_batch(&mut prepared));
                });
            });
            group.finish();

            let timing = read_algo_timing_estimates(&args.criterion_dir, &group_name);
            batch_algo_timing.insert(algo_key, timing);
        }

        // Assign timing + result counts to each query's algo result.
        for qr in &mut per_query_results {
            for algo in &args.common.algo {
                let algo_key = algo.to_string();
                // Only fill in queries that didn't already get an error/panic result.
                if qr.algorithms.contains_key(&algo_key) {
                    continue;
                }
                let timing = batch_algo_timing
                    .get(&algo_key)
                    .and_then(|t| t.as_ref())
                    .map(|t| AlgoTiming {
                        total: TimingStats {
                            mean_ns: t.total.mean_ns,
                            median_ns: t.total.median_ns,
                            stddev_ns: t.total.stddev_ns,
                            iterations: t.total.iterations,
                        },
                        ffi_only: TimingStats {
                            mean_ns: t.ffi_only.mean_ns,
                            median_ns: t.ffi_only.median_ns,
                            stddev_ns: t.ffi_only.stddev_ns,
                            iterations: t.ffi_only.iterations,
                        },
                    });
                // Attach the result count from the correctness-check pass.
                let result_count = valid_queries_per_algo.get(&algo_key).and_then(|v| {
                    v.iter()
                        .find(|(idx, _, _)| *idx == qr.query_index)
                        .map(|(_, _, c)| *c)
                });
                // Fallback: if we can't match by pointer, use the first count from
                // the batch (acceptable when batch_size == 1, which is the default).
                let result_count = result_count.or_else(|| {
                    valid_queries_per_algo
                        .get(&algo_key)
                        .and_then(|v| v.first().map(|(_, _, c)| *c))
                });
                qr.algorithms
                    .insert(algo_key.clone(), AlgoResult::ok(result_count, timing));
            }
        }

        // Mark all queries in this batch as done.
        for &(idx, loaded) in batch {
            for algo in &args.common.algo {
                checkpoint.mark_algo_done(idx, &loaded.id, algo);
            }
        }

        if let Err(e) = checkpoint.save(checkpoint_path) {
            eprintln!("[warn] failed to save checkpoint: {e}");
        }

        batch_results.push(BatchResult {
            batch_index,
            query_indices: batch_indices,
            queries: per_query_results,
        });
    }

    batch_results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_estimate_files(base: &Path, bench_name: &str, mean_ns: f64, iterations: usize) {
        let bench_dir = base.join("batch0_nfa").join(bench_name).join("new");
        fs::create_dir_all(&bench_dir).expect("create bench dir");
        fs::write(
            bench_dir.join("estimates.json"),
            format!(
                r#"{{"mean":{{"point_estimate":{mean_ns}}},"median":{{"point_estimate":{mean_ns}}},"std_dev":{{"point_estimate":0.0}}}}"#
            ),
        )
        .expect("write estimates");
        let sample = format!("{{\"iters\":[{}]}}", vec!["1"; iterations].join(","));
        fs::write(bench_dir.join("sample.json"), sample).expect("write sample");
    }

    #[test]
    fn read_split_criterion_estimates() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_estimate_files(dir.path(), "eval_total", 10.0, 3);
        write_estimate_files(dir.path(), "eval_ffi_only", 4.0, 5);

        let timing =
            read_algo_timing_estimates(dir.path().to_str().expect("utf8 path"), "batch0_nfa")
                .expect("split timing");

        assert_eq!(timing.total.mean_ns, 10.0);
        assert_eq!(timing.total.iterations, 3);
        assert_eq!(timing.ffi_only.mean_ns, 4.0);
        assert_eq!(timing.ffi_only.iterations, 5);
    }
}
