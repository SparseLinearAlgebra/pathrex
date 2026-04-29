//! Checkpoint read/write/validation for crash recovery.
//!
//! After each query-algorithm pair completes, the checkpoint file is updated
//! so that a crashed run can be resumed from the last completed point.
//!
//! Two layers:
//! - [`Checkpoint`] — pure data; serialised to disk.
//! - [`Checkpointer`] — runtime owner; pairs the data with its file path
//!   and exposes a fallible `mark_and_save`. Errors propagate; no silent
//!   corruption.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::args::Algo;

/// Persistent checkpoint state written to disk as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    pub graph_path: String,
    pub queries_file: String,
    pub algorithms: Vec<Algo>,
    pub completed: Vec<QueryCompletion>,
}

/// Tracks which algorithms have been completed for a single query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCompletion {
    pub query_index: usize,
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

    /// Save the checkpoint to disk.
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

    pub fn is_algo_done(&self, query_index: usize, algo: &Algo) -> bool {
        self.completed
            .iter()
            .find(|c| c.query_index == query_index)
            .map(|c| c.algorithms_done.contains(algo))
            .unwrap_or(false)
    }

    pub fn mark_algo_done(&mut self, query_index: usize, algo: &Algo) {
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
                algorithms_done: vec![algo.clone()],
            });
        }
    }
}

/// Runtime owner for a [`Checkpoint`] paired with its on-disk path.
pub struct Checkpointer {
    inner: Checkpoint,
    path: PathBuf,
}

impl Checkpointer {
    /// Create a new checkpointer with no completions.
    pub fn fresh(graph_path: &str, queries_file: &str, algorithms: &[Algo], path: PathBuf) -> Self {
        Self {
            inner: Checkpoint::new(graph_path, queries_file, algorithms),
            path,
        }
    }

    /// Wrap an existing [`Checkpoint`] (e.g. one loaded from disk).
    pub fn with_inner(inner: Checkpoint, path: PathBuf) -> Self {
        Self { inner, path }
    }

    /// Number of queries that have *all* requested algorithms done.
    pub fn fully_done_count(&self, algos: &[Algo]) -> usize {
        self.inner
            .completed
            .iter()
            .filter(|c| {
                let done: HashSet<&Algo> = c.algorithms_done.iter().collect();
                algos.iter().all(|a| done.contains(a))
            })
            .count()
    }

    pub fn is_fully_done(&self, query_index: usize, algos: &[Algo]) -> bool {
        self.inner.is_fully_done(query_index, algos)
    }

    pub fn is_algo_done(&self, query_index: usize, algo: &Algo) -> bool {
        self.inner.is_algo_done(query_index, algo)
    }

    /// Mark `(query_index, algo)` complete and persist atomically.
    pub fn mark_and_save(
        &mut self,
        query_index: usize,
        algo: &Algo,
    ) -> Result<(), CheckpointError> {
        self.inner.mark_algo_done(query_index, algo);
        self.inner.save(&self.path)
    }
}

/// Errors that can occur during checkpoint operations.
#[derive(Debug, Error)]
pub enum CheckpointError {
    /// I/O error reading or writing the checkpoint file.
    #[error("checkpoint I/O error ({0}): {1}")]
    Io(String, std::io::Error),
    /// JSON parsing error.
    #[error("checkpoint parse error ({0}): {1}")]
    Parse(String, serde_json::Error),
    /// JSON serialization error.
    #[error("checkpoint serialize error: {0}")]
    Serialize(serde_json::Error),
    /// Checkpoint parameters don't match current run.
    #[error("checkpoint mismatch: {0}")]
    Mismatch(String),
}
