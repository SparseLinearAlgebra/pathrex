//! CLI argument definitions for the `pathrex` binary.
//!
//! Structure:
//! - [`Cli`] — top-level parser with a `subcommand` field
//! - [`Commands`] — `bench` or `query`
//! - [`CommonArgs`] — args shared by both subcommands (graph, queries, algo, …)
//! - [`BenchArgs`] — bench-specific args (criterion, checkpoint, …)
//! - [`QueryArgs`] — query-specific args (optional output file)
//! - [`Algo`] — algorithm identifier enum
//! - [`GraphFormat`] — input graph format enum

use clap::{Args, Parser, Subcommand, ValueEnum};

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

    /// Graph format.
    #[arg(short = 'f', long, value_enum, default_value_t = GraphFormat::Mm)]
    pub format: GraphFormat,

    /// Path to queries file (format: `<id>,<sparql_pattern>` per line).
    #[arg(short = 'q', long)]
    pub queries: String,

    /// Optional base IRI prepended to bare SPARQL patterns as `BASE <iri>`.
    /// Pass without a value (`--base-iri`) to use the default `http://example.org/`.
    /// Pass with a value (`--base-iri <iri>`) to use a custom IRI.
    /// When omitted entirely, no BASE declaration is added to the query.
    #[arg(
        short = 'b',
        long,
        num_args = 0..=1,
        default_missing_value = "http://example.org/",
        require_equals = false
    )]
    pub base_iri: Option<String>,

    /// Algorithms to use.
    #[arg(short = 'a', long, value_enum, num_args = 1.., required = true)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum Algo {
    /// NFA-based evaluator (`LAGraph_RegularPathQuery`).
    NfaRpq,
    /// Matrix-plan evaluator (`LAGraph_RPQMatrix`).
    Rpqmatrix,
}

impl std::fmt::Display for Algo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Algo::NfaRpq => write!(f, "nfarpq"),
            Algo::Rpqmatrix => write!(f, "rpqmatrix"),
        }
    }
}

/// Input graph format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum GraphFormat {
    /// MatrixMarket directory layout (vertices.txt, edges.txt, *.txt).
    Mm,
    /// CSV file with source/target/label columns.
    Csv,
}

impl std::fmt::Display for GraphFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphFormat::Mm => write!(f, "mm"),
            GraphFormat::Csv => write!(f, "csv"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn query_requires_at_least_one_algo() {
        let result = Cli::try_parse_from([
            "pathrex",
            "query",
            "--graph",
            "graph",
            "--queries",
            "queries",
        ]);

        assert!(result.is_err());
    }
}
