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
}

impl<R: Read> NTriples<R> {
    pub fn new(reader: R) -> Self {
        Self {
            inner: NTriplesParser::new().for_reader(reader),
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
}

impl<R: Read> Iterator for NTriples<R> {
    type Item = Result<Edge, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        let triple = match self.inner.next()? {
            Ok(t) => t,
            Err(e) => return Some(Err(FormatError::NTriples(e.to_string()))),
        };

        let source = Self::subject_to_node_id(triple.subject.into());
        let label = triple.predicate.as_str().to_owned();
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
        assert_eq!(e0.label, "http://example.org/knows");

        let e1 = edges[1].as_ref().unwrap();
        assert_eq!(e1.source, "http://example.org/Bob");
        assert_eq!(e1.target, "http://example.org/Charlie");
        assert_eq!(e1.label, "http://example.org/likes");
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
    fn test_predicate_with_fragment_is_full_iri_string() {
        let nt =
            "<http://example.org/Alice> <http://example.org/ns#knows> <http://example.org/Bob> .\n";
        let edges = parse(nt);
        assert_eq!(
            edges[0].as_ref().unwrap().label,
            "http://example.org/ns#knows"
        );
    }

    #[test]
    fn test_non_ascii_in_iris() {
        let nt = "<http://example.org/人甲> <http://example.org/关系/认识> <http://example.org/人乙> .\n\
                  <http://example.org/Алиса> <http://example.org/знает> <http://example.org/Боб> .\n";
        let edges = parse(nt);
        assert_eq!(edges.len(), 2);

        let e0 = edges[0].as_ref().unwrap();
        assert_eq!(e0.source, "http://example.org/人甲");
        assert_eq!(e0.target, "http://example.org/人乙");
        assert_eq!(e0.label, "http://example.org/关系/认识");

        let e1 = edges[1].as_ref().unwrap();
        assert_eq!(e1.source, "http://example.org/Алиса");
        assert_eq!(e1.target, "http://example.org/Боб");
        assert_eq!(e1.label, "http://example.org/знает");
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
