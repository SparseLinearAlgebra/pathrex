//! Typed deserialization of criterion's per-benchmark JSON output.
//!
//! After `group.finish()`, criterion writes:
//!
//! ```text
//! <criterion_dir>/<group>/<bench>/new/estimates.json
//! <criterion_dir>/<group>/<bench>/new/sample.json
//! ```
//!
//! We read both to extract a [`TimingStats`].

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cli::bench::error::BenchError;
use crate::cli::output::{AlgoTiming, TimingStats};

#[derive(Deserialize)]
struct Estimates {
    mean: PointEstimate,
    median: PointEstimate,
    std_dev: PointEstimate,
}

#[derive(Deserialize)]
struct PointEstimate {
    point_estimate: f64,
}

#[derive(Deserialize)]
struct Sample {
    iters: Vec<f64>,
}

fn estimates_path(criterion_dir: &Path, group: &str, bench: &str) -> PathBuf {
    criterion_dir
        .join(group)
        .join(bench)
        .join("new")
        .join("estimates.json")
}

fn sample_path(criterion_dir: &Path, group: &str, bench: &str) -> PathBuf {
    criterion_dir
        .join(group)
        .join(bench)
        .join("new")
        .join("sample.json")
}

/// Read one bench's timing stats out of criterion's output directory.
///
/// Returns `Err(MissingEstimates)` if the JSON file isn't present (e.g. the
/// criterion run was interrupted) and `Err(EstimatesParse)` on shape
/// mismatches — making schema drift loud rather than silent.
pub fn read_timing_stats(
    criterion_dir: &Path,
    group: &str,
    bench: &str,
) -> Result<TimingStats, BenchError> {
    let est_path = estimates_path(criterion_dir, group, bench);
    let file = File::open(&est_path)
        .map_err(|_| BenchError::MissingEstimates(format!("{}/{}", group, bench)))?;
    let est: Estimates = serde_json::from_reader(file).map_err(|e| BenchError::EstimatesParse {
        group: format!("{}/{}", group, bench),
        source: e,
    })?;

    // sample.json is best-effort; if missing we report iterations=0.
    let iterations = File::open(sample_path(criterion_dir, group, bench))
        .ok()
        .and_then(|f| serde_json::from_reader::<_, Sample>(f).ok())
        .map(|s| s.iters.len())
        .unwrap_or(0);

    Ok(TimingStats {
        mean_ns: est.mean.point_estimate,
        median_ns: est.median.point_estimate,
        stddev_ns: est.std_dev.point_estimate,
        iterations,
    })
}

/// Read both `eval_total` and `eval_ffi_only` benches for a single group.
pub fn read_algo_timing(criterion_dir: &Path, group: &str) -> Result<AlgoTiming, BenchError> {
    let total = read_timing_stats(criterion_dir, group, "eval_total")?;
    let ffi_only = read_timing_stats(criterion_dir, group, "eval_ffi_only")?;
    Ok(AlgoTiming { total, ffi_only })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_estimate_files(base: &Path, group: &str, bench: &str, mean_ns: f64, iters: usize) {
        let bench_dir = base.join(group).join(bench).join("new");
        fs::create_dir_all(&bench_dir).expect("create bench dir");
        fs::write(
            bench_dir.join("estimates.json"),
            format!(
                r#"{{"mean":{{"point_estimate":{mean_ns}}},"median":{{"point_estimate":{mean_ns}}},"std_dev":{{"point_estimate":0.0}}}}"#
            ),
        )
        .expect("write estimates");
        let iters_array: Vec<&str> = vec!["1.0"; iters];
        let sample = format!("{{\"iters\":[{}]}}", iters_array.join(","));
        fs::write(bench_dir.join("sample.json"), sample).expect("write sample");
    }

    #[test]
    fn reads_split_estimates() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_estimate_files(dir.path(), "query_0_nfa", "eval_total", 10.0, 3);
        write_estimate_files(dir.path(), "query_0_nfa", "eval_ffi_only", 4.0, 5);

        let timing = read_algo_timing(dir.path(), "query_0_nfa").expect("split timing");
        assert_eq!(timing.total.mean_ns, 10.0);
        assert_eq!(timing.total.iterations, 3);
        assert_eq!(timing.ffi_only.mean_ns, 4.0);
        assert_eq!(timing.ffi_only.iterations, 5);
    }

    #[test]
    fn missing_file_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = read_timing_stats(dir.path(), "missing", "eval_total").unwrap_err();
        match err {
            BenchError::MissingEstimates(g) => assert!(g.contains("missing")),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn malformed_json_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bench_dir = dir.path().join("g").join("b").join("new");
        fs::create_dir_all(&bench_dir).unwrap();
        fs::write(bench_dir.join("estimates.json"), "{not json").unwrap();

        let err = read_timing_stats(dir.path(), "g", "b").unwrap_err();
        match err {
            BenchError::EstimatesParse { .. } => {}
            other => panic!("unexpected error: {other}"),
        }
    }
}
