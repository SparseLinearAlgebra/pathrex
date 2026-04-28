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

use std::error::Error as StdError;
use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::Parser;
use thiserror::Error;

use pathrex::cli::args::{BenchArgs, Cli, Commands, QueryArgs};
use pathrex::cli::bench::BenchError;
use pathrex::cli::checkpoint::{Checkpoint, CheckpointError, Checkpointer};
use pathrex::cli::dispatch::{dispatch_bench, dispatch_query};
use pathrex::cli::loader::{GraphLoadError, LoadedQuery, load_graph, load_queries};
use pathrex::cli::output::{BenchMetadata, BenchOutput, QueryMetadata, QueryOutput};
use pathrex::graph::{GraphDecomposition, InMemoryGraph};

#[derive(Debug, Error)]
enum MainError {
    #[error(transparent)]
    Graph(#[from] GraphLoadError),
    #[error("error loading queries from '{path}': {source}")]
    Queries {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
    #[error(transparent)]
    Bench(#[from] BenchError),
    #[error("error writing output to '{path}': {source}")]
    Output {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        // Walk the source chain so users see the underlying cause.
        let mut cur: Option<&dyn StdError> = e.source();
        while let Some(c) = cur {
            eprintln!("  caused by: {c}");
            cur = c.source();
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), MainError> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Query(args) => run_query_cmd(args),
        Commands::Bench(args) => run_bench_cmd(args),
    }
}

fn load_query_file(path: &str, base_iri: Option<&str>) -> Result<Vec<LoadedQuery>, MainError> {
    load_queries(Path::new(path), base_iri).map_err(|e| MainError::Queries {
        path: path.to_string(),
        source: e,
    })
}

fn run_query_cmd(args: QueryArgs) -> Result<(), MainError> {
    let common = &args.common;

    eprintln!("=== pathrex query ===");
    eprintln!("Graph:   {}", common.graph);
    eprintln!("Format:  {}", common.format);
    eprintln!("Queries: {}", common.queries);
    eprintln!("Algos:   {:?}", common.algo);
    eprintln!();

    eprintln!("[1/2] Loading graph...");
    let graph: InMemoryGraph =
        load_graph(&common.graph, common.format, common.base_iri.as_deref())?;
    eprintln!("  nodes:  {}", graph.num_nodes());
    eprintln!("  labels: {}", graph.num_labels());
    eprintln!();

    eprintln!("[2/2] Loading and running queries...");
    let queries = load_query_file(&common.queries, common.base_iri.as_deref())?;
    eprintln!("  loaded {} queries", queries.len());

    let results = dispatch_query(&args, &graph, &queries);

    let errors = results
        .iter()
        .flat_map(|r| r.algorithms.values())
        .filter(|a| !matches!(a.status, pathrex::cli::output::AlgoStatus::Ok))
        .count();
    eprintln!();
    eprintln!(
        "Done. {} queries × {} algos. {errors} error(s).",
        results.len(),
        common.algo.len()
    );

    if let Some(ref out_path) = args.output {
        let output = QueryOutput {
            metadata: QueryMetadata {
                timestamp: Utc::now().to_rfc3339(),
                graph_path: common.graph.clone(),
                graph_format: common.format.to_string(),
                queries_file: common.queries.clone(),
                base_iri: common.base_iri.clone(),
                num_nodes: graph.num_nodes(),
                num_labels: graph.num_labels(),
            },
            results,
        };
        output
            .write_to_file(Path::new(out_path))
            .map_err(|e| MainError::Output {
                path: out_path.clone(),
                source: e,
            })?;
        eprintln!("Results written to: {out_path}");
    }

    Ok(())
}

fn build_checkpointer(args: &BenchArgs, queries_len: usize) -> Result<Checkpointer, MainError> {
    let common = &args.common;
    let path = PathBuf::from(&args.checkpoint);

    if args.resume {
        match Checkpoint::load(&path)? {
            Some(cp) => {
                cp.validate(&common.graph, &common.queries, &common.algo)?;
                let cper = Checkpointer::with_inner(cp, path);
                eprintln!(
                    "  resuming: {}/{} queries fully done",
                    cper.fully_done_count(&common.algo),
                    queries_len
                );
                Ok(cper)
            }
            None => {
                eprintln!("  no checkpoint file found, starting fresh");
                Ok(Checkpointer::fresh(
                    &common.graph,
                    &common.queries,
                    &common.algo,
                    path,
                ))
            }
        }
    } else {
        Ok(Checkpointer::fresh(
            &common.graph,
            &common.queries,
            &common.algo,
            path,
        ))
    }
}

fn run_bench_cmd(args: BenchArgs) -> Result<(), MainError> {
    let common = &args.common;

    eprintln!("=== pathrex bench ===");
    eprintln!("Graph:      {}", common.graph);
    eprintln!("Format:     {}", common.format);
    eprintln!("Queries:    {}", common.queries);
    eprintln!("Algos:      {:?}", common.algo);
    eprintln!("Output:     {}", args.output);
    eprintln!();

    eprintln!("[1/4] Loading graph...");
    let graph: InMemoryGraph =
        load_graph(&common.graph, common.format, common.base_iri.as_deref())?;
    eprintln!("  nodes:  {}", graph.num_nodes());
    eprintln!("  labels: {}", graph.num_labels());
    eprintln!();

    eprintln!("[2/4] Loading queries...");
    let queries = load_query_file(&common.queries, common.base_iri.as_deref())?;
    eprintln!("  loaded {} queries", queries.len());
    let parse_errors = queries.iter().filter(|q| q.parsed.is_err()).count();
    if parse_errors > 0 {
        eprintln!("  ({parse_errors} queries failed to parse)");
    }
    eprintln!();

    eprintln!("[3/4] Setting up checkpoint...");
    let mut checkpointer = build_checkpointer(&args, queries.len())?;
    eprintln!();

    eprintln!("[4/4] Running benchmarks...");
    eprintln!();
    let results = dispatch_bench(&args, &graph, &queries, &mut checkpointer)?;

    let output = BenchOutput {
        metadata: BenchMetadata {
            timestamp: Utc::now().to_rfc3339(),
            graph_path: common.graph.clone(),
            graph_format: common.format.to_string(),
            queries_file: common.queries.clone(),
            base_iri: common.base_iri.clone(),
            num_nodes: graph.num_nodes(),
            num_labels: graph.num_labels(),
            sample_size: args.sample_size,
            warm_up_secs: args.warm_up,
            measurement_secs: args.measurement,
        },
        results,
    };

    output
        .write_to_file(Path::new(&args.output))
        .map_err(|e| MainError::Output {
            path: args.output.clone(),
            source: e,
        })?;

    eprintln!();
    eprintln!("=== Done ===");
    eprintln!("Results written to: {}", args.output);
    eprintln!("Criterion data in:  {}", args.criterion_dir);

    Ok(())
}
