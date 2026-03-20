//! Regular Path Query (RPQ) evaluation over edge-labeled graphs.
//! ```rust,ignore
//! use pathrex::sparql::parse_rpq;
//! use pathrex::rpq::{RpqEvaluator, nfarpq::NfaRpqEvaluator};
//!
//! let triple = parse_rpq("SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }")?;
//! let result = NfaRpqEvaluator.evaluate(&triple.subject, &triple.path, &triple.object, &graph)?;
//! ```

pub mod rpqmatrix;

use crate::graph::GraphDecomposition;
use crate::graph::GraphblasVector;
use crate::sparql::ExtractError;
use spargebra::SparqlSyntaxError;
use spargebra::algebra::PropertyPathExpression;
use spargebra::term::TermPattern;
use thiserror::Error;

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

#[derive(Debug)]
pub struct RpqResult {
    pub reachable: GraphblasVector,
}

pub trait RpqEvaluator {
    fn evaluate<G: GraphDecomposition>(
        &self,
        subject: &TermPattern,
        path: &PropertyPathExpression,
        object: &TermPattern,
        graph: &G,
    ) -> Result<RpqResult, RpqError>;
}
