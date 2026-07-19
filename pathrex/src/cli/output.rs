//! JSON output types and serialization for benchmark and query results.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    #[serde(skip)]
    pub samples: Option<AlgoTimingSamples>,
}

#[derive(Debug, Serialize)]
pub struct AlgoTimingSamples {
    pub total_ns: Vec<f64>,
    pub ffi_only_ns: Vec<f64>,
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
    pub rpqmatrix_optimizer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
    pub num_nodes: usize,
    pub num_labels: usize,
}

impl QueryOutput {
    pub fn write_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_json_to_file(path, json)
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
    pub rpqmatrix_optimizer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
    pub num_nodes: usize,
    pub num_labels: usize,
    pub bench_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warm_up_runs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warm_up_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurement_secs: Option<u64>,
}

impl BenchOutput {
    pub fn write_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_json_to_file(path, json)
    }

    pub fn write_samples_to_file(&self, path: &Path) -> Result<Option<PathBuf>, std::io::Error> {
        let Some(samples) = BenchSamplesOutput::from_bench_output(self) else {
            return Ok(None);
        };
        let samples_path = samples_path_for(path);
        let json = serde_json::to_string_pretty(&samples).map_err(std::io::Error::other)?;
        write_json_to_file(&samples_path, json)?;
        Ok(Some(samples_path))
    }
}

#[derive(Debug, Serialize)]
pub struct BenchSamplesOutput<'a> {
    pub metadata: &'a BenchMetadata,
    pub results: Vec<QuerySamples<'a>>,
}

#[derive(Debug, Serialize)]
pub struct QuerySamples<'a> {
    pub query_index: usize,
    pub query_id: &'a str,
    pub query_text: &'a str,
    pub algorithms: HashMap<&'a str, AlgoSamples<'a>>,
}

#[derive(Debug, Serialize)]
pub struct AlgoSamples<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
    pub total_ns: &'a [f64],
    pub ffi_only_ns: &'a [f64],
}

impl<'a> BenchSamplesOutput<'a> {
    pub fn from_bench_output(output: &'a BenchOutput) -> Option<Self> {
        let mut results = Vec::new();

        for query in &output.results {
            let mut algorithms = HashMap::new();
            for (algo, result) in &query.algorithms {
                let Some(timing) = &result.timing else {
                    continue;
                };
                let Some(samples) = &timing.samples else {
                    continue;
                };
                algorithms.insert(
                    algo.as_str(),
                    AlgoSamples {
                        result_count: result.result_count,
                        total_ns: &samples.total_ns,
                        ffi_only_ns: &samples.ffi_only_ns,
                    },
                );
            }

            if !algorithms.is_empty() {
                results.push(QuerySamples {
                    query_index: query.query_index,
                    query_id: &query.query_id,
                    query_text: &query.query_text,
                    algorithms,
                });
            }
        }

        (!results.is_empty()).then_some(Self {
            metadata: &output.metadata,
            results,
        })
    }
}

fn samples_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bench_results.json");
    let samples_name = file_name
        .strip_suffix(".json")
        .map(|stem| format!("{stem}.runs.json"))
        .unwrap_or_else(|| format!("{file_name}.runs.json"));

    path.with_file_name(samples_name)
}

fn write_json_to_file(path: &Path, json: String) -> Result<(), std::io::Error> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json)
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
                samples: None,
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

    #[test]
    fn query_output_creates_parent_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output_path = dir.path().join("nested").join("query.json");
        let output = QueryOutput {
            metadata: QueryMetadata {
                timestamp: "now".into(),
                graph_path: "graph".into(),
                graph_format: "mm".into(),
                queries_file: "queries".into(),
                rpqmatrix_optimizer: Some("none".into()),
                base_iri: None,
                num_nodes: 0,
                num_labels: 0,
            },
            results: Vec::new(),
        };

        output.write_to_file(&output_path).expect("write output");

        assert!(output_path.exists());
    }

    #[test]
    fn bench_output_writes_samples_next_to_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output_path = dir.path().join("bench.json");
        let output = BenchOutput {
            metadata: BenchMetadata {
                timestamp: "now".into(),
                graph_path: "graph".into(),
                graph_format: "mm".into(),
                queries_file: "queries".into(),
                rpqmatrix_optimizer: Some("none".into()),
                base_iri: None,
                num_nodes: 0,
                num_labels: 0,
                bench_mode: "fixed".into(),
                runs: Some(2),
                warm_up_runs: Some(0),
                sample_size: None,
                warm_up_secs: None,
                measurement_secs: None,
            },
            results: vec![QueryResult {
                query_index: 0,
                query_id: "q0".into(),
                query_text: "query".into(),
                algorithms: HashMap::from([(
                    "rpqmatrix".into(),
                    AlgoResult::ok(
                        Some(1),
                        Some(AlgoTiming {
                            total: TimingStats {
                                mean_ns: 15.0,
                                median_ns: 15.0,
                                stddev_ns: 5.0,
                                iterations: 2,
                            },
                            ffi_only: TimingStats {
                                mean_ns: 4.0,
                                median_ns: 4.0,
                                stddev_ns: 1.0,
                                iterations: 2,
                            },
                            samples: Some(AlgoTimingSamples {
                                total_ns: vec![10.0, 20.0],
                                ffi_only_ns: vec![3.0, 5.0],
                            }),
                        }),
                    ),
                )]),
            }],
        };

        let samples_path = output
            .write_samples_to_file(&output_path)
            .expect("write samples")
            .expect("samples path");
        let samples_json = fs::read_to_string(samples_path).expect("read samples");
        let value: serde_json::Value = serde_json::from_str(&samples_json).expect("json");

        assert_eq!(
            value["results"][0]["algorithms"]["rpqmatrix"]["total_ns"][0],
            10.0
        );
        assert_eq!(
            value["results"][0]["algorithms"]["rpqmatrix"]["ffi_only_ns"][1],
            5.0
        );
    }
}
