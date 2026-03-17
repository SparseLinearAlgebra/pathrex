//! Data format parsers for pathrex.
//!
//! # Quick-start examples
//!
//! ```no_run
//! use pathrex::graph::{Graph, InMemory, GraphDecomposition};
//! use pathrex::formats::Csv;
//! use std::fs::File;
//!
//! // Build from CSV in one line
//! let g = Graph::<InMemory>::try_from(
//!     Csv::from_reader(File::open("edges.csv").unwrap()).unwrap()
//! ).unwrap();
//! ```

pub mod csv;

pub use csv::Csv;

use thiserror::Error;

/// Unified error type for all format parsing operations.
#[derive(Error, Debug)]
pub enum FormatError {
    /// An error produced by the `csv` crate during parsing.
    #[error("CSV error: {0}")]
    Csv(#[from] ::csv::Error),

    /// A required column was not found in the CSV header row.
    #[error("Missing CSV column '{name}'")]
    MissingColumn { name: String },

    /// An I/O error occurred while reading the data source.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
