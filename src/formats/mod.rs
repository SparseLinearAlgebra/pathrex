//! Data format parsers for pathrex.
//!
//! # Quick-start examples
//!
//! ```no_run
//! use pathrex::graph::{Graph, InMemory, GraphDecomposition};
//! use pathrex::formats::{Csv, Rdf};
//! use std::fs::File;
//!
//! // Build from CSV
//! let g = Graph::<InMemory>::try_from(
//!     Csv::from_reader(File::open("edges.csv").unwrap()).unwrap()
//! ).unwrap();
//!
//! // Build from Turtle (auto-detect from extension)
//! let g2 = Graph::<InMemory>::try_from(
//!     Rdf::from_path("data.ttl").unwrap()
//! ).unwrap();
//! ```

pub mod csv;
pub mod mm;
pub mod rdf;

pub use csv::Csv;
pub use mm::MatrixMarket;
pub use rdf::{Rdf, RdfFormat};

use oxttl::TurtleSyntaxError;
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

    /// An error produced by an RDF parser (N-Triples, Turtle, etc.)
    #[error("RDF parse error: {0}")]
    Rdf(#[from] TurtleSyntaxError),

    /// An RDF literal appeared as a subject or object where a node IRI or
    /// blank node was expected.
    #[error("RDF literal cannot be used as a graph node")]
    LiteralAsNode,
}
