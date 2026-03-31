//! SPARQL parsing and validation utilities.
//!
//! This module provides helpers for parsing SPARQL query strings using the
//! [`spargebra`] crate and extracting the property path triple pattern that
//! pathrex's RPQ evaluators operate on.
//!
//! # Supported query form
//!
//! SELECT queries with a single triple pattern in the
//! WHERE clause are supported:
//!
//! ```sparql
//! SELECT ?x ?y WHERE { ?x <knows> ?y . }
//! SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }
//! SELECT ?x WHERE { <http://example.org/alice> <knows>+ ?x . }
//! ```

use spargebra::algebra::{GraphPattern, PropertyPathExpression};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use spargebra::{Query, SparqlParser, SparqlSyntaxError};
use thiserror::Error;

/// Error returned when extracting a property path triple from a parsed query.
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("expected SELECT query, got a different query form")]
    NotSelect,
    #[error("WHERE clause must contain exactly one triple or property path pattern")]
    NotSinglePath,
    #[error("unsupported subject term: {0}")]
    UnsupportedSubject(String),
    #[error("unsupported object term: {0}")]
    UnsupportedObject(String),
    #[error("predicate in plain triple must be a named node, not a variable")]
    VariablePredicate,
}

pub const DEFAULT_BASE_IRI: &str = "http://example.org/";

/// Parse a SPARQL query string into a [`spargebra::Query`].
///
/// # Errors
///
/// Returns [`SparqlSyntaxError`] if the input is not valid SPARQL 1.1.
pub fn parse_query(sparql: &str) -> Result<Query, SparqlSyntaxError> {
    SparqlParser::new()
        // .with_base_iri(DEFAULT_BASE_IRI)
        // .expect("DEFAULT_BASE_IRI is a valid IRI")
        .parse_query(sparql)
}

/// Extracted triple components from a parsed SPARQL query.
///
/// Holds owned data so callers do not need to keep the [`Query`] alive.
#[derive(Debug, Clone)]
pub struct PathTriple {
    pub subject: TermPattern,
    pub path: PropertyPathExpression,
    pub object: TermPattern,
}

/// Extract the property path triple from a parsed SPARQL [`Query`].
///
/// Validates that the query is a `SELECT` with a single triple or property
/// path pattern in the WHERE clause and returns a [`PathTriple`] with the
/// three components.
pub fn extract_path(query: &Query) -> Result<PathTriple, ExtractError> {
    let pattern = match query {
        Query::Select { pattern, .. } => pattern,
        _ => return Err(ExtractError::NotSelect),
    };

    let triple = extract_path_from_pattern(pattern)?;

    validate_term(&triple.subject, true)?;
    validate_term(&triple.object, false)?;

    Ok(triple)
}

/// Recursively unwrap `GraphPattern` wrappers (Project, Distinct, etc.) to
/// find the single triple or path pattern inside.
fn extract_path_from_pattern(pattern: &GraphPattern) -> Result<PathTriple, ExtractError> {
    match pattern {
        GraphPattern::Path {
            subject,
            path,
            object,
        } => Ok(PathTriple {
            subject: subject.clone(),
            path: path.clone(),
            object: object.clone(),
        }),

        GraphPattern::Bgp { patterns } => extract_from_bgp(patterns),

        GraphPattern::Project { inner, .. } => extract_path_from_pattern(inner),

        GraphPattern::Distinct { inner } => extract_path_from_pattern(inner),
        GraphPattern::Reduced { inner } => extract_path_from_pattern(inner),
        GraphPattern::Slice { inner, .. } => extract_path_from_pattern(inner),

        _ => Err(ExtractError::NotSinglePath),
    }
}

/// Extract a [`PathTriple`] from a BGP's triple patterns.
///
/// Handles two cases:
/// 1. **Single triple** — `?x <knows> ?y` → wraps predicate as
///    [`PropertyPathExpression::NamedNode`].
/// 2. **Desugared sequence path** — spargebra rewrites `?x <a>/<b>/<c> ?y`
///    into a chain of triples linked by blank-node intermediates:
///    `?x <a> _:b0 . _:b0 <b> _:b1 . _:b1 <c> ?y`.
///    We detect this pattern and reconstruct a
///    [`PropertyPathExpression::Sequence`].
fn extract_from_bgp(patterns: &[TriplePattern]) -> Result<PathTriple, ExtractError> {
    if patterns.is_empty() {
        return Err(ExtractError::NotSinglePath);
    }
    if patterns.len() == 1 {
        return bgp_triple_to_path_triple(&patterns[0]);
    }

    let mut steps: Vec<PropertyPathExpression> = Vec::with_capacity(patterns.len());
    for triple in patterns {
        match &triple.predicate {
            NamedNodePattern::NamedNode(nn) => {
                steps.push(PropertyPathExpression::NamedNode(nn.clone()));
            }
            NamedNodePattern::Variable(_) => return Err(ExtractError::NotSinglePath),
        }
    }

    for i in 0..patterns.len() - 1 {
        let obj_bn = match &patterns[i].object {
            TermPattern::BlankNode(bn) => bn,
            _ => return Err(ExtractError::NotSinglePath),
        };
        let subj_bn = match &patterns[i + 1].subject {
            TermPattern::BlankNode(bn) => bn,
            _ => return Err(ExtractError::NotSinglePath),
        };
        if obj_bn != subj_bn {
            return Err(ExtractError::NotSinglePath);
        }
    }

    let path = steps
        .into_iter()
        .reduce(|acc, step| PropertyPathExpression::Sequence(Box::new(acc), Box::new(step)))
        .unwrap();

    Ok(PathTriple {
        subject: patterns[0].subject.clone(),
        path,
        object: patterns.last().unwrap().object.clone(),
    })
}

/// Convert a plain BGP [`TriplePattern`] into a [`PathTriple`] by wrapping
/// the predicate as a [`PropertyPathExpression::NamedNode`].
fn bgp_triple_to_path_triple(triple: &TriplePattern) -> Result<PathTriple, ExtractError> {
    let path = match &triple.predicate {
        NamedNodePattern::NamedNode(nn) => PropertyPathExpression::NamedNode(nn.clone()),
        NamedNodePattern::Variable(_) => return Err(ExtractError::VariablePredicate),
    };
    Ok(PathTriple {
        subject: triple.subject.clone(),
        path,
        object: triple.object.clone(),
    })
}

/// Validate that a [`TermPattern`] is a supported vertex form.
fn validate_term(term: &TermPattern, is_subject: bool) -> Result<(), ExtractError> {
    match term {
        TermPattern::Variable(_) | TermPattern::NamedNode(_) => Ok(()),
        other => {
            let msg = format!("{other}");
            if is_subject {
                Err(ExtractError::UnsupportedSubject(msg))
            } else {
                Err(ExtractError::UnsupportedObject(msg))
            }
        }
    }
}

pub fn parse_rpq(sparql: &str) -> Result<PathTriple, RpqParseError> {
    let query = parse_query(sparql)?;
    let triple = extract_path(&query)?;
    Ok(triple)
}

/// Combined error for [`parse_rpq`].
#[derive(Debug, Error)]
pub enum RpqParseError {
    #[error("SPARQL syntax error: {0}")]
    Syntax(#[from] SparqlSyntaxError),
    #[error("query extraction error: {0}")]
    Extract(#[from] ExtractError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use spargebra::algebra::PropertyPathExpression;
    use spargebra::term::TermPattern;

    pub const DEFAULT_BASE_IRI: &str = "BASE <http://example.org/>";

    fn parse_and_extract(sparql: &str) -> PathTriple {
        let q = format!("{DEFAULT_BASE_IRI} {sparql}");
        parse_rpq(&q).expect("parse_rpq failed")
    }

    #[test]
    fn test_plain_triple_bgp() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <knows> ?y . }");
        assert!(matches!(triple.subject, TermPattern::Variable(_)));
        assert!(matches!(triple.object, TermPattern::Variable(_)));
        assert!(matches!(triple.path, PropertyPathExpression::NamedNode(_)));
    }

    #[test]
    fn test_variable_variable_zero_or_more() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <knows>* ?y . }");
        assert!(matches!(triple.subject, TermPattern::Variable(_)));
        assert!(matches!(triple.object, TermPattern::Variable(_)));
        assert!(matches!(triple.path, PropertyPathExpression::ZeroOrMore(_)));
    }

    #[test]
    fn test_variable_variable_sequence() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <knows>/<likes> ?y . }");
        assert!(matches!(triple.subject, TermPattern::Variable(_)));
        assert!(matches!(triple.object, TermPattern::Variable(_)));
        assert!(matches!(
            triple.path,
            PropertyPathExpression::Sequence(_, _)
        ));
    }

    #[test]
    fn test_named_variable_sequence() {
        let triple = parse_and_extract("SELECT ?y WHERE { <alice> <knows>/<likes> ?y . }");
        assert!(matches!(triple.subject, TermPattern::NamedNode(_)));
        assert!(matches!(triple.object, TermPattern::Variable(_)));
        assert!(matches!(
            triple.path,
            PropertyPathExpression::Sequence(_, _)
        ));
    }

    #[test]
    fn test_three_step_sequence() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <a>/<b>/<c> ?y . }");
        assert!(matches!(triple.subject, TermPattern::Variable(_)));
        assert!(matches!(triple.object, TermPattern::Variable(_)));
        match &triple.path {
            PropertyPathExpression::Sequence(lhs, _rhs) => {
                assert!(matches!(
                    lhs.as_ref(),
                    PropertyPathExpression::Sequence(_, _)
                ));
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn test_variable_named_star() {
        let triple = parse_and_extract("SELECT ?x WHERE { ?x <knows>* <bob> . }");
        assert!(matches!(triple.subject, TermPattern::Variable(_)));
        assert!(matches!(triple.object, TermPattern::NamedNode(_)));
        assert!(matches!(triple.path, PropertyPathExpression::ZeroOrMore(_)));
    }

    #[test]
    fn test_alternative_path() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <a>|<b> ?y . }");
        assert!(matches!(
            triple.path,
            PropertyPathExpression::Alternative(_, _)
        ));
    }

    #[test]
    fn test_one_or_more() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <knows>+ ?y . }");
        assert!(matches!(triple.path, PropertyPathExpression::OneOrMore(_)));
    }

    #[test]
    fn test_zero_or_one() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <knows>? ?y . }");
        assert!(matches!(triple.path, PropertyPathExpression::ZeroOrOne(_)));
    }

    #[test]
    fn test_complex_path() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x (<a>/<b>)* ?y . }");
        assert!(matches!(triple.path, PropertyPathExpression::ZeroOrMore(_)));
    }

    #[test]
    fn test_not_select_returns_error() {
        let sparql = format!("{DEFAULT_BASE_IRI} ASK {{ ?x <knows> ?y }}");
        let query = parse_query(&sparql).expect("parse failed");
        let result = extract_path(&query);
        assert!(matches!(result, Err(ExtractError::NotSelect)));
    }

    #[test]
    fn test_multiple_triples_returns_error() {
        let sparql = format!("{DEFAULT_BASE_IRI} SELECT ?x ?y WHERE {{ ?x <a> ?z . ?z <b> ?y . }}");
        let result = parse_rpq(&sparql);
        assert!(matches!(
            result,
            Err(RpqParseError::Extract(ExtractError::NotSinglePath))
        ));
    }

    #[test]
    fn test_default_base_iri_resolves_relative_iris() {
        let triple = parse_and_extract("SELECT ?x ?y WHERE { ?x <knows> ?y . }");
        if let PropertyPathExpression::NamedNode(nn) = &triple.path {
            assert_eq!(nn.as_str(), "http://example.org/knows");
        } else {
            panic!("expected NamedNode path");
        }
    }

    #[test]
    fn test_with_prefix_resolves_prefixed_iris() {
        let query = SparqlParser::new()
            .with_prefix("ex", "http://example.org/")
            .unwrap()
            .parse_query("SELECT ?x ?y WHERE { ?x ex:knows/ex:likes ?y . }")
            .expect("parse with prefix failed");
        let triple = extract_path(&query).expect("extract failed");
        assert!(matches!(
            triple.path,
            PropertyPathExpression::Sequence(_, _)
        ));
    }
}
