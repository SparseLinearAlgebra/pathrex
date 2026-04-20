//! JSON output types and serialization for benchmark and query results.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

// ── Shared types ─────────────────────────────────────────────────────────────

/// Result of running a single algorithm on a single query.
#[derive(Debug, Serialize)]
pub struct AlgoResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Result count (nnz / number of reachable nodes). Present in both
    /// `query` and `bench` modes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
    /// Timing statistics — only present in `bench` mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<TimingStats>,
}

impl AlgoResult {
    /// Create a successful result with an optional result count and timing.
    pub fn ok(result_count: Option<usize>, timing: Option<TimingStats>) -> Self {
        Self {
            status: "ok".to_string(),
            error: None,
            result_count,
            timing,
        }
    }

    /// Create an error result.
    pub fn error(message: String) -> Self {
        Self {
            status: "error".to_string(),
            error: Some(message),
            result_count: None,
            timing: None,
        }
    }

    /// Create a panic result.
    pub fn panic(message: String) -> Self {
        Self {
            status: "panic".to_string(),
            error: Some(message),
            result_count: None,
            timing: None,
        }
    }
}

/// Timing statistics extracted from criterion estimates.
#[derive(Debug, Serialize)]
pub struct TimingStats {
    pub mean_ns: f64,
    pub median_ns: f64,
    pub stddev_ns: f64,
    pub iterations: usize,
}

/// Results for a single query across all algorithms.
#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub query_index: usize,
    pub query_id: String,
    pub query_text: String,
    pub algorithms: HashMap<String, AlgoResult>,
}

// ── Query output ─────────────────────────────────────────────────────────────

/// Top-level JSON output for the `query` subcommand.
#[derive(Debug, Serialize)]
pub struct QueryOutput {
    pub metadata: QueryMetadata,
    pub results: Vec<QueryResult>,
}

/// Metadata for a `query` run (no criterion parameters).
#[derive(Debug, Serialize)]
pub struct QueryMetadata {
    pub timestamp: String,
    pub graph_path: String,
    pub graph_format: String,
    pub queries_file: String,
    pub base_iri: String,
    pub num_nodes: usize,
    pub num_labels: usize,
}

impl QueryOutput {
    /// Write the output to a JSON file.
    pub fn write_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)
    }
}

// ── Bench output ──────────────────────────────────────────────────────────────

/// Top-level JSON output for the `bench` subcommand.
#[derive(Debug, Serialize)]
pub struct BenchOutput {
    pub metadata: BenchMetadata,
    pub results: Vec<BatchResult>,
}

/// Metadata for a `bench` run (includes criterion parameters).
#[derive(Debug, Serialize)]
pub struct BenchMetadata {
    pub timestamp: String,
    pub graph_path: String,
    pub graph_format: String,
    pub queries_file: String,
    pub base_iri: String,
    pub num_nodes: usize,
    pub num_labels: usize,
    pub sample_size: usize,
    pub warm_up_secs: u64,
    pub measurement_secs: u64,
    pub batch_size: usize,
}

/// Results for a batch of queries.
#[derive(Debug, Serialize)]
pub struct BatchResult {
    /// Zero-based batch index.
    pub batch_index: usize,
    /// Query indices included in this batch.
    pub query_indices: Vec<usize>,
    /// Per-query results within this batch.
    pub queries: Vec<QueryResult>,
}

impl BenchOutput {
    /// Write the output to a JSON file.
    pub fn write_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)
    }
}
