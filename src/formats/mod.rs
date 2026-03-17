//! Data format parsers for pathrex.
//!
//! # Quick-start examples
//!
//! ```no_run
//! use pathrex::graph::{Graph, InMemory, GraphDecomposition};
//! use pathrex::formats::{Csv, NTriples};
//! use std::fs::File;
//!
//! // Build from CSV in one line
//! let g = Graph::<InMemory>::try_from(
//!     Csv::from_reader(File::open("edges.csv").unwrap()).unwrap()
//! ).unwrap();
//!
//! // Build from N-Triples in one line
//! let g2 = Graph::<InMemory>::try_from(
//!     NTriples::new(File::open("data.nt").unwrap())
//! ).unwrap();
//! ```

pub mod csv;
pub mod mm;
pub mod nt;

pub use csv::Csv;
pub use mm::MatrixMarket;
pub use nt::NTriples;

use thiserror::Error;

use crate::lagraph_sys::GrB_Info;

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

    /// [`LAGraph_MMRead`](crate::lagraph_sys::LAGraph_MMRead) returned a
    /// non-zero info code while reading a MatrixMarket file.
    #[error("MatrixMarket read error (code {code}): {message}")]
    MatrixMarket { code: GrB_Info, message: String },

    #[error("Invalid format in '{file}' at line {line}: {reason}")]
    InvalidFormat {
        file: String,
        line: usize,
        reason: String,
    },

    /// An error produced by the N-Triples parser.
    #[error("N-Triples parse error: {0}")]
    NTriples(String),

    /// An RDF literal appeared as a subject or object where a node IRI or
    /// blank node was expected.
    #[error("RDF literal cannot be used as a graph node (triple skipped)")]
    LiteralAsNode,
}
