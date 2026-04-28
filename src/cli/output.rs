//! JSON output types and serialization for benchmark and query results.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

/// Outcome of running a single algorithm on a single query.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlgoStatus {
    Ok,
    Error,
    Panic,
}

/// Result of running a single algorithm on a single query.
#[derive(Debug, Serialize)]
pub struct AlgoResult {
    pub status: AlgoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<AlgoTiming>,
}

impl AlgoResult {
    pub fn ok(result_count: Option<usize>, timing: Option<AlgoTiming>) -> Self {
        Self {
            status: AlgoStatus::Ok,
            error: None,
            result_count,
            timing,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            status: AlgoStatus::Error,
            error: Some(message),
            result_count: None,
            timing: None,
        }
    }

    pub fn panic(message: String) -> Self {
        Self {
            status: AlgoStatus::Panic,
            error: Some(message),
            result_count: None,
            timing: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AlgoTiming {
    pub total: TimingStats,
    pub ffi_only: TimingStats,
}

/// Timing statistics extracted from criterion estimates.
#[derive(Debug, Serialize)]
pub struct TimingStats {
    pub mean_ns: f64,
    pub median_ns: f64,
    pub stddev_ns: f64,
    pub iterations: usize,
}

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub query_index: usize,
    pub query_id: String,
    pub query_text: String,
    pub algorithms: HashMap<String, AlgoResult>,
}

#[derive(Debug, Serialize)]
pub struct QueryOutput {
    pub metadata: QueryMetadata,
    pub results: Vec<QueryResult>,
}

#[derive(Debug, Serialize)]
pub struct QueryMetadata {
    pub timestamp: String,
    pub graph_path: String,
    pub graph_format: String,
    pub queries_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
    pub num_nodes: usize,
    pub num_labels: usize,
}

impl QueryOutput {
    pub fn write_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)
    }
}

#[derive(Debug, Serialize)]
pub struct BenchOutput {
    pub metadata: BenchMetadata,
    pub results: Vec<QueryResult>,
}

#[derive(Debug, Serialize)]
pub struct BenchMetadata {
    pub timestamp: String,
    pub graph_path: String,
    pub graph_format: String,
    pub queries_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
    pub num_nodes: usize,
    pub num_labels: usize,
    pub sample_size: usize,
    pub warm_up_secs: u64,
    pub measurement_secs: u64,
}

impl BenchOutput {
    pub fn write_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn algo_result_serializes_split_timing() {
        let result = AlgoResult::ok(
            Some(3),
            Some(AlgoTiming {
                total: TimingStats {
                    mean_ns: 1.0,
                    median_ns: 1.0,
                    stddev_ns: 0.0,
                    iterations: 10,
                },
                ffi_only: TimingStats {
                    mean_ns: 0.5,
                    median_ns: 0.5,
                    stddev_ns: 0.0,
                    iterations: 10,
                },
            }),
        );

        let value = serde_json::to_value(&result).expect("serialize");
        assert!(value["timing"]["total"].is_object());
        assert!(value["timing"]["ffi_only"].is_object());
        assert_eq!(value["status"], "ok");
    }

    #[test]
    fn error_status_serializes_lowercase() {
        let r = AlgoResult::error("boom".into());
        let v = serde_json::to_value(&r).expect("serialize");
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], "boom");
    }

    #[test]
    fn panic_status_serializes_lowercase() {
        let r = AlgoResult::panic("kaboom".into());
        let v = serde_json::to_value(&r).expect("serialize");
        assert_eq!(v["status"], "panic");
    }
}
