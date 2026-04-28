//! CLI layer for the `pathrex` binary.
//!
//! This module is only compiled when the `bench` feature is enabled.
//! It provides the argument definitions, graph/query loading, and the two
//! subcommand runners:
//!
//! - `bench` — criterion-based benchmarking with checkpointing
//! - `query` — single-shot query execution with result counts

pub mod args;
pub mod bench;
pub mod checkpoint;
pub mod dispatch;
pub mod loader;
pub mod output;
pub mod query;
