//! RDF parser supporting N-Tripples and Turtle formats.
//!
//! # Example
//! ```no_run
//! use pathrex::formats::{Rdf, RdfFormat};
//! use pathrex::graph::{Graph, InMemory};
//!
//! // Auto-detect from path
//! let graph = Graph::<InMemory>::try_from(
//!     Rdf::from_path("data.ttl").unwrap()
//! ).unwrap();
//! ```

use std::ops::Deref;
use std::path::Path;

use oxrdf::{NamedOrBlankNode, Term, Triple};
use oxttl::{NTriplesParser, TurtleParser};
use rayon::prelude::*;

use crate::formats::FormatError;
use crate::graph::Edge;

enum RdfData {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Deref for RdfData { type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            RdfData::Mapped(m) => m,
            RdfData::Owned(v) => v,
        }
    }
}

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

/// RDF parser supporting N-Triples and Turtle formats.
///
/// # Example
/// ```no_run
/// use pathrex::formats::{Rdf, RdfFormat};
/// use pathrex::graph::{Graph, InMemory};
///
/// let graph = Graph::<InMemory>::try_from(
///     Rdf::from_path("data.ttl").unwrap()
/// ).unwrap();
/// ```
pub struct Rdf {
    data: RdfData,
    format: RdfFormat,
}

impl Rdf {
    /// Create a parser from an in-memory byte slice and a known format.
    pub fn new(data: impl Into<Vec<u8>>, format: RdfFormat) -> Self {
        Self {
            data: RdfData::Owned(data.into()),
            format,
        }
    }

    /// Load a file from `path`, detecting its format from the extension.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, FormatError> {
        let path = path.as_ref();
        let format = RdfFormat::from_path(path).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Unknown RDF extension: {:?}", path.extension()),
            ))
        })?;

        let file = std::fs::File::open(path)?;

        let data = match unsafe { memmap2::Mmap::map(&file) } {
            Ok(mmap) => RdfData::Mapped(mmap),
            Err(_) => {
                use std::io::Read;
                let mut buf = Vec::new();
                let mut file = file;
                file.read_to_end(&mut buf)?;
                RdfData::Owned(buf)
            }
        };

        Ok(Self { data, format })
    }

    /// Parse the stored bytes in parallel, returning an iterator of edges and errors.
    pub fn parse(self) -> impl Iterator<Item = Result<Edge, FormatError>> {
        let target_parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let bytes: &[u8] = &self.data;

        let edges: Vec<Result<Edge, FormatError>> = match self.format {
            RdfFormat::NTriples => NTriplesParser::new()
                .lenient()
                .split_slice_for_parallel_parsing(bytes, target_parallelism)
                .into_par_iter()
                .flat_map_iter(|parser| parser.map(|item| triple_to_edge(item?)))
                .collect(),
            RdfFormat::Turtle => TurtleParser::new()
                .lenient()
                .split_slice_for_parallel_parsing(bytes, target_parallelism)
                .into_par_iter()
                .flat_map_iter(|parser| parser.map(|item| triple_to_edge(item?)))
                .collect(),
        };

        edges.into_iter()
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

/// Convert a parsed [`Triple`] into an [`Edge`].
pub(crate) fn triple_to_edge(triple: Triple) -> Result<Edge, FormatError> {
    let source = subject_to_node_id(triple.subject.into());
    let label = triple.predicate.as_str().to_owned();
    let target = object_to_node_id(triple.object)?;
    Ok(Edge {
        source,
        target,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_turtle(ttl: &[u8]) -> Vec<Result<Edge, FormatError>> {
        Rdf::new(ttl, RdfFormat::Turtle).parse().collect()
    }

    fn parse_ntriples(nt: &[u8]) -> Vec<Result<Edge, FormatError>> {
        Rdf::new(nt, RdfFormat::NTriples).parse().collect()
    }

    fn ok_edges(results: Vec<Result<Edge, FormatError>>) -> Vec<Edge> {
        results.into_iter().filter_map(|r| r.ok()).collect()
    }

    #[test]
    fn test_turtle_basic() {
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:knows ex:Bob .
            ex:Bob ex:knows ex:Charlie .
        "#;
        let mut edges = ok_edges(parse_turtle(ttl));
        edges.sort_by(|a, b| a.source.cmp(&b.source).then(a.target.cmp(&b.target)));
        assert_eq!(edges.len(), 2);
        // Find Alice->Bob edge
        let alice_bob = edges
            .iter()
            .find(|e| e.source == "http://example.org/Alice")
            .unwrap();
        assert_eq!(alice_bob.label, "http://example.org/knows");
        assert_eq!(alice_bob.target, "http://example.org/Bob");
    }

    #[test]
    fn test_turtle_predicate_object_lists() {
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:knows ex:Bob, ex:Charlie ;
                     ex:likes ex:Dave .
        "#;
        let edges = ok_edges(parse_turtle(ttl));
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_ntriples_basic() {
        let nt = b"<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n\
                  <http://example.org/Bob> <http://example.org/likes> <http://example.org/Charlie> .\n";
        let edges = ok_edges(parse_ntriples(nt));
        assert_eq!(edges.len(), 2);
        let alice = edges
            .iter()
            .find(|e| e.source == "http://example.org/Alice")
            .unwrap();
        assert_eq!(alice.label, "http://example.org/knows");
    }

    #[test]
    fn test_literal_yields_error() {
        // parallel_rdf_edges now returns Err(FormatError::LiteralAsNode) for literal-object triples
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:name "Alice" .
            ex:Alice ex:knows ex:Bob .
        "#;
        let results = parse_turtle(ttl);
        let errors: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
        let edges: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        // The literal-object triple produces an error
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], Err(FormatError::LiteralAsNode)));
        // The valid triple still produces an edge
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "http://example.org/Alice");
        assert_eq!(edges[0].label, "http://example.org/knows");
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
        let edges = ok_edges(parse_ntriples(nt));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "_:b1");
        assert_eq!(edges[0].target, "_:b2");
    }

    #[test]
    fn test_non_ascii_in_iris() {
        let nt = "<http://example.org/人甲> <http://example.org/关系/认识> <http://example.org/人乙> .\n\
                  <http://example.org/Алиса> <http://example.org/знает> <http://example.org/Боб> .\n";
        let edges = ok_edges(parse_ntriples(nt.as_bytes()));
        assert_eq!(edges.len(), 2);

        let person1 = edges
            .iter()
            .find(|e| e.source == "http://example.org/人甲")
            .unwrap();
        assert_eq!(person1.target, "http://example.org/人乙");
        assert_eq!(person1.label, "http://example.org/关系/认识");

        let alice = edges
            .iter()
            .find(|e| e.source == "http://example.org/Алиса")
            .unwrap();
        assert_eq!(alice.target, "http://example.org/Боб");
        assert_eq!(alice.label, "http://example.org/знает");
    }

    #[test]
    fn test_from_path_mmap() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let nt = b"<http://example.org/A> <http://example.org/rel> <http://example.org/B> .\n";
        let mut f = NamedTempFile::with_suffix(".nt").unwrap();
        f.write_all(nt).unwrap();
        f.flush().unwrap();

        let edges = ok_edges(Rdf::from_path(f.path()).unwrap().parse().collect());
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "http://example.org/A");
        assert_eq!(edges[0].target, "http://example.org/B");
    }
}
