//! RDF parser supporting N-Triples and Turtle formats.
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

use lasso::{Key, Rodeo, Spur};
use oxrdf::{NamedOrBlankNode, Term, Triple};
use oxttl::{NTriplesParser, TurtleParser};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::formats::FormatError;
use crate::graph::Edge;
use crate::lagraph_sys::GrB_Index;

enum RdfData {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Deref for RdfData {
    type Target = [u8];

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
            Ok(mmap) => {
                let _ = mmap.advise(memmap2::Advice::Sequential);
                RdfData::Mapped(mmap)
            }
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

    /// Returns the detected/configured RDF format.
    pub(crate) fn format(&self) -> RdfFormat {
        self.format
    }

    /// Returns a byte-slice view of the stored data.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Parse the stored bytes in parallel, returning an iterator of edges and errors.
    pub fn parse(self) -> impl Iterator<Item = Result<Edge, FormatError>> {
        let n_shards = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let shards = parse_shards(&self.data, self.format, n_shards);

        shards.into_iter().flat_map(|shard| {
            let mut node_strings: Vec<String> = vec![String::new(); shard.nodes.len()];
            for (spur, s) in shard.nodes.iter() {
                node_strings[spur.into_usize()] = s.to_owned();
            }
            shard
                .edges_by_label
                .into_iter()
                .flat_map(move |(label, pairs)| {
                    let node_strings = node_strings.clone();
                    pairs.into_iter().map(move |(src, tgt)| {
                        Ok(Edge {
                            source: node_strings[src as usize].clone(),
                            label: label.clone(),
                            target: node_strings[tgt as usize].clone(),
                        })
                    })
                })
        })
    }
}

pub(crate) struct RdfShard {
    nodes: Rodeo,
    edges_by_label: FxHashMap<String, Vec<(u32, u32)>>,
    skipped: u64,
}

impl RdfShard {
    fn new() -> Self {
        Self {
            nodes: Rodeo::new(),
            edges_by_label: FxHashMap::default(),
            skipped: 0,
        }
    }

    #[inline(always)]
    fn push_triple(&mut self, subject: &str, predicate: &str, object: &str) {
        let s = self.nodes.get_or_intern(subject).into_usize() as u32;
        let t = self.nodes.get_or_intern(object).into_usize() as u32;
        self.edges_by_label
            .entry(predicate.to_owned())
            .or_default()
            .push((s, t));
    }
}

#[allow(dead_code)] // skipped_total read only in tests
pub(crate) struct Merged {
    pub(crate) nodes: Rodeo,
    pub(crate) label_names: Vec<String>,
    pub(crate) edges_by_global_label: Vec<Vec<(GrB_Index, GrB_Index)>>,
    pub(crate) skipped_total: u64,
}

/// Parse `bytes` in parallel using N shards and return one [`RdfShard`] per shard.
pub(crate) fn parse_shards(bytes: &[u8], format: RdfFormat, n_shards: usize) -> Vec<RdfShard> {
    let process = |shard: &mut RdfShard, result: Result<Triple, _>| match result {
        Ok(triple) => {
            let object = match &triple.object {
                Term::NamedNode(n) => n.as_str().to_owned(),
                Term::BlankNode(b) => format!("_:{}", b.as_str()),
                Term::Literal(l) => l.to_string(),
            };
            let subject = match &triple.subject {
                NamedOrBlankNode::NamedNode(n) => n.as_str().to_owned(),
                NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
            };
            shard.push_triple(&subject, triple.predicate.as_str(), &object);
        }
        Err(_) => {
            shard.skipped += 1;
        }
    };

    match format {
        RdfFormat::NTriples => NTriplesParser::new()
            .lenient()
            .split_slice_for_parallel_parsing(bytes, n_shards)
            .into_par_iter()
            .map(|parser| {
                let mut shard = RdfShard::new();
                for result in parser {
                    process(&mut shard, result);
                }
                shard
            })
            .collect(),
        RdfFormat::Turtle => TurtleParser::new()
            .lenient()
            .split_slice_for_parallel_parsing(bytes, n_shards)
            .into_par_iter()
            .map(|parser| {
                let mut shard = RdfShard::new();
                for result in parser {
                    process(&mut shard, result);
                }
                shard
            })
            .collect(),
    }
}

pub(crate) fn merge_shards(shards: Vec<RdfShard>) -> Merged {
    let n_shards = shards.len();

    let mut global_nodes: Rodeo = Rodeo::new();
    let mut node_remaps: Vec<Vec<u32>> = Vec::with_capacity(n_shards);
    let mut shard_node_rodeos: Vec<Option<Rodeo>> = Vec::with_capacity(n_shards);
    let mut shard_edges: Vec<FxHashMap<String, Vec<(u32, u32)>>> = Vec::with_capacity(n_shards);
    let mut skipped_total: u64 = 0;

    for shard in shards {
        skipped_total += shard.skipped;
        shard_node_rodeos.push(Some(shard.nodes));
        shard_edges.push(shard.edges_by_label);
    }

    // Pass 1: node remap — drop each shard Rodeo immediately after building its table.
    for rodeo_opt in shard_node_rodeos.iter_mut() {
        let rodeo = rodeo_opt.take().unwrap();
        let mut remap: Vec<u32> = Vec::with_capacity(rodeo.len());
        for (_spur, string) in rodeo.iter() {
            let global_spur: Spur = global_nodes.get_or_intern(string);
            remap.push(global_spur.into_usize() as u32);
        }
        node_remaps.push(remap);
        // rodeo dropped here — frees memory before next iteration
    }
    drop(shard_node_rodeos);

    // Pass 2: collect the union of all predicate IRI strings across shards.
    // No separate Rodeo needed — collect unique predicate strings across shards.
    let mut label_names: Vec<String> = Vec::new();
    {
        let mut seen: FxHashMap<&str, ()> = FxHashMap::default();
        for edges_map in &shard_edges {
            for label in edges_map.keys() {
                seen.entry(label).or_insert_with(|| {
                    label_names.push(label.to_string());
                });
            }
        }
    }

    // Pass 3: parallel rewrite + concatenation of per-label COO pairs.
    // Each label is looked up by its string key in each shard's FxHashMap —
    // O(1) with Fx hashing, no remap indirection needed.
    let shard_edges_ref = &shard_edges;
    let node_remaps_ref = &node_remaps;

    let edges_by_global_label: Vec<Vec<(GrB_Index, GrB_Index)>> = label_names
        .par_iter()
        .map(|label| {
            let mut combined: Vec<(GrB_Index, GrB_Index)> = Vec::new();
            for (shard_idx, edges_map) in shard_edges_ref.iter().enumerate() {
                if let Some(pairs) = edges_map.get(label.as_str()) {
                    let nremap = &node_remaps_ref[shard_idx];
                    combined.reserve(pairs.len());
                    for &(local_src, local_tgt) in pairs {
                        combined.push((
                            nremap[local_src as usize] as GrB_Index,
                            nremap[local_tgt as usize] as GrB_Index,
                        ));
                    }
                }
            }
            combined
        })
        .collect();

    if skipped_total > 0 {
        eprintln!(
            "[pathrex] RDF load: skipped {skipped_total} triples (parse errors or literal objects)"
        );
    }

    Merged {
        nodes: global_nodes,
        label_names,
        edges_by_global_label,
        skipped_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lasso::RodeoReader;

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

    fn merge_nt(nt: &[u8]) -> Merged {
        let shards = parse_shards(nt, RdfFormat::NTriples, 4);
        merge_shards(shards)
    }

    fn merge_ttl(ttl: &[u8]) -> Merged {
        let shards = parse_shards(ttl, RdfFormat::Turtle, 4);
        merge_shards(shards)
    }

    fn reader(rodeo: Rodeo) -> RodeoReader {
        rodeo.into_reader()
    }

    #[test]
    fn test_nt_basic_node_and_edge_counts() {
        let nt = b"<http://example.org/A> <http://example.org/knows> <http://example.org/B> .\n\
                   <http://example.org/B> <http://example.org/knows> <http://example.org/C> .\n\
                   <http://example.org/A> <http://example.org/likes> <http://example.org/C> .\n";

        let merged = merge_nt(nt);
        let r = reader(merged.nodes);

        assert_eq!(r.len(), 3, "A, B, C");
        assert_eq!(merged.label_names.len(), 2, "knows, likes");

        let knows_idx = merged
            .label_names
            .iter()
            .position(|l| l == "http://example.org/knows")
            .expect("knows label");
        let likes_idx = merged
            .label_names
            .iter()
            .position(|l| l == "http://example.org/likes")
            .expect("likes label");

        assert_eq!(merged.edges_by_global_label[knows_idx].len(), 2);
        assert_eq!(merged.edges_by_global_label[likes_idx].len(), 1);
    }

    #[test]
    fn test_nt_round_trip_node_ids() {
        let nt =
            b"<http://example.org/Alice> <http://example.org/knows> <http://example.org/Bob> .\n";

        let merged = merge_nt(nt);
        let r = reader(merged.nodes);

        let alice_id = r.get("http://example.org/Alice").map(|s| s.into_usize());
        let bob_id = r.get("http://example.org/Bob").map(|s| s.into_usize());

        assert!(alice_id.is_some(), "Alice must be in global dictionary");
        assert!(bob_id.is_some(), "Bob must be in global dictionary");
        assert_ne!(alice_id, bob_id, "Alice and Bob must have distinct ids");
    }

    #[test]
    fn test_nt_matches_reference_single_threaded() {
        let nt = b"<http://a> <http://p1> <http://b> .\n\
                   <http://b> <http://p2> <http://c> .\n\
                   <http://a> <http://p1> <http://c> .\n\
                   <http://c> <http://p3> <http://a> .\n";

        let merged = merge_nt(nt);
        let r = reader(merged.nodes);

        assert_eq!(r.len(), 3);
        assert_eq!(merged.label_names.len(), 3);

        let p1 = merged
            .label_names
            .iter()
            .position(|l| l == "http://p1")
            .unwrap();
        let p2 = merged
            .label_names
            .iter()
            .position(|l| l == "http://p2")
            .unwrap();
        let p3 = merged
            .label_names
            .iter()
            .position(|l| l == "http://p3")
            .unwrap();

        assert_eq!(merged.edges_by_global_label[p1].len(), 2);
        assert_eq!(merged.edges_by_global_label[p2].len(), 1);
        assert_eq!(merged.edges_by_global_label[p3].len(), 1);
    }

    #[test]
    fn test_nt_shard_boundary_safety() {
        let mut nt = Vec::new();
        for i in 0..20u32 {
            nt.extend_from_slice(
                format!(
                    "<http://example.org/n{i}> <http://example.org/edge> <http://example.org/n{}> .\n",
                    i + 1
                )
                .as_bytes(),
            );
        }

        let merged = merge_nt(&nt);
        let r = reader(merged.nodes);

        assert_eq!(r.len(), 21);
        let edge_idx = merged
            .label_names
            .iter()
            .position(|l| l == "http://example.org/edge")
            .unwrap();
        assert_eq!(merged.edges_by_global_label[edge_idx].len(), 20);
    }

    #[test]
    fn test_nt_determinism_of_structure() {
        let nt = b"<http://a> <http://p> <http://b> .\n\
                   <http://b> <http://p> <http://c> .\n";

        let m1 = merge_nt(nt);
        let m2 = merge_nt(nt);

        assert_eq!(m1.nodes.len(), m2.nodes.len());
        assert_eq!(m1.label_names.len(), m2.label_names.len());
        for (name, edges) in m1.label_names.iter().zip(m1.edges_by_global_label.iter()) {
            let idx2 = m2.label_names.iter().position(|l| l == name).unwrap();
            assert_eq!(edges.len(), m2.edges_by_global_label[idx2].len());
        }
    }

    #[test]
    fn test_ttl_basic_sharded() {
        let ttl = br#"
            @prefix ex: <http://example.org/> .
            ex:Alice ex:knows ex:Bob .
            ex:Bob ex:knows ex:Charlie .
            ex:Alice ex:likes ex:Charlie .
        "#;

        let merged = merge_ttl(ttl);
        let r = reader(merged.nodes);

        assert_eq!(r.len(), 3);
        assert_eq!(merged.label_names.len(), 2);

        let knows_idx = merged
            .label_names
            .iter()
            .position(|l| l == "http://example.org/knows")
            .expect("knows label");
        assert_eq!(merged.edges_by_global_label[knows_idx].len(), 2);
    }
}
