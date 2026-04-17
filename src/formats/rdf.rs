//! RDF parser supporting N-Tripples and Turtle formats.
//!
//! # Example
//! ```no_run
//! use pathrex::formats::{Rdf, RdfFormat};
//! use pathrex::graph::{Graph, InMemory};
//! use std::fs::File;
//!
//! // Explicit format
//! let file = File::open("data.ttl").unwrap();
//! let graph = Graph::<InMemory>::try_from(
//!     Rdf::new(file, RdfFormat::Turtle)
//! ).unwrap();
//!
//! // Auto-detect from path
//! let graph = Graph::<InMemory>::try_from(
//!     Rdf::from_path("data.ttl").unwrap()
//! ).unwrap();
//! ```

use std::io::Read;
use std::path::Path;

use oxrdf::{NamedOrBlankNode, Term, Triple};
use oxttl::{NTriplesParser, TurtleParseError, TurtleParser};

use crate::formats::FormatError;
use crate::graph::Edge;

/// Supported RDF serialization formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdfFormat {
    /// N-Triples format (.nt)
    NTriples,
    /// Turtle format (.ttl)
    Turtle,
}

impl RdfFormat {
    /// Detect format from file extension.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Option<Self> {
        match path.as_ref().extension()?.to_str()? {
            "nt" | "ntriples" => Some(Self::NTriples),
            "ttl" | "turtle" => Some(Self::Turtle),
            _ => None,
        }
    }
}

/// RDF parser supporting N-Tripples and Turtle formats.
///
/// # Example
/// ```no_run
/// use pathrex::formats::{Rdf, RdfFormat};
/// use std::fs::File;
///
/// let file = File::open("data.ttl").unwrap();
/// for edge in Rdf::new(file, RdfFormat::Turtle) {
///     println!("{:?}", edge);
/// }
/// ```
pub struct Rdf {
    inner: Box<dyn Iterator<Item = Result<Triple, TurtleParseError>>>,
}

impl Rdf {
    /// Create a new RDF parser with explicit format.
    pub fn new<R: Read + 'static>(reader: R, format: RdfFormat) -> Self {
        let inner: Box<dyn Iterator<Item = Result<Triple, TurtleParseError>>> = match format {
            RdfFormat::NTriples => Box::new(NTriplesParser::new().for_reader(reader)),
            RdfFormat::Turtle => Box::new(TurtleParser::new().for_reader(reader)),
        };
        Self { inner }
    }

    /// Create parser by detecting format from file path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, FormatError> {
        let path = path.as_ref();
        let format = RdfFormat::from_path(path).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Unknown RDF extension: {:?}", path.extension()),
            ))
        })?;
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        Ok(Rdf::new(reader, format))
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

impl Iterator for Rdf {
    type Item = Result<Edge, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        let triple = match self.inner.next()? {
            Ok(t) => t,
            Err(e) => return Some(Err(e.into())),
        };

        let source = subject_to_node_id(triple.subject.into());
        let label = triple.predicate.as_str().to_owned();
        let target = match object_to_node_id(triple.object) {
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

    fn parse_turtle(ttl: &[u8]) -> Vec<Result<Edge, FormatError>> {
        Rdf::new(std::io::Cursor::new(ttl.to_vec()), RdfFormat::Turtle).collect()
    }

    fn parse_ntriples(nt: &[u8]) -> Vec<Result<Edge, FormatError>> {
        Rdf::new(std::io::Cursor::new(nt.to_vec()), RdfFormat::NTriples).collect()
    }

    #[test]
    fn test_turtle_basic() {
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:knows ex:Bob .
            ex:Bob ex:knows ex:Charlie .
        "#;
        let edges: Vec<_> = parse_turtle(ttl)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].source, "http://example.org/Alice");
        assert_eq!(edges[0].label, "http://example.org/knows");
        assert_eq!(edges[0].target, "http://example.org/Bob");
    }

    #[test]
    fn test_turtle_predicate_object_lists() {
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:knows ex:Bob, ex:Charlie ;
                     ex:likes ex:Dave .
        "#;
        let edges: Vec<_> = parse_turtle(ttl)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_ntriples_basic() {
        let nt = b"<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n\
                  <http://example.org/Bob> <http://example.org/likes> <http://example.org/Charlie> .\n";
        let edges: Vec<_> = parse_ntriples(nt)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].source, "http://example.org/Alice");
        assert_eq!(edges[0].label, "http://example.org/knows");
    }

    #[test]
    fn test_literal_yields_error() {
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:name "Alice" .
        "#;
        let results = parse_turtle(ttl);
        assert!(matches!(results[0], Err(FormatError::LiteralAsNode)));
    }

    #[test]
    fn test_format_detection() {
        assert_eq!(RdfFormat::from_path("data.ttl"), Some(RdfFormat::Turtle));
        assert_eq!(RdfFormat::from_path("data.turtle"), Some(RdfFormat::Turtle));
        assert_eq!(RdfFormat::from_path("data.nt"), Some(RdfFormat::NTriples));
        assert_eq!(
            RdfFormat::from_path("data.ntriples"),
            Some(RdfFormat::NTriples)
        );
        assert_eq!(RdfFormat::from_path("data.csv"), None);
    }

    #[test]
    fn test_blank_nodes() {
        let nt = b"_:b1 <http://example.org/knows> _:b2 .\n";
        let edges: Vec<_> = parse_ntriples(nt)
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "_:b1");
        assert_eq!(edges[0].target, "_:b2");
    }

    #[test]
    fn test_caller_can_skip_literal_triples() {
        let nt = b"<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n\
                  <http://example.org/Alice> <http://example.org/name> \"Alice\" .\n\
                  <http://example.org/Bob> <http://example.org/knows> <http://example.org/Charlie> .\n";
        let edges: Vec<_> = Rdf::new(std::io::Cursor::new(nt.to_vec()), RdfFormat::NTriples)
            .filter_map(|r| match r {
                Err(FormatError::LiteralAsNode) => None,
                other => Some(other),
            })
            .collect();

        assert_eq!(edges.len(), 2, "literal triple should be skipped");
        assert!(edges.iter().all(|r| r.is_ok()));
    }

    #[test]
    fn test_non_ascii_in_iris() {
        let nt = "<http://example.org/人甲> <http://example.org/关系/认识> <http://example.org/人乙> .\n\
                  <http://example.org/Алиса> <http://example.org/знает> <http://example.org/Боб> .\n";
        let edges: Vec<_> = parse_ntriples(nt.as_bytes())
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(edges.len(), 2);

        assert_eq!(edges[0].source, "http://example.org/人甲");
        assert_eq!(edges[0].target, "http://example.org/人乙");
        assert_eq!(edges[0].label, "http://example.org/关系/认识");

        assert_eq!(edges[1].source, "http://example.org/Алиса");
        assert_eq!(edges[1].target, "http://example.org/Боб");
        assert_eq!(edges[1].label, "http://example.org/знает");
    }
}
