//! CLI argument definitions for the `pathrex` binary.
//!
//! Structure:
//! - [`Cli`] — top-level parser with a `subcommand` field
//! - [`Commands`] — `bench` or `query`
//! - [`CommonArgs`] — args shared by both subcommands (graph, queries, algo, …)
//! - [`BenchArgs`] — bench-specific args (criterion, checkpoint, …)
//! - [`QueryArgs`] — query-specific args (optional output file)
//! - [`Algo`] — algorithm identifier enum

use clap::{Args, Parser, Subcommand};

/// Top-level CLI for pathrex.
#[derive(Parser, Debug)]
#[command(
    name = "pathrex",
    about = "RPQ evaluator and benchmarking tool for edge-labeled graphs"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run queries once and report result counts
    Query(QueryArgs),
    /// Benchmark RPQ evaluators with criterion
    Bench(BenchArgs),
}

/// Arguments shared by both subcommands.
#[derive(Args, Debug)]
pub struct CommonArgs {
    /// Path to graph directory (mm) or file (csv).
    #[arg(short = 'g', long)]
    pub graph: String,

    /// Graph format: mm | csv
    #[arg(short = 'f', long, default_value = "mm")]
    pub format: String,

    /// Path to queries file (format: `<id>,<sparql_pattern>` per line).
    #[arg(short = 'q', long)]
    pub queries: String,

    /// Base IRI used when wrapping bare SPARQL patterns.
    #[arg(short = 'b', long, default_value = "http://example.org/")]
    pub base_iri: String,

    /// Algorithms to use.
    #[arg(short = 'a', long, num_args = 1.., default_values_t = vec![Algo::Nfa, Algo::Rpqmatrix])]
    pub algo: Vec<Algo>,
}

/// Arguments for the `query` subcommand.
#[derive(Args, Debug)]
pub struct QueryArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Optional path to write results as JSON.
    #[arg(short = 'o', long)]
    pub output: Option<String>,
}

/// Arguments for the `bench` subcommand.
#[derive(Args, Debug)]
pub struct BenchArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Output JSON file for benchmark results.
    #[arg(short = 'o', long, default_value = "bench_results.json")]
    pub output: String,

    /// Checkpoint file path.
    #[arg(short = 'c', long, default_value = "bench_checkpoint.json")]
    pub checkpoint: String,

    /// Resume from checkpoint, skipping completed queries.
    #[arg(long)]
    pub resume: bool,

    /// Number of queries per batch. Controls how often results are logged
    /// and checkpoints are saved. Default is 1 (checkpoint after every query).
    #[arg(long, default_value_t = 1)]
    pub batch_size: usize,

    /// Directory for criterion output.
    #[arg(long, default_value = "bench_criterion/")]
    pub criterion_dir: String,

    /// Enable criterion HTML plot generation.
    #[arg(long)]
    pub plots: bool,

    /// Criterion sample size per benchmark group.
    #[arg(long, default_value_t = 10)]
    pub sample_size: usize,

    /// Criterion warm-up time in seconds.
    #[arg(long, default_value_t = 1)]
    pub warm_up: u64,

    /// Criterion measurement time in seconds.
    #[arg(long, default_value_t = 5)]
    pub measurement: u64,
}

/// Algorithm identifiers for RPQ evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Algo {
    /// NFA-based evaluator (`LAGraph_RegularPathQuery`).
    Nfa,
    /// Matrix-plan evaluator (`LAGraph_RPQMatrix`).
    Rpqmatrix,
}

impl std::fmt::Display for Algo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Algo::Nfa => write!(f, "nfa"),
            Algo::Rpqmatrix => write!(f, "rpqmatrix"),
        }
    }
}

impl std::str::FromStr for Algo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nfa" => Ok(Algo::Nfa),
            "rpqmatrix" => Ok(Algo::Rpqmatrix),
            other => Err(format!(
                "unknown algorithm: '{other}' (expected: nfa, rpqmatrix)"
            )),
        }
    }
}
