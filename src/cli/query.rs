//! Single-shot query runner for the `query` subcommand.
//!
//! Runs each query once per algorithm, prints a per-query summary to stderr,
//! and returns structured results that the binary can optionally write to JSON.

use std::collections::HashMap;

use crate::graph::InMemoryGraph;
use crate::rpq::RpqQuery;

use super::args::QueryArgs;
use super::bench::run_once;
use super::loader::LoadedQuery;
use super::output::{AlgoResult, QueryResult};

/// Run all queries once per algorithm and return structured results.
///
/// Progress and per-query summaries are printed to stderr. No checkpoint
/// or criterion involvement — this is a simple single-pass execution.
pub fn run_queries(
    args: &QueryArgs,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
) -> Vec<QueryResult> {
    let mut results = Vec::with_capacity(queries.len());

    for (idx, loaded) in queries.iter().enumerate() {
        let mut algo_results: HashMap<String, AlgoResult> = HashMap::new();

        let query: &RpqQuery = match &loaded.parsed {
            Ok(q) => q,
            Err(e) => {
                eprintln!("[query #{idx}] id={} — parse error: {e}", loaded.id);
                for algo in &args.common.algo {
                    algo_results.insert(algo.to_string(), AlgoResult::error(e.to_string()));
                }
                results.push(QueryResult {
                    query_index: idx,
                    query_id: loaded.id.clone(),
                    query_text: loaded.text.clone(),
                    algorithms: algo_results,
                });
                continue;
            }
        };

        for algo in &args.common.algo {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_once(algo, query, graph)
            }));

            let algo_result = match outcome {
                Ok(Ok(count)) => {
                    eprintln!(
                        "[query #{idx}] id={} algo={} — {count} result(s)",
                        loaded.id, algo
                    );
                    AlgoResult::ok(Some(count), None)
                }
                Ok(Err(e)) => {
                    eprintln!("[query #{idx}] id={} algo={} — error: {e}", loaded.id, algo);
                    AlgoResult::error(e.to_string())
                }
                Err(panic_info) => {
                    let msg = format!("{:?}", panic_info);
                    eprintln!(
                        "[query #{idx}] id={} algo={} — panic: {msg}",
                        loaded.id, algo
                    );
                    AlgoResult::panic(msg)
                }
            };

            algo_results.insert(algo.to_string(), algo_result);
        }

        results.push(QueryResult {
            query_index: idx,
            query_id: loaded.id.clone(),
            query_text: loaded.text.clone(),
            algorithms: algo_results,
        });
    }

    results
}
