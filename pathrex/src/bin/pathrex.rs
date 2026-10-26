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
//!   --algo nfarpq rpqmatrix \
//!   --bench-mode criterion \
//!   --rpqmatrix-optimizer join \
//!   --output results.json
//! ```

use std::error::Error as StdError;
use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::Parser;
use thiserror::Error;

use pathrex::cli::args::{
    Algo, BenchArgs, BenchMode, Cli, Commands, CommonArgs, GraphFormat, QueryArgs,
    RpqMatrixOptimizer, TrainLinearArgs,
};
use pathrex::cli::bench::BenchError;
use pathrex::cli::checkpoint::{BenchRunConfig, Checkpoint, CheckpointError, Checkpointer};
use pathrex::cli::dispatch::{dispatch_bench, dispatch_query};
use pathrex::cli::loader::{GraphLoadError, LoadedQuery, load_graph_with_optimizer, load_queries};
use pathrex::cli::output::{BenchMetadata, BenchOutput, QueryMetadata, QueryOutput};
use pathrex::graph::{GraphDecomposition, InMemoryGraph};
use pathrex::rpq::RpqError;
use pathrex::rpq::rpqmatrix::{LinearCostModel, LinearModelError, RpqMatrixEvaluator};

#[derive(Debug, Error)]
enum MainError {
    #[error(transparent)]
    LinearModel(#[from] LinearModelError),
    #[error("training query {id}: {source}")]
    Training {
        id: String,
        #[source]
        source: RpqError,
    },
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
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
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
        Commands::TrainLinear(args) => run_train_linear_cmd(args),
    }
}

fn validate_common_args(common: &CommonArgs) -> Result<(), MainError> {
    let linear = common.rpqmatrix_optimizer == RpqMatrixOptimizer::Stochastic;
    if linear != common.linear_model.is_some() {
        return Err(MainError::InvalidArgs(
            "--linear-model must be supplied exactly when --rpqmatrix-optimizer is stochastic"
                .into(),
        ));
    }
    if common.rpqmatrix_optimizer == RpqMatrixOptimizer::None {
        return Ok(());
    }

    if !common.algo.contains(&Algo::Rpqmatrix) {
        return Err(MainError::InvalidArgs(
            "--rpqmatrix-optimizer can only be used when --algo includes rpqmatrix".to_string(),
        ));
    }

    if common.format != GraphFormat::Mm {
        return Err(MainError::InvalidArgs(
            "--rpqmatrix-optimizer can only be used with --format mm".to_string(),
        ));
    }

    Ok(())
}

// Load once: execution, checkpoint identity, and output refer to this same
// immutable snapshot even if the source file changes while queries run.
fn load_linear_model(common: &CommonArgs) -> Result<Option<LinearCostModel>, MainError> {
    common
        .linear_model
        .as_ref()
        .map(|path| LinearCostModel::load(path).map_err(MainError::from))
        .transpose()
}

fn validate_bench_args(args: &BenchArgs) -> Result<(), MainError> {
    validate_common_args(&args.common)?;

    if args.resume && args.checkpoint.is_none() {
        return Err(MainError::InvalidArgs(
            "--resume requires --checkpoint".to_string(),
        ));
    }

    match args.bench_mode {
        BenchMode::Fixed => {
            if args.criterion_dir.is_some()
                || args.plots
                || args.sample_size.is_some()
                || args.warm_up.is_some()
                || args.measurement.is_some()
            {
                return Err(MainError::InvalidArgs(
                    "criterion options can only be used with --bench-mode criterion".to_string(),
                ));
            }
            if args.fixed_runs() == 0 {
                return Err(MainError::InvalidArgs(
                    "--runs must be greater than 0".to_string(),
                ));
            }
        }
        BenchMode::Criterion => {
            if args.runs.is_some() || args.warm_up_runs.is_some() {
                return Err(MainError::InvalidArgs(
                    "fixed-run options can only be used with --bench-mode fixed".to_string(),
                ));
            }
            if args.plots && args.criterion_dir.is_none() {
                return Err(MainError::InvalidArgs(
                    "--plots requires --criterion-dir".to_string(),
                ));
            }
            if args.criterion_sample_size() < 10 {
                return Err(MainError::InvalidArgs(
                    "--sample-size must be at least 10 for criterion".to_string(),
                ));
            }
        }
    }

    Ok(())
}

fn load_query_file(path: &str, base_iri: Option<&str>) -> Result<Vec<LoadedQuery>, MainError> {
    load_queries(Path::new(path), base_iri).map_err(|e| MainError::Queries {
        path: path.to_string(),
        source: e,
    })
}

fn run_query_cmd(args: QueryArgs) -> Result<(), MainError> {
    let common = &args.common;
    validate_common_args(common)?;
    let model = load_linear_model(common)?;

    eprintln!("=== pathrex query ===");
    eprintln!("Graph:   {}", common.graph);
    eprintln!("Format:  {}", common.format);
    eprintln!("Queries: {}", common.queries);
    eprintln!("Algos:   {:?}", common.algo);
    eprintln!("RPQMatrix optimizer: {}", common.rpqmatrix_optimizer);
    eprintln!();

    eprintln!("[1/2] Loading graph...");
    let graph: InMemoryGraph = load_graph_with_optimizer(
        &common.graph,
        common.format,
        common.base_iri.as_deref(),
        common.rpqmatrix_optimizer,
    )?;
    eprintln!("  nodes:  {}", graph.num_nodes());
    eprintln!("  labels: {}", graph.num_labels());
    eprintln!();

    eprintln!("[2/2] Loading and running queries...");
    let queries = load_query_file(&common.queries, common.base_iri.as_deref())?;
    eprintln!("  loaded {} queries", queries.len());

    let results = dispatch_query(&args, &graph, &queries, model.as_ref())?;

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
                rpqmatrix_optimizer: Some(common.rpqmatrix_optimizer.to_string()),
                linear_model_coefficients: model.as_ref().map(|m| m.coefficients()),
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

fn build_checkpointer(
    args: &BenchArgs,
    queries_len: usize,
    model: Option<&LinearCostModel>,
) -> Result<Checkpointer, MainError> {
    let common = &args.common;
    let bench_config = BenchRunConfig::from_args(args, model);

    if let Some(checkpoint) = &args.checkpoint {
        let path = PathBuf::from(checkpoint);
        if args.resume {
            match Checkpoint::load(&path)? {
                Some(cp) => {
                    cp.validate(
                        &common.graph,
                        &common.queries,
                        &common.algo,
                        common.rpqmatrix_optimizer,
                        &bench_config,
                    )?;
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
                        common.rpqmatrix_optimizer,
                        bench_config,
                        Some(path),
                    ))
                }
            }
        } else {
            Ok(Checkpointer::fresh(
                &common.graph,
                &common.queries,
                &common.algo,
                common.rpqmatrix_optimizer,
                bench_config,
                Some(path),
            ))
        }
    } else {
        Ok(Checkpointer::fresh(
            &common.graph,
            &common.queries,
            &common.algo,
            common.rpqmatrix_optimizer,
            bench_config,
            None,
        ))
    }
}

fn run_bench_cmd(args: BenchArgs) -> Result<(), MainError> {
    let common = &args.common;
    validate_bench_args(&args)?;
    let model = load_linear_model(common)?;

    eprintln!("=== pathrex bench ===");
    eprintln!("Graph:      {}", common.graph);
    eprintln!("Format:     {}", common.format);
    eprintln!("Queries:    {}", common.queries);
    eprintln!("Algos:      {:?}", common.algo);
    eprintln!("Bench mode: {}", args.bench_mode);
    eprintln!("RPQMatrix optimizer: {}", common.rpqmatrix_optimizer);
    eprintln!("Output:     {}", args.output);
    eprintln!();

    eprintln!("[1/4] Loading graph...");
    let graph: InMemoryGraph = load_graph_with_optimizer(
        &common.graph,
        common.format,
        common.base_iri.as_deref(),
        common.rpqmatrix_optimizer,
    )?;
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
    let mut checkpointer = build_checkpointer(&args, queries.len(), model.as_ref())?;
    eprintln!();

    eprintln!("[4/4] Running benchmarks...");
    eprintln!();
    let results = dispatch_bench(&args, &graph, &queries, &mut checkpointer, model.as_ref())?;

    let output = BenchOutput {
        metadata: BenchMetadata {
            timestamp: Utc::now().to_rfc3339(),
            graph_path: common.graph.clone(),
            graph_format: common.format.to_string(),
            queries_file: common.queries.clone(),
            base_iri: common.base_iri.clone(),
            rpqmatrix_optimizer: Some(common.rpqmatrix_optimizer.to_string()),
            linear_model_coefficients: model.as_ref().map(|m| m.coefficients()),
            num_nodes: graph.num_nodes(),
            num_labels: graph.num_labels(),
            bench_mode: args.bench_mode.to_string(),
            runs: (args.bench_mode == BenchMode::Fixed).then(|| args.fixed_runs()),
            warm_up_runs: (args.bench_mode == BenchMode::Fixed).then(|| args.fixed_warm_up_runs()),
            sample_size: (args.bench_mode == BenchMode::Criterion)
                .then(|| args.criterion_sample_size()),
            warm_up_secs: (args.bench_mode == BenchMode::Criterion)
                .then(|| args.criterion_warm_up_secs()),
            measurement_secs: (args.bench_mode == BenchMode::Criterion)
                .then(|| args.criterion_measurement_secs()),
        },
        results,
    };

    output
        .write_to_file(Path::new(&args.output))
        .map_err(|e| MainError::Output {
            path: args.output.clone(),
            source: e,
        })?;
    let samples_path = output
        .write_samples_to_file(Path::new(&args.output))
        .map_err(|e| MainError::Output {
            path: args.output.clone(),
            source: e,
        })?;

    eprintln!();
    eprintln!("=== Done ===");
    eprintln!("Results written to: {}", args.output);
    if let Some(path) = samples_path {
        eprintln!("Run samples written to: {}", path.display());
    }
    if let Some(dir) = &args.criterion_dir {
        eprintln!("Criterion data in:  {dir}")
    }

    Ok(())
}

fn run_train_linear_cmd(args: TrainLinearArgs) -> Result<(), MainError> {
    let graph = load_graph_with_optimizer(
        &args.graph,
        GraphFormat::Mm,
        args.base_iri.as_deref(),
        RpqMatrixOptimizer::None,
    )?;
    let queries = load_query_file(&args.queries, args.base_iri.as_deref())?;
    let mut observations = Vec::new();
    for loaded in queries {
        let query = loaded.parsed.map_err(|source| MainError::Training {
            id: loaded.id.clone(),
            source,
        })?;
        let samples = RpqMatrixEvaluator::collect_linear_observations(
            &query,
            &graph,
            args.runs as usize,
            args.warm_up_runs as usize,
        )
        .map_err(|source| MainError::Training {
            id: loaded.id.clone(),
            source,
        })?;
        eprintln!("[train] query={} observations={}", loaded.id, samples.len());
        observations.extend(samples);
    }
    let model = LinearCostModel::fit(&observations)?;
    model.save(&args.output)?;
    eprintln!(
        "Fitted {} observations; coefficients={:?}",
        observations.len(),
        model.coefficients()
    );
    eprintln!("Frozen model written to {}", args.output);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_regression_requires_a_model_and_other_strategies_reject_it() {
        for (optimizer, path, valid) in [
            ("stochastic", None, false),
            ("stochastic", Some("model.txt"), true),
            ("hybrid", Some("model.txt"), false),
            ("none", Some("model.txt"), false),
            ("hybrid", None, true),
        ] {
            let mut arguments = vec![
                "pathrex",
                "query",
                "-g",
                "graph",
                "-q",
                "queries",
                "-a",
                "rpqmatrix",
                "-p",
                optimizer,
            ];
            if let Some(path) = path {
                arguments.extend(["--linear-model", path]);
            }
            let Commands::Query(args) = Cli::parse_from(arguments).command else {
                panic!("expected query");
            };
            assert_eq!(validate_common_args(&args.common).is_ok(), valid);
        }
    }
}
