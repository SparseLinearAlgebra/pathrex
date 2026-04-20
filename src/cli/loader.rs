//! Graph and query loading for the `pathrex` CLI.
//!
//! Both subcommands (`bench` and `query`) need to load a graph and a queries
//! file. This module centralises that I/O so neither subcommand runner
//! duplicates it.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process;

use crate::formats::mm::MatrixMarket;
use crate::formats::Csv;
use crate::graph::{Graph, InMemory, InMemoryGraph};
use crate::rpq::{RpqError, RpqQuery};
use crate::sparql::parse_rpq;

// ── Graph loading ────────────────────────────────────────────────────────────

/// Load an [`InMemoryGraph`] from `graph_path` in the given `format`.
///
/// Prints an error message and exits the process on failure, which is
/// appropriate for a CLI entry point.
pub fn load_graph(graph_path: &str, format: &str, base_iri: &str) -> InMemoryGraph {
    match format {
        "mm" => {
            let mm = MatrixMarket::from_dir(graph_path).with_base_iri(base_iri);
            Graph::<InMemory>::try_from(mm).unwrap_or_else(|e| {
                eprintln!("Error loading MatrixMarket graph from '{graph_path}': {e}");
                process::exit(1);
            })
        }
        "csv" => {
            let file = File::open(graph_path).unwrap_or_else(|e| {
                eprintln!("Error opening CSV file '{graph_path}': {e}");
                process::exit(1);
            });
            let csv_source = Csv::from_reader(file).unwrap_or_else(|e| {
                eprintln!("Error creating CSV reader for '{graph_path}': {e}");
                process::exit(1);
            });
            Graph::<InMemory>::try_from(csv_source).unwrap_or_else(|e| {
                eprintln!("Error loading CSV graph from '{graph_path}': {e}");
                process::exit(1);
            })
        }
        other => {
            eprintln!("Unknown graph format: '{other}' (expected: mm, csv)");
            process::exit(1);
        }
    }
}

// ── Query loading ─────────────────────────────────────────────────────────────

/// A single loaded query with its metadata.
#[derive(Debug)]
pub struct LoadedQuery {
    /// The ID from the query file (the part before the first comma).
    pub id: String,
    /// The raw SPARQL pattern text (the part after the first comma).
    pub text: String,
    /// The parsed RPQ query, or an error if parsing failed.
    pub parsed: Result<RpqQuery, RpqError>,
}

/// Load and parse queries from a file.
///
/// Each non-empty line must have the format `<id>,<sparql_pattern>`.
/// The pattern is wrapped into a full SPARQL query:
/// `BASE <{base_iri}> SELECT * WHERE { {pattern} . }`
/// before parsing, matching the convention used in integration tests.
pub fn load_queries(path: &Path, base_iri: &str) -> Result<Vec<LoadedQuery>, std::io::Error> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut queries = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (id, pattern) = match trimmed.splitn(2, ',').collect::<Vec<_>>().as_slice() {
            [id, pattern] => (id.trim().to_string(), pattern.trim().to_string()),
            _ => {
                queries.push(LoadedQuery {
                    id: "?".to_string(),
                    text: trimmed.to_string(),
                    parsed: Err(RpqError::UnsupportedPath(format!(
                        "query line has no comma: {trimmed:?}"
                    ))),
                });
                continue;
            }
        };

        let sparql = format!("BASE <{base_iri}> SELECT * WHERE {{ {pattern} . }}");
        let parsed = parse_rpq(&sparql);

        queries.push(LoadedQuery {
            id,
            text: pattern,
            parsed,
        });
    }

    Ok(queries)
}
