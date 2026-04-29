//! Graph and query loading for the `pathrex` CLI.
//!
//! Both subcommands (`bench` and `query`) need to load a graph and a queries
//! file.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use thiserror::Error;

use crate::formats::Csv;
use crate::formats::MatrixMarket;
use crate::formats::Rdf;
use crate::graph::{Graph, GraphError, InMemory, InMemoryGraph};
use crate::rpq::{RpqError, RpqQuery};
use crate::sparql::parse_rpq;

use super::args::GraphFormat;

#[derive(Debug, Error)]
pub enum GraphLoadError {
    #[error("error opening graph at '{path}': {source}")]
    Open {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("error loading graph from '{path}': {source}")]
    Build {
        path: String,
        #[source]
        source: GraphError,
    },
}

/// Load an [`InMemoryGraph`] from `graph_path` in the given `format`.
pub fn load_graph(
    graph_path: &str,
    format: GraphFormat,
    base_iri: Option<&str>,
) -> Result<InMemoryGraph, GraphLoadError> {
    match format {
        GraphFormat::Mm => {
            let mm_base = MatrixMarket::from_dir(graph_path);
            let mm = match base_iri {
                Some(iri) => mm_base.with_base_iri(iri),
                None => mm_base,
            };
            Graph::<InMemory>::try_from(mm).map_err(|e| GraphLoadError::Build {
                path: graph_path.to_string(),
                source: e,
            })
        }
        GraphFormat::Csv => {
            let file = File::open(graph_path).map_err(|e| GraphLoadError::Open {
                path: graph_path.to_string(),
                source: e,
            })?;
            let csv_source = Csv::from_reader(file).map_err(|e| GraphLoadError::Build {
                path: graph_path.to_string(),
                source: e.into(),
            })?;
            Graph::<InMemory>::try_from(csv_source).map_err(|e| GraphLoadError::Build {
                path: graph_path.to_string(),
                source: e,
            })
        }
        GraphFormat::Rdf => {
            let rdf = Rdf::from_path(graph_path).unwrap();
            Graph::<InMemory>::try_from(rdf).map_err(|e| GraphLoadError::Build {
                path: graph_path.to_string(),
                source: e,
            })
        }
    }
}

#[derive(Debug)]
pub struct LoadedQuery {
    pub id: String,
    pub text: String,
    pub parsed: Result<RpqQuery, RpqError>,
}

/// Load and parse queries from a file.
///
/// Each non-empty line must have the format `<id>,<sparql_pattern>`.
/// The pattern is wrapped into a full SPARQL query before parsing:
/// - When `base_iri` is `Some(iri)`: `BASE <{iri}> SELECT * WHERE { {pattern} . }`
/// - When `base_iri` is `None`:      `SELECT * WHERE { {pattern} . }`
pub fn load_queries(
    path: &Path,
    base_iri: Option<&str>,
) -> Result<Vec<LoadedQuery>, std::io::Error> {
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

        let sparql = match base_iri {
            Some(iri) => format!("BASE <{iri}> SELECT * WHERE {{ {pattern} . }}"),
            None => format!("SELECT * WHERE {{ {pattern} . }}"),
        };
        let parsed = parse_rpq(&sparql);

        queries.push(LoadedQuery {
            id,
            text: pattern,
            parsed,
        });
    }

    Ok(queries)
}
