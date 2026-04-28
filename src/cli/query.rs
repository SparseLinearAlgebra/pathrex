//! Single-shot query runner for the `query` subcommand.
//!
//! Runs each query once per algorithm, prints a per-query summary to stderr,
//! and returns structured results that the binary can optionally write to JSON.

use std::collections::HashMap;

use crate::eval::{Evaluator, ResultCount};
use crate::graph::InMemoryGraph;
use crate::rpq::{RpqError, RpqQuery};

use super::loader::LoadedQuery;
use super::output::{AlgoResult, QueryResult};

/// Run all queries once for one evaluator and return structured results.
pub fn run_query_for_evaluator<E>(
    algo_name: &str,
    evaluator: E,
    graph: &InMemoryGraph,
    queries: &[LoadedQuery],
) -> Vec<QueryResult>
where
    E: Evaluator<Query = RpqQuery, Error = RpqError> + Copy,
    E::Result: ResultCount,
{
    let mut results = Vec::with_capacity(queries.len());

    for (idx, loaded) in queries.iter().enumerate() {
        let mut algo_results: HashMap<String, AlgoResult> = HashMap::new();

        let query: &RpqQuery = match &loaded.parsed {
            Ok(q) => q,
            Err(e) => {
                eprintln!("[query #{idx}] id={} — parse error: {e}", loaded.id);
                algo_results.insert(algo_name.to_string(), AlgoResult::error(e.to_string()));
                results.push(QueryResult {
                    query_index: idx,
                    query_id: loaded.id.clone(),
                    query_text: loaded.text.clone(),
                    algorithms: algo_results,
                });
                continue;
            }
        };

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            evaluator
                .evaluate(query, graph)
                .and_then(|result| result.result_count().map_err(RpqError::Graph))
        }));

        let algo_result = match outcome {
            Ok(Ok(count)) => {
                eprintln!(
                    "[query #{idx}] id={} algo={} — {count} count",
                    loaded.id, algo_name
                );
                AlgoResult::ok(Some(count), None)
            }
            Ok(Err(e)) => {
                eprintln!(
                    "[query #{idx}] id={} algo={} — error: {e}",
                    loaded.id, algo_name
                );
                AlgoResult::error(e.to_string())
            }
            Err(panic_info) => {
                let msg = format!("{:?}", panic_info);
                eprintln!(
                    "[query #{idx}] id={} algo={} — panic: {msg}",
                    loaded.id, algo_name
                );
                AlgoResult::panic(msg)
            }
        };

        algo_results.insert(algo_name.to_string(), algo_result);

        results.push(QueryResult {
            query_index: idx,
            query_id: loaded.id.clone(),
            query_text: loaded.text.clone(),
            algorithms: algo_results,
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpq::nfarpq::NfaRpqEvaluator;
    use crate::utils::build_graph;

    #[test]
    fn generic_runner_records_result_for_one_algorithm() {
        let graph = build_graph(&[("A", "B", "p")]);
        let queries = vec![LoadedQuery {
            id: "q1".into(),
            text: "SELECT ?x ?y WHERE { ?x <p> ?y . }".into(),
            parsed: Ok(RpqQuery {
                subject: crate::rpq::Endpoint::Variable("x".into()),
                path: crate::rpq::PathExpr::Label("p".into()),
                object: crate::rpq::Endpoint::Variable("y".into()),
            }),
        }];

        let results = run_query_for_evaluator("nfarpq", NfaRpqEvaluator, &graph, &queries);

        assert_eq!(results.len(), 1);
        assert!(results[0].algorithms.contains_key("nfarpq"));
    }
}
