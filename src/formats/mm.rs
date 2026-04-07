//! MatrixMarket directory format loader.
//!
//! An edge-labeled graph is stored in a directory with the following layout:
//!
//! ```text
//! <dir>/
//!   vertices.txt   — one line per node:  `<node_name> <1-based-index>`
//!   edges.txt      — one line per label: `<label_name> <1-based-index>`
//!   1.txt          — MM adjacency matrix for the label with index 1
//!   2.txt          — MM adjacency matrix for the label with index 2
//!   …
//! ```
//!
//! # Example
//!
//! ```no_run
//! use pathrex::graph::{Graph, InMemory, GraphDecomposition};
//! use pathrex::formats::mm::MatrixMarket;
//!
//! let graph = Graph::<InMemory>::try_from(
//!     MatrixMarket::from_dir("path/to/graph/dir")
//! ).unwrap();
//! println!("Nodes: {}", graph.num_nodes());
//! ```

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::IntoRawFd;
use std::path::{Path, PathBuf};

use crate::formats::FormatError;
use crate::graph::{GraphError, ensure_grb_init};
use crate::la_ok;
use crate::lagraph_sys::{FILE, GrB_Matrix, LAGraph_MMRead};

/// Read a single MatrixMarket file and return the raw [`GrB_Matrix`].
pub fn load_mm_file(path: impl AsRef<Path>) -> Result<GrB_Matrix, FormatError> {
    let path = path.as_ref();

    ensure_grb_init().map_err(|e| match e {
        GraphError::LAGraph(info, msg) => FormatError::MatrixMarket {
            code: info,
            message: msg,
        },
        _ => FormatError::MatrixMarket {
            code: crate::lagraph_sys::GrB_Info::GrB_PANIC,
            message: "Failed to initialize GraphBLAS".to_string(),
        },
    })?;

    let file = File::open(path)?;
    let fd = file.into_raw_fd();

    let c_mode = CString::new("r").unwrap();
    let f = unsafe { libc::fdopen(fd, c_mode.as_ptr()) };
    if f.is_null() {
        unsafe { libc::close(fd) };
        return Err(std::io::Error::last_os_error().into());
    }

    let mut matrix: GrB_Matrix = std::ptr::null_mut();

    let err = la_ok!(LAGraph_MMRead(&mut matrix, f as *mut FILE));
    unsafe { libc::fclose(f) };

    match err {
        Ok(_) => Ok(matrix),
        Err(GraphError::LAGraph(info, msg)) => Err(FormatError::MatrixMarket {
            code: info,
            message: msg,
        }),
        _ => unreachable!("should be either mm read error or ok"),
    }
}

// Trims first "<" and last ">".
fn normalize_map_name(name: &str) -> String {
    let name = name.trim();
    if name.len() >= 2 && name.starts_with('<') && name.ends_with('>') {
        name[1..name.len() - 1].to_owned()
    } else {
        name.to_owned()
    }
}

pub(crate) fn apply_base_iri(name: String, base: Option<&str>) -> String {
    match base {
        Some(b) if !b.is_empty() => format!("{b}{name}"),
        _ => name,
    }
}

/// Parse a `<name> <index>` mapping file.
///
/// Throws error on non-positive or duplicate indicies
pub(crate) fn parse_index_map(
    path: &Path,
) -> Result<(HashMap<usize, String>, HashMap<String, usize>), FormatError> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let reader = BufReader::new(File::open(path)?);
    let mut by_idx: HashMap<usize, String> = HashMap::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let (name, idx_str) =
            line.rsplit_once(char::is_whitespace)
                .ok_or_else(|| FormatError::InvalidFormat {
                    file: file_name.clone(),
                    line: line_no + 1,
                    reason: "expected '<name> <index>' but found no whitespace".into(),
                })?;

        let idx: usize = idx_str
            .trim()
            .parse()
            .map_err(|_| FormatError::InvalidFormat {
                file: file_name.clone(),
                line: line_no + 1,
                reason: format!("index '{}' is not a valid positive integer", idx_str.trim()),
            })?;

        if idx == 0 {
            return Err(FormatError::InvalidFormat {
                file: file_name.clone(),
                line: line_no + 1,
                reason: "index must be a positive integer (>= 1)".into(),
            });
        }

        let name = normalize_map_name(name);
        if by_idx.insert(idx, name.clone()).is_some() {
            return Err(FormatError::InvalidFormat {
                file: file_name.clone(),
                line: line_no + 1,
                reason: format!("duplicate index {idx}"),
            });
        }
        by_name.insert(name, idx);
    }

    Ok((by_idx, by_name))
}

/// A MatrixMarket directory data source.
///
/// Reads the graph from a directory that contains:
/// - `vertices.txt` — `<node_name> <1-based-index>` mapping
/// - `edges.txt`    — `<label_name> <1-based-index>` mapping
/// - `<n>.txt`      — one MM adjacency matrix per label index
/// # Example
///
/// ```no_run
/// use pathrex::graph::{Graph, InMemory, GraphDecomposition};
/// use pathrex::formats::mm::MatrixMarket;
///
/// let graph = Graph::<InMemory>::try_from(
///     MatrixMarket::from_dir("path/to/graph/dir")
/// ).unwrap();
/// println!("Nodes: {}", graph.num_nodes());
/// ```
pub struct MatrixMarket {
    pub(crate) dir: PathBuf,
    pub(crate) base_iri: Option<String>,
}

impl MatrixMarket {
    /// Create a `MatrixMarket` source that will load from `dir`.
    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            base_iri: None,
        }
    }

    pub fn with_base_iri(mut self, base: impl Into<String>) -> Self {
        self.base_iri = Some(base.into());
        self
    }

    pub(crate) fn mm_path(&self, idx: usize) -> PathBuf {
        self.dir.join(format!("{}.txt", idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_file(dir: &Path, name: &str, content: &str) {
        let mut f = File::create(dir.join(name)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn test_parse_index_map_basic() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "vertices.txt",
            "<Article1> 1\n<Paul_Erdoes> 2\n<1940> 3\n",
        );
        let (by_idx, by_name) = parse_index_map(&tmp.path().join("vertices.txt")).unwrap();
        assert_eq!(by_idx[&1], "Article1");
        assert_eq!(by_idx[&2], "Paul_Erdoes");
        assert_eq!(by_idx[&3], "1940");
        assert_eq!(by_name["Article1"], 1);
        assert_eq!(by_name["Paul_Erdoes"], 2);
    }

    #[test]
    fn test_parse_index_map_preserves_unbracketed_names() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "v.txt", "plain_name 1\n");
        let (by_idx, by_name) = parse_index_map(&tmp.path().join("v.txt")).unwrap();
        assert_eq!(by_idx[&1], "plain_name");
        assert_eq!(by_name["plain_name"], 1);
    }

    #[test]
    fn test_parse_index_map_rejects_zero_index() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "v.txt", "<a> 0\n");
        let err = parse_index_map(&tmp.path().join("v.txt")).unwrap_err();
        assert!(matches!(err, FormatError::InvalidFormat { .. }));
    }

    #[test]
    fn test_parse_index_map_rejects_duplicate_index() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "v.txt", "<a> 1\n<b> 1\n");
        let err = parse_index_map(&tmp.path().join("v.txt")).unwrap_err();
        assert!(matches!(err, FormatError::InvalidFormat { .. }));
    }

    #[test]
    fn test_parse_index_map_empty_lines_ignored() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "edges.txt", "\n<journal> 1\n\n<creator> 2\n");
        let (by_idx, _) = parse_index_map(&tmp.path().join("edges.txt")).unwrap();
        assert_eq!(by_idx.len(), 2);
        assert_eq!(by_idx[&1], "journal");
        assert_eq!(by_idx[&2], "creator");
    }

    #[test]
    fn test_parse_index_map_bad_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "bad.txt", "<foo> notanumber\n");
        let err = parse_index_map(&tmp.path().join("bad.txt")).unwrap_err();
        assert!(
            matches!(err, FormatError::InvalidFormat { .. }),
            "expected InvalidFormat, got {:?}",
            err
        );
    }

    #[test]
    fn test_parse_index_map_missing_whitespace_returns_error() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "bad.txt", "nospacehere\n");
        let err = parse_index_map(&tmp.path().join("bad.txt")).unwrap_err();
        assert!(matches!(err, FormatError::InvalidFormat { .. }));
    }

    #[test]
    fn test_load_nonexistent_mm_file_returns_io_error() {
        let result = load_mm_file("/nonexistent/path/to/file.txt");
        assert!(
            matches!(result, Err(FormatError::Io(_))),
            "expected Io error for missing file, got: {:?}",
            result
        );
    }

    #[test]
    fn test_from_dir_stores_path() {
        let src = MatrixMarket::from_dir("/some/path");
        assert_eq!(src.dir, PathBuf::from("/some/path"));
        assert!(src.base_iri.is_none());
    }

    #[test]
    fn test_with_base_iri() {
        let m = MatrixMarket::from_dir("/x").with_base_iri("http://example.org/");
        assert_eq!(m.dir, PathBuf::from("/x"));
        assert_eq!(m.base_iri.as_deref(), Some("http://example.org/"));
    }

    #[test]
    fn test_apply_base_iri() {
        assert_eq!(apply_base_iri("foo".into(), None), "foo");
        assert_eq!(
            apply_base_iri("Article1".into(), Some("http://example.org/")),
            "http://example.org/Article1"
        );
        assert_eq!(apply_base_iri("foo".into(), Some("")), "foo");
    }

    #[test]
    fn test_mm_path() {
        let src = MatrixMarket::from_dir("/graph");
        assert_eq!(src.mm_path(3), PathBuf::from("/graph/3.txt"));
    }
}
