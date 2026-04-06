//! Regular Path Query (RPQ) evaluation over edge-labeled graphs.
//! ```rust,ignore
//! use pathrex::sparql::parse_rpq;
//! use pathrex::rpq::{RpqEvaluator, nfarpq::{NfaRpqEvaluator, NfaRpqResult}};
//!
//! let mut query = parse_rpq(
//!     "BASE <http://example.org/> SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }",
//! )?;
//! query.strip_base("http://example.org/");
//! let result: NfaRpqResult = NfaRpqEvaluator.evaluate(&query, &graph)?;
//! ```

pub mod nfarpq;
pub mod rpqmatrix;

use crate::graph::GraphDecomposition;
use crate::sparql::ExtractError;
use spargebra::SparqlSyntaxError;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Variable(String),
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathExpr {
    Label(String),
    Sequence(Box<PathExpr>, Box<PathExpr>),
    Alternative(Box<PathExpr>, Box<PathExpr>),
    ZeroOrMore(Box<PathExpr>),
    OneOrMore(Box<PathExpr>),
    ZeroOrOne(Box<PathExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpqQuery {
    pub subject: Endpoint,
    pub path: PathExpr,
    pub object: Endpoint,
}

impl RpqQuery {
    /// Strip a base IRI prefix from all IRIs in this query.
    pub fn strip_base(&mut self, base: &str) {
        strip_endpoint(&mut self.subject, base);
        strip_endpoint(&mut self.object, base);
        strip_path(&mut self.path, base);
    }
}

fn strip_endpoint(ep: &mut Endpoint, base: &str) {
    if let Endpoint::Named(s) = ep {
        if s.starts_with(base) {
            *s = s[base.len()..].to_owned();
        }
    }
}

fn strip_path(path: &mut PathExpr, base: &str) {
    match path {
        PathExpr::Label(s) => {
            if s.starts_with(base) {
                *s = s[base.len()..].to_owned();
            }
        }
        PathExpr::Sequence(l, r) | PathExpr::Alternative(l, r) => {
            strip_path(l, base);
            strip_path(r, base);
        }
        PathExpr::ZeroOrMore(inner) | PathExpr::OneOrMore(inner) | PathExpr::ZeroOrOne(inner) => {
            strip_path(inner, base);
        }
    }
}

#[derive(Debug, Error)]
pub enum RpqError {
    #[error("SPARQL syntax error: {0}")]
    Parse(#[from] SparqlSyntaxError),

    #[error("query extraction error: {0}")]
    Extract(#[from] ExtractError),

    #[error("unsupported path expression: {0}")]
    UnsupportedPath(String),

    #[error("label not found in graph: '{0}'")]
    LabelNotFound(String),

    #[error("vertex not found in graph: '{0}'")]
    VertexNotFound(String),

    #[error("GraphBLAS/LAGraph error: {0}")]
    GraphBlas(String),
}

pub trait RpqEvaluator {
    /// Output of this evaluator (e.g. reachable vector vs path matrix + nnz).
    type Result;

    fn evaluate<G: GraphDecomposition>(
        &self,
        query: &RpqQuery,
        graph: &G,
    ) -> Result<Self::Result, RpqError>;
}
