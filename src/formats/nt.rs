//! N-Triples edge iterator for the formats layer.
//!
//! ```no_run
//! use pathrex::formats::NTriples;
//! use pathrex::formats::FormatError;
//!
//! # let reader = std::io::empty();
//! let iter = NTriples::new(reader)
//!     .filter_map(|r| match r {
//!         Err(FormatError::LiteralAsNode) => None,  // skip
//!         other => Some(other),
//!     });
//! ```
//!
//! To load into a graph:
//!
//! ```no_run
//! use pathrex::graph::{Graph, InMemory, GraphDecomposition};
//! use pathrex::formats::NTriples;
//! use std::fs::File;
//!
//! let graph = Graph::<InMemory>::try_from(
//!     NTriples::new(File::open("data.nt").unwrap())
//! ).unwrap();
//! ```

use std::io::Read;

use oxrdf::{NamedOrBlankNode, Term};
use oxttl::NTriplesParser;
use oxttl::ntriples::ReaderNTriplesParser;

use crate::formats::FormatError;
use crate::graph::Edge;

/// Controls how predicate IRIs are converted to edge label strings.
#[derive(Debug, Clone, Default)]
pub enum LabelExtraction {
    /// Use only the local name: the fragment (`#name`) or last path segment.
    /// For example, `http://example.org/ns/knows` → `"knows"`.
    /// This is the default.
    #[default]
    LocalName,
    /// Use the full IRI string as the label.
    /// For example, `http://example.org/ns/knows` → `"http://example.org/ns/knows"`.
    FullIri,
}

/// An iterator that reads N-Triples and yields `Result<Edge, FormatError>`.
///
/// # Example
///
/// ```no_run
/// use pathrex::formats::nt::NTriples;
/// use std::fs::File;
///
/// let file = File::open("data.nt").unwrap();
/// let iter = NTriples::new(file);
/// for result in iter {
///     let edge = result.unwrap();
///     println!("{} --{}--> {}", edge.source, edge.label, edge.target);
/// }
/// ```
pub struct NTriples<R: Read> {
    inner: ReaderNTriplesParser<R>,
    label_extraction: LabelExtraction,
}

impl<R: Read> NTriples<R> {
    pub fn new(reader: R) -> Self {
        Self::with_label_extraction(reader, LabelExtraction::LocalName)
    }

    pub fn with_label_extraction(reader: R, label_extraction: LabelExtraction) -> Self {
        Self {
            inner: NTriplesParser::new().for_reader(reader),
            label_extraction,
        }
    }

    fn subject_to_node_id(subject: NamedOrBlankNode) -> String {
        match subject {
            NamedOrBlankNode::NamedNode(n) => n.into_string(),
            NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
        }
    }

    fn object_to_node_id(object: Term) -> Result<String, FormatError> {
        match object {
            Term::NamedNode(n) => Ok(n.into_string()),
            Term::BlankNode(b) => Ok(format!("_:{}", b.as_str())),
            Term::Literal(_) => Err(FormatError::LiteralAsNode),
        }
    }

    fn extract_label(iri: &str, strategy: &LabelExtraction) -> String {
        match strategy {
            LabelExtraction::FullIri => iri.to_owned(),
            LabelExtraction::LocalName => {
                // Fragment takes priority, then last path segment.
                if let Some(pos) = iri.rfind('#') {
                    iri[pos + 1..].to_owned()
                } else if let Some(pos) = iri.rfind('/') {
                    iri[pos + 1..].to_owned()
                } else {
                    iri.to_owned()
                }
            }
        }
    }
}

impl<R: Read> Iterator for NTriples<R> {
    type Item = Result<Edge, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        let triple = match self.inner.next()? {
            Ok(t) => t,
            Err(e) => return Some(Err(FormatError::NTriples(e.to_string()))),
        };

        let source = Self::subject_to_node_id(triple.subject.into());
        let label = Self::extract_label(triple.predicate.as_str(), &self.label_extraction);
        let target = match Self::object_to_node_id(triple.object) {
            Ok(t) => t,
            Err(e) => return Some(Err(e)),
        };

        Some(Ok(Edge {
            source,
            target,
            label,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(nt: &str) -> Vec<Result<Edge, FormatError>> {
        NTriples::new(nt.as_bytes()).collect()
    }

    #[test]
    fn test_basic_ntriples() {
        let nt = "<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n\
                  <http://example.org/Bob> <http://example.org/likes> <http://example.org/Charlie> .\n";
        let edges = parse(nt);
        assert_eq!(edges.len(), 2);

        let e0 = edges[0].as_ref().unwrap();
        assert_eq!(e0.source, "http://example.org/Alice");
        assert_eq!(e0.target, "http://example.org/Bob");
        assert_eq!(e0.label, "knows");

        let e1 = edges[1].as_ref().unwrap();
        assert_eq!(e1.source, "http://example.org/Bob");
        assert_eq!(e1.target, "http://example.org/Charlie");
        assert_eq!(e1.label, "likes");
    }

    #[test]
    fn test_full_iri_label_extraction() {
        let nt =
            "<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n";
        let edges: Vec<_> =
            NTriples::with_label_extraction(nt.as_bytes(), LabelExtraction::FullIri).collect();

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].as_ref().unwrap().label, "http://example.org/knows");
    }

    #[test]
    fn test_blank_node_subject_and_object() {
        let nt = "_:b1 <http://example.org/knows> _:b2 .\n";
        let edges = parse(nt);
        assert_eq!(edges.len(), 1);

        let e = edges[0].as_ref().unwrap();
        assert_eq!(e.source, "_:b1");
        assert_eq!(e.target, "_:b2");
    }

    #[test]
    fn test_literal_object_yields_error() {
        let nt = "<http://example.org/Alice> <http://example.org/name> \"Alice\" .\n";
        let edges = parse(nt);
        assert_eq!(edges.len(), 1);
        assert!(
            matches!(edges[0], Err(FormatError::LiteralAsNode)),
            "literal object should yield LiteralAsNode error"
        );
    }

    #[test]
    fn test_caller_can_skip_literal_triples() {
        let nt = "<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n\
                  <http://example.org/Alice> <http://example.org/name> \"Alice\" .\n\
                  <http://example.org/Bob> <http://example.org/knows> <http://example.org/Charlie> .\n";
        let edges: Vec<_> = NTriples::new(nt.as_bytes())
            .filter_map(|r| match r {
                Err(FormatError::LiteralAsNode) => None,
                other => Some(other),
            })
            .collect();

        assert_eq!(edges.len(), 2, "literal triple should be skipped");
        assert!(edges.iter().all(|r| r.is_ok()));
    }

    #[test]
    fn test_fragment_iri_local_name() {
        let nt =
            "<http://example.org/Alice> <http://example.org/ns#knows> <http://example.org/Bob> .\n";
        let edges = parse(nt);
        assert_eq!(edges[0].as_ref().unwrap().label, "knows");
    }

    #[test]
    fn test_ntriples_graph_source() {
        use crate::graph::{GraphBuilder, GraphDecomposition, InMemoryBuilder};

        let nt = "<http://example.org/A> <http://example.org/knows> <http://example.org/B> .\n\
                  <http://example.org/B> <http://example.org/knows> <http://example.org/C> .\n";
        let iter = NTriples::new(nt.as_bytes());

        let graph = InMemoryBuilder::default()
            .load(iter)
            .expect("load should succeed")
            .build()
            .expect("build should succeed");
        assert_eq!(graph.num_nodes(), 3);
    }
}
