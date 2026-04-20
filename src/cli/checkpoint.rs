//! Checkpoint read/write/validation for crash recovery.
//!
//! After each query-algorithm pair completes, the checkpoint file is updated
//! so that a crashed run can be resumed from the last completed point.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::args::Algo;

/// Persistent checkpoint state written to disk as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Schema version (always 1 for now).
    pub version: u32,
    /// The graph path used for this benchmark run.
    pub graph_path: String,
    /// The queries file used for this benchmark run.
    pub queries_file: String,
    /// The algorithms requested for this benchmark run.
    pub algorithms: Vec<Algo>,
    /// Per-query completion records.
    pub completed: Vec<QueryCompletion>,
}

/// Tracks which algorithms have been completed for a single query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCompletion {
    /// Zero-based index of the query in the queries file.
    pub query_index: usize,
    /// The query ID from the file (the number before the comma).
    pub query_id: String,
    /// Which algorithms have finished for this query.
    pub algorithms_done: Vec<Algo>,
}

impl Checkpoint {
    /// Create a fresh checkpoint for a new benchmark run.
    pub fn new(graph_path: &str, queries_file: &str, algorithms: &[Algo]) -> Self {
        Self {
            version: 1,
            graph_path: graph_path.to_string(),
            queries_file: queries_file.to_string(),
            algorithms: algorithms.to_vec(),
            completed: Vec::new(),
        }
    }

    /// Load a checkpoint from disk. Returns `None` if the file doesn't exist.
    pub fn load(path: &Path) -> Result<Option<Self>, CheckpointError> {
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(path)
            .map_err(|e| CheckpointError::Io(path.display().to_string(), e))?;
        let cp: Self = serde_json::from_str(&data)
            .map_err(|e| CheckpointError::Parse(path.display().to_string(), e))?;
        Ok(Some(cp))
    }

    /// Validate that a loaded checkpoint matches the current run parameters.
    pub fn validate(
        &self,
        graph_path: &str,
        queries_file: &str,
        algorithms: &[Algo],
    ) -> Result<(), CheckpointError> {
        if self.graph_path != graph_path {
            return Err(CheckpointError::Mismatch(format!(
                "graph_path: checkpoint has '{}', current is '{}'",
                self.graph_path, graph_path
            )));
        }
        if self.queries_file != queries_file {
            return Err(CheckpointError::Mismatch(format!(
                "queries_file: checkpoint has '{}', current is '{}'",
                self.queries_file, queries_file
            )));
        }
        let cp_algos: HashSet<&Algo> = self.algorithms.iter().collect();
        let cur_algos: HashSet<&Algo> = algorithms.iter().collect();
        if cp_algos != cur_algos {
            return Err(CheckpointError::Mismatch(format!(
                "algorithms: checkpoint has {:?}, current is {:?}",
                self.algorithms, algorithms
            )));
        }
        Ok(())
    }

    /// Save the checkpoint to disk (atomic write via temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), CheckpointError> {
        let json = serde_json::to_string_pretty(self).map_err(CheckpointError::Serialize)?;

        // Write to a temp file first, then rename for atomicity.
        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, &json)
            .map_err(|e| CheckpointError::Io(tmp_path.display().to_string(), e))?;
        fs::rename(&tmp_path, path)
            .map_err(|e| CheckpointError::Io(path.display().to_string(), e))?;
        Ok(())
    }

    /// Check if all requested algorithms are done for a given query index.
    pub fn is_fully_done(&self, query_index: usize, algos: &[Algo]) -> bool {
        let Some(entry) = self.completed.iter().find(|c| c.query_index == query_index) else {
            return false;
        };
        let done: HashSet<&Algo> = entry.algorithms_done.iter().collect();
        algos.iter().all(|a| done.contains(a))
    }

    /// Check if a specific algorithm is done for a given query index.
    pub fn is_algo_done(&self, query_index: usize, algo: &Algo) -> bool {
        self.completed
            .iter()
            .find(|c| c.query_index == query_index)
            .map(|c| c.algorithms_done.contains(algo))
            .unwrap_or(false)
    }

    /// Mark an algorithm as completed for a given query.
    pub fn mark_algo_done(&mut self, query_index: usize, query_id: &str, algo: &Algo) {
        if let Some(entry) = self
            .completed
            .iter_mut()
            .find(|c| c.query_index == query_index)
        {
            if !entry.algorithms_done.contains(algo) {
                entry.algorithms_done.push(algo.clone());
            }
        } else {
            self.completed.push(QueryCompletion {
                query_index,
                query_id: query_id.to_string(),
                algorithms_done: vec![algo.clone()],
            });
        }
    }
}

/// Errors that can occur during checkpoint operations.
#[derive(Debug)]
pub enum CheckpointError {
    /// I/O error reading or writing the checkpoint file.
    Io(String, std::io::Error),
    /// JSON parsing error.
    Parse(String, serde_json::Error),
    /// JSON serialization error.
    Serialize(serde_json::Error),
    /// Checkpoint parameters don't match current run.
    Mismatch(String),
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointError::Io(path, e) => write!(f, "checkpoint I/O error ({path}): {e}"),
            CheckpointError::Parse(path, e) => write!(f, "checkpoint parse error ({path}): {e}"),
            CheckpointError::Serialize(e) => write!(f, "checkpoint serialize error: {e}"),
            CheckpointError::Mismatch(msg) => write!(f, "checkpoint mismatch: {msg}"),
        }
    }
}

impl std::error::Error for CheckpointError {}
