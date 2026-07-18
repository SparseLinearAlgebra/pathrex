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

use crate::rpq::rpqmatrix::OptimizationStrategy;

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
    /// Benchmark RPQ evaluators
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

    /// Optimizer type (only for the RPQMatrix algorithm and MatrixMarket source graph).
    #[arg(
        short = 'p',
        long = "rpqmatrix-optimizer",
        value_enum,
        default_value_t = RpqMatrixOptimizer::None
    )]
    pub rpqmatrix_optimizer: RpqMatrixOptimizer,
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

    /// Optional checkpoint file path.
    #[arg(short = 'c', long)]
    pub checkpoint: Option<String>,

    /// Resume from checkpoint, skipping completed queries.
    #[arg(long)]
    pub resume: bool,

    /// Benchmarking mode.
    #[arg(long, value_enum, default_value_t = BenchMode::Fixed)]
    pub bench_mode: BenchMode,

    /// Directory for criterion output. When omitted, criterion writes into a
    /// per-group temporary directory that is wiped immediately after each
    /// benchmark group is parsed (default behavior).
    #[arg(long)]
    pub criterion_dir: Option<String>,

    /// Enable criterion HTML plot generation. Requires `--criterion-dir`,
    /// since plots written to a tempdir would be wiped before they could be
    /// inspected.
    #[arg(long)]
    pub plots: bool,

    /// Criterion sample size per benchmark group.
    #[arg(long)]
    pub sample_size: Option<usize>,

    /// Criterion warm-up time in seconds.
    #[arg(long)]
    pub warm_up: Option<u64>,

    /// Criterion measurement time in seconds.
    #[arg(long)]
    pub measurement: Option<u64>,

    /// Number of warm-up runs.
    #[arg(long = "warm-up-runs")]
    pub warm_up_runs: Option<u64>,

    /// Number of measured runs.
    #[arg(long)]
    pub runs: Option<u64>,
}

impl BenchArgs {
    pub const DEFAULT_FIXED_RUNS: u64 = 10;
    pub const DEFAULT_FIXED_WARM_UP_RUNS: u64 = 0;
    pub const DEFAULT_CRITERION_SAMPLE_SIZE: usize = 10;
    pub const DEFAULT_CRITERION_WARM_UP_SECS: u64 = 1;
    pub const DEFAULT_CRITERION_MEASUREMENT_SECS: u64 = 5;

    pub fn fixed_runs(&self) -> u64 {
        self.runs.unwrap_or(Self::DEFAULT_FIXED_RUNS)
    }

    pub fn fixed_warm_up_runs(&self) -> u64 {
        self.warm_up_runs
            .unwrap_or(Self::DEFAULT_FIXED_WARM_UP_RUNS)
    }

    pub fn criterion_sample_size(&self) -> usize {
        self.sample_size
            .unwrap_or(Self::DEFAULT_CRITERION_SAMPLE_SIZE)
    }

    pub fn criterion_warm_up_secs(&self) -> u64 {
        self.warm_up.unwrap_or(Self::DEFAULT_CRITERION_WARM_UP_SECS)
    }

    pub fn criterion_measurement_secs(&self) -> u64 {
        self.measurement
            .unwrap_or(Self::DEFAULT_CRITERION_MEASUREMENT_SECS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum BenchMode {
    /// Fixed number of runs per query.
    Fixed,
    /// Criterion time-based benchmark.
    Criterion,
}

impl Default for BenchMode {
    fn default() -> Self {
        Self::Fixed
    }
}

impl std::fmt::Display for BenchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchMode::Fixed => write!(f, "fixed"),
            BenchMode::Criterion => write!(f, "criterion"),
        }
    }
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
    Mm,
    Csv,
    Rdf,
}

impl std::fmt::Display for GraphFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphFormat::Mm => write!(f, "mm"),
            GraphFormat::Csv => write!(f, "csv"),
            GraphFormat::Rdf => write!(f, "rdf"),
        }
    }
}

/// Optimizer types.
/// Only for RPQMatrix algorithm and MatrixMarket source graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[value(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RpqMatrixOptimizer {
    /// Optimizer based on cardinality of source matrices.
    Cardinality,
    /// Without any optimizations.
    None,
}

impl Default for RpqMatrixOptimizer {
    fn default() -> Self {
        Self::None
    }
}

impl std::fmt::Display for RpqMatrixOptimizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpqMatrixOptimizer::Cardinality => write!(f, "cardinality"),
            RpqMatrixOptimizer::None => write!(f, "none"),
        }
    }
}

impl From<RpqMatrixOptimizer> for OptimizationStrategy {
    fn from(value: RpqMatrixOptimizer) -> Self {
        match value {
            RpqMatrixOptimizer::None => OptimizationStrategy::NoOpt,
            RpqMatrixOptimizer::Cardinality => OptimizationStrategy::Cardinality,
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

    #[test]
    fn bench_defaults_to_fixed_runs_without_checkpoint_or_criterion() {
        let cli = Cli::parse_from([
            "pathrex",
            "bench",
            "--graph",
            "graph",
            "--queries",
            "queries",
            "--algo",
            "rpqmatrix",
        ]);

        let Commands::Bench(args) = cli.command else {
            panic!("expected bench command");
        };

        assert_eq!(args.bench_mode, BenchMode::Fixed);
        assert_eq!(args.fixed_runs(), BenchArgs::DEFAULT_FIXED_RUNS);
        assert_eq!(
            args.fixed_warm_up_runs(),
            BenchArgs::DEFAULT_FIXED_WARM_UP_RUNS
        );
        assert!(args.checkpoint.is_none());
        assert!(args.criterion_dir.is_none());
        assert!(args.sample_size.is_none());
        assert!(args.warm_up.is_none());
        assert!(args.measurement.is_none());
    }

    #[test]
    fn criterion_mode_accepts_optional_criterion_settings() {
        let cli = Cli::parse_from([
            "pathrex",
            "bench",
            "--graph",
            "graph",
            "--queries",
            "queries",
            "--algo",
            "rpqmatrix",
            "--bench-mode",
            "criterion",
            "--sample-size",
            "20",
            "--warm-up",
            "2",
            "--measurement",
            "7",
        ]);

        let Commands::Bench(args) = cli.command else {
            panic!("expected bench command");
        };

        assert_eq!(args.bench_mode, BenchMode::Criterion);
        assert_eq!(args.criterion_sample_size(), 20);
        assert_eq!(args.criterion_warm_up_secs(), 2);
        assert_eq!(args.criterion_measurement_secs(), 7);
    }
}
