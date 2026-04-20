//! Entry point for the `pathrex` binary.
//!
//! Subcommands:
//! - `query`  — run queries once and report result counts
//! - `bench`  — benchmark RPQ evaluators with criterion
//!
//! # Examples
//!
//! ```bash
//! # Run queries once (prints per-query result counts):
//! cargo run --release --bin pathrex --features bench -- query \
//!   --graph tests/testdata/mm_graph \
//!   --queries tests/testdata/cases/any-any/queries.txt
//!
//! # Benchmark with criterion:
//! cargo run --release --bin pathrex --features bench -- bench \
//!   --graph tests/testdata/mm_graph \
//!   --queries tests/testdata/cases/any-any/queries.txt \
//!   --algo nfa rpqmatrix \
//!   --output results.json
//! ```

use std::collections::HashSet;
use std::path::Path;
use std::process;

use chrono::Utc;
use clap::Parser;

use pathrex::cli::args::{Cli, Commands};
use pathrex::cli::bench::run_benchmarks;
use pathrex::cli::checkpoint::Checkpoint;
use pathrex::cli::loader::{load_graph, load_queries};
use pathrex::cli::output::{BenchMetadata, BenchOutput, QueryMetadata, QueryOutput};
use pathrex::cli::query::run_queries;
use pathrex::graph::GraphDecomposition;

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Query(args) => {
            let common = &args.common;

            eprintln!("=== pathrex query ===");
            eprintln!("Graph:   {}", common.graph);
            eprintln!("Format:  {}", common.format);
            eprintln!("Queries: {}", common.queries);
            eprintln!("Algos:   {:?}", common.algo);
            eprintln!();

            eprintln!("[1/2] Loading graph...");
            let graph = load_graph(&common.graph, &common.format, &common.base_iri);
            eprintln!("  nodes:  {}", graph.num_nodes());
            eprintln!("  labels: {}", graph.num_labels());
            eprintln!();

            eprintln!("[2/2] Loading and running queries...");
            let queries_path = Path::new(&common.queries);
            let queries = load_queries(queries_path, &common.base_iri).unwrap_or_else(|e| {
                eprintln!("Error loading queries from '{}': {e}", common.queries);
                process::exit(1);
            });
            eprintln!("  loaded {} queries", queries.len());

            let results = run_queries(&args, &graph, &queries);

            // Summary
            let errors = results
                .iter()
                .flat_map(|r| r.algorithms.values())
                .filter(|a| a.status != "ok")
                .count();
            eprintln!();
            eprintln!(
                "Done. {} queries × {} algos. {errors} error(s).",
                results.len(),
                common.algo.len()
            );

            // Optional JSON output
            if let Some(ref out_path) = args.output {
                let output = QueryOutput {
                    metadata: QueryMetadata {
                        timestamp: Utc::now().to_rfc3339(),
                        graph_path: common.graph.clone(),
                        graph_format: common.format.clone(),
                        queries_file: common.queries.clone(),
                        base_iri: common.base_iri.clone(),
                        num_nodes: graph.num_nodes(),
                        num_labels: graph.num_labels(),
                    },
                    results,
                };
                if let Err(e) = output.write_to_file(Path::new(out_path)) {
                    eprintln!("Error writing output to '{out_path}': {e}");
                    process::exit(1);
                }
                eprintln!("Results written to: {out_path}");
            }
        }

        Commands::Bench(args) => {
            let common = &args.common;

            eprintln!("=== pathrex bench ===");
            eprintln!("Graph:      {}", common.graph);
            eprintln!("Format:     {}", common.format);
            eprintln!("Queries:    {}", common.queries);
            eprintln!("Algos:      {:?}", common.algo);
            eprintln!("Batch size: {}", args.batch_size);
            eprintln!("Output:     {}", args.output);
            eprintln!();

            eprintln!("[1/4] Loading graph...");
            let graph = load_graph(&common.graph, &common.format, &common.base_iri);
            eprintln!("  nodes:  {}", graph.num_nodes());
            eprintln!("  labels: {}", graph.num_labels());
            eprintln!();

            eprintln!("[2/4] Loading queries...");
            let queries_path = Path::new(&common.queries);
            let queries = load_queries(queries_path, &common.base_iri).unwrap_or_else(|e| {
                eprintln!("Error loading queries from '{}': {e}", common.queries);
                process::exit(1);
            });
            eprintln!("  loaded {} queries", queries.len());
            let parse_errors = queries.iter().filter(|q| q.parsed.is_err()).count();
            if parse_errors > 0 {
                eprintln!("  ({parse_errors} queries failed to parse)");
            }
            eprintln!();

            eprintln!("[3/4] Setting up checkpoint...");
            let checkpoint_path = Path::new(&args.checkpoint);
            let mut checkpoint = if args.resume {
                match Checkpoint::load(checkpoint_path) {
                    Ok(Some(cp)) => {
                        if let Err(e) = cp.validate(&common.graph, &common.queries, &common.algo) {
                            eprintln!("Checkpoint validation failed: {e}");
                            process::exit(1);
                        }
                        let done_count = cp
                            .completed
                            .iter()
                            .filter(|c| {
                                let done: HashSet<_> = c.algorithms_done.iter().collect();
                                common.algo.iter().all(|a| done.contains(a))
                            })
                            .count();
                        eprintln!(
                            "  resuming: {done_count}/{} queries fully done",
                            queries.len()
                        );
                        cp
                    }
                    Ok(None) => {
                        eprintln!("  no checkpoint file found, starting fresh");
                        Checkpoint::new(&common.graph, &common.queries, &common.algo)
                    }
                    Err(e) => {
                        eprintln!("Error loading checkpoint: {e}");
                        process::exit(1);
                    }
                }
            } else {
                Checkpoint::new(&common.graph, &common.queries, &common.algo)
            };
            eprintln!();

            eprintln!("[4/4] Running benchmarks...");
            eprintln!();
            let results = run_benchmarks(&args, &graph, &queries, &mut checkpoint, checkpoint_path);

            let output = BenchOutput {
                metadata: BenchMetadata {
                    timestamp: Utc::now().to_rfc3339(),
                    graph_path: common.graph.clone(),
                    graph_format: common.format.clone(),
                    queries_file: common.queries.clone(),
                    base_iri: common.base_iri.clone(),
                    num_nodes: graph.num_nodes(),
                    num_labels: graph.num_labels(),
                    sample_size: args.sample_size,
                    warm_up_secs: args.warm_up,
                    measurement_secs: args.measurement,
                    batch_size: args.batch_size,
                },
                results,
            };

            let output_path = Path::new(&args.output);
            if let Err(e) = output.write_to_file(output_path) {
                eprintln!("Error writing output to '{}': {e}", args.output);
                process::exit(1);
            }

            eprintln!();
            eprintln!("=== Done ===");
            eprintln!("Results written to: {}", args.output);
            eprintln!("Criterion data in:  {}", args.criterion_dir);
        }
    }
}
