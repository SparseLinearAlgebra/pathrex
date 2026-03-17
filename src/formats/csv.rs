//! CSV edge iterator for the formats layer.

use std::io::Read;

use csv::StringRecord;

use crate::formats::FormatError;
use crate::graph::{Edge};

#[derive(Debug, Clone)]
pub enum ColumnSpec {
    Index(usize),
    Name(String),
}

#[derive(Debug, Clone)]
pub struct CsvConfig {
    pub source_column: ColumnSpec,
    pub target_column: ColumnSpec,
    pub label_column: ColumnSpec,
    pub has_header: bool,
    pub delimiter: u8,
}

impl Default for CsvConfig {
    fn default() -> Self {
        Self {
            source_column: ColumnSpec::Index(0),
            target_column: ColumnSpec::Index(1),
            label_column: ColumnSpec::Index(2),
            has_header: true,
            delimiter: b',',
        }
    }
}

/// An iterator that reads CSV records and yields `Result<Edge, FormatError>`.
///
/// # Example
///
/// ```no_run
/// use pathrex::formats::csv::Csv;
/// use std::fs::File;
///
/// let file = File::open("edges.csv").unwrap();
/// let iter = Csv::from_reader(file).unwrap();
/// for result in iter {
///     let edge = result.unwrap();
///     println!("{} --{}--> {}", edge.source, edge.label, edge.target);
/// }
/// ```
pub struct Csv<R: Read> {
    records: csv::StringRecordsIntoIter<R>,
    source_idx: usize,
    target_idx: usize,
    label_idx: usize,
}

impl<R: Read> Csv<R> {
    pub fn new(reader: R, config: CsvConfig) -> Result<Self, FormatError> {
        let mut csv_reader = csv::ReaderBuilder::new()
            .has_headers(config.has_header)
            .delimiter(config.delimiter)
            .from_reader(reader);

        let (source_idx, target_idx, label_idx) = if config.has_header {
            let headers = csv_reader.headers()?.clone();
            let resolve = |spec: &ColumnSpec| -> Result<usize, FormatError> {
                match spec {
                    ColumnSpec::Index(i) => Ok(*i),
                    ColumnSpec::Name(name) => headers
                        .iter()
                        .position(|h| h == name)
                        .ok_or_else(|| FormatError::MissingColumn { name: name.clone() }),
                }
            };
            (
                resolve(&config.source_column)?,
                resolve(&config.target_column)?,
                resolve(&config.label_column)?,
            )
        } else {
            let index_only = |spec: &ColumnSpec| -> Result<usize, FormatError> {
                match spec {
                    ColumnSpec::Index(i) => Ok(*i),
                    ColumnSpec::Name(name) => {
                        Err(FormatError::MissingColumn { name: name.clone() })
                    }
                }
            };
            (
                index_only(&config.source_column)?,
                index_only(&config.target_column)?,
                index_only(&config.label_column)?,
            )
        };

        Ok(Self {
            records: csv_reader.into_records(),
            source_idx,
            target_idx,
            label_idx,
        })
    }

    pub fn from_reader(reader: R) -> Result<Self, FormatError> {
        Self::new(reader, CsvConfig::default())
    }

    fn get_field(record: &StringRecord, idx: usize) -> Result<String, FormatError> {
        record
            .get(idx)
            .map(str::to_owned)
            .ok_or_else(|| FormatError::MissingColumn {
                name: format!("index {idx}"),
            })
    }
}

impl<R: Read> Iterator for Csv<R> {
    type Item = Result<Edge, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        let record = match self.records.next()? {
            Ok(r) => r,
            Err(e) => return Some(Err(FormatError::Csv(e))),
        };

        Some((|| {
            let source = Self::get_field(&record, self.source_idx)?;
            let target = Self::get_field(&record, self.target_idx)?;
            let label = Self::get_field(&record, self.label_idx)?;
            Ok(Edge { source, target, label })
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_csv(content: &str) -> Csv<&[u8]> {
        Csv::from_reader(content.as_bytes()).expect("should construct iterator")
    }

    #[test]
    fn test_basic_csv_with_header() {
        let csv = "source,target,label\nA,B,knows\nB,C,likes\n";
        let edges: Vec<_> = make_csv(csv).collect();

        assert_eq!(edges.len(), 2);
        let e0 = edges[0].as_ref().unwrap();
        assert_eq!(e0.source, "A");
        assert_eq!(e0.target, "B");
        assert_eq!(e0.label, "knows");
    }

    #[test]
    fn test_named_columns() {
        let csv = "from,to,rel\nAlice,Bob,knows\n";
        let config = CsvConfig {
            source_column: ColumnSpec::Name("from".to_string()),
            target_column: ColumnSpec::Name("to".to_string()),
            label_column: ColumnSpec::Name("rel".to_string()),
            ..CsvConfig::default()
        };
        let iter = Csv::new(csv.as_bytes(), config).unwrap();
        let edges: Vec<_> = iter.collect();

        assert_eq!(edges.len(), 1);
        let e = edges[0].as_ref().unwrap();
        assert_eq!(e.source, "Alice");
        assert_eq!(e.target, "Bob");
        assert_eq!(e.label, "knows");
    }

    #[test]
    fn test_missing_named_column_returns_error() {
        let csv = "source,target,label\nA,B,knows\n";
        let config = CsvConfig {
            source_column: ColumnSpec::Name("nonexistent".to_string()),
            ..CsvConfig::default()
        };
        let result = Csv::new(csv.as_bytes(), config);
        assert!(
            matches!(result, Err(FormatError::MissingColumn { name }) if name == "nonexistent")
        );
    }

    #[test]
    fn test_custom_delimiter() {
        let csv = "source\ttarget\tlabel\nX\tY\tedge\n";
        let config = CsvConfig {
            delimiter: b'\t',
            ..CsvConfig::default()
        };
        let iter = Csv::new(csv.as_bytes(), config).unwrap();
        let edges: Vec<_> = iter.collect();

        assert_eq!(edges.len(), 1);
        let e = edges[0].as_ref().unwrap();
        assert_eq!(e.source, "X");
        assert_eq!(e.target, "Y");
        assert_eq!(e.label, "edge");
    }

    #[test]
    fn test_no_header_with_index_columns() {
        let csv = "A,B,knows\nC,D,likes\n";
        let config = CsvConfig {
            has_header: false,
            ..CsvConfig::default()
        };
        let iter = Csv::new(csv.as_bytes(), config).unwrap();
        let edges: Vec<_> = iter.collect();

        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].as_ref().unwrap().source, "A");
        assert_eq!(edges[1].as_ref().unwrap().source, "C");
    }

    #[test]
    fn test_empty_csv_yields_no_edges() {
        let csv = "source,target,label\n";
        let edges: Vec<_> = make_csv(csv).collect();
        assert!(edges.is_empty());
    }

    #[test]
    fn test_graph_source_impl() {
        use crate::graph::{GraphBuilder, GraphDecomposition, InMemoryBuilder};

        let csv = "source,target,label\nA,B,knows\nB,C,likes\nC,A,knows\n";
        let iter = Csv::from_reader(csv.as_bytes()).unwrap();
        let graph = InMemoryBuilder::default()
            .load(iter)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(graph.num_nodes(), 3);
    }
}
