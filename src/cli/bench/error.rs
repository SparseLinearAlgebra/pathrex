//! Error type for the bench pipeline.

use thiserror::Error;

use crate::cli::checkpoint::CheckpointError;

#[derive(Debug, Error)]
pub enum BenchError {
    #[error("criterion estimates missing for group '{0}' (file not found or unreadable)")]
    MissingEstimates(String),

    #[error("criterion estimates parse error for group '{group}': {source}")]
    EstimatesParse {
        group: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("checkpoint error: {0}")]
    Checkpoint(#[from] CheckpointError),

    #[error("invalid bench arguments: {0}")]
    InvalidArgs(String),

    #[error("failed to create temporary directory for criterion output: {0}")]
    TempDir(#[source] std::io::Error),
}
