//! SPARQL parsing utilities.
//!
//!
//! # Supported query form
//!
//! SELECT queries with exactly one triple or property path pattern:
//!
//! ```sparql
//! BASE <http://example.org/>
//! SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }
//! ```

use spargebra::algebra::{GraphPattern, PropertyPathExpression};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use spargebra::{Query, SparqlParser, SparqlSyntaxError};
use thiserror::Error;

use crate::rpq::{Endpoint, PathExpr, RpqQuery};

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

/// Parse a SPARQL string and produce an [`RpqQuery`].
pub fn parse_rpq(sparql: &str) -> Result<RpqQuery, RpqParseError> {
    let query = SparqlParser::new().parse_query(sparql)?;
    extract_rpq(&query)
}

/// Extract an [`RpqQuery`] from a parsed spargebra [`Query`].
pub fn extract_rpq(query: &Query) -> Result<RpqQuery, RpqParseError> {
    let pattern = match query {
        Query::Select { pattern, .. } => pattern,
        _ => return Err(ExtractError::NotSelect.into()),
    };

    extract_from_pattern(pattern)
}

fn extract_from_pattern(pattern: &GraphPattern) -> Result<RpqQuery, RpqParseError> {
    match pattern {
        GraphPattern::Path {
            subject,
            path,
            object,
        } => {
            let subject = term_to_endpoint(subject)?;
            let object = term_to_endpoint(object)?;
            let path = path_from_spargebra(path)?;
            Ok(RpqQuery {
                subject,
                path,
                object,
            })
        }

        GraphPattern::Bgp { patterns } => extract_from_bgp(patterns),

        GraphPattern::Project { inner, .. } => extract_from_pattern(inner),
        GraphPattern::Distinct { inner } => extract_from_pattern(inner),
        GraphPattern::Reduced { inner } => extract_from_pattern(inner),
        GraphPattern::Slice { inner, .. } => extract_from_pattern(inner),

        _ => Err(ExtractError::NotSinglePath.into()),
    }
}

/// Extract from a BGP.
///
/// Handles:
/// 1. A single triple `?x <p> ?y` → `PathExpr::Label`.
/// 2. Spargebra's desugared sequence `?x <a>/<b>/<c> ?y` → chain of triples
///    linked by blank-node intermediates, reconstructed as `PathExpr::Sequence`.
fn extract_from_bgp(patterns: &[TriplePattern]) -> Result<RpqQuery, RpqParseError> {
    if patterns.is_empty() {
        return Err(ExtractError::NotSinglePath.into());
    }

    if patterns.len() == 1 {
        let t = &patterns[0];
        let path = match &t.predicate {
            NamedNodePattern::NamedNode(nn) => PathExpr::Label(nn.as_str().to_owned()),
            NamedNodePattern::Variable(_) => return Err(ExtractError::VariablePredicate.into()),
        };
        let subject = term_to_endpoint(&t.subject)?;
        let object = term_to_endpoint(&t.object)?;
        return Ok(RpqQuery {
            subject,
            path,
            object,
        });
    }

    let mut steps: Vec<PathExpr> = Vec::with_capacity(patterns.len());
    for triple in patterns {
        match &triple.predicate {
            NamedNodePattern::NamedNode(nn) => {
                steps.push(PathExpr::Label(nn.as_str().to_owned()));
            }
            NamedNodePattern::Variable(_) => return Err(ExtractError::NotSinglePath.into()),
        }
    }

    for i in 0..patterns.len() - 1 {
        let obj_bn = match &patterns[i].object {
            TermPattern::BlankNode(bn) => bn,
            _ => return Err(ExtractError::NotSinglePath.into()),
        };
        let subj_bn = match &patterns[i + 1].subject {
            TermPattern::BlankNode(bn) => bn,
            _ => return Err(ExtractError::NotSinglePath.into()),
        };
        if obj_bn != subj_bn {
            return Err(ExtractError::NotSinglePath.into());
        }
    }

    let path = steps
        .into_iter()
        .reduce(|acc, step| PathExpr::Sequence(Box::new(acc), Box::new(step)))
        .unwrap();

    let subject = term_to_endpoint(&patterns[0].subject)?;
    let object = term_to_endpoint(&patterns.last().unwrap().object)?;

    Ok(RpqQuery {
        subject,
        path,
        object,
    })
}

fn term_to_endpoint(term: &TermPattern) -> Result<Endpoint, ExtractError> {
    match term {
        TermPattern::Variable(v) => Ok(Endpoint::Variable(v.as_str().to_owned())),
        TermPattern::NamedNode(nn) => Ok(Endpoint::Named(nn.as_str().to_owned())),
        other => {
            let msg = format!("{other}");
            Err(ExtractError::UnsupportedSubject(msg))
        }
    }
}

fn path_from_spargebra(path: &PropertyPathExpression) -> Result<PathExpr, RpqParseError> {
    match path {
        PropertyPathExpression::NamedNode(nn) => Ok(PathExpr::Label(nn.as_str().to_owned())),

        PropertyPathExpression::Sequence(lhs, rhs) => Ok(PathExpr::Sequence(
            Box::new(path_from_spargebra(lhs)?),
            Box::new(path_from_spargebra(rhs)?),
        )),

        PropertyPathExpression::Alternative(lhs, rhs) => Ok(PathExpr::Alternative(
            Box::new(path_from_spargebra(lhs)?),
            Box::new(path_from_spargebra(rhs)?),
        )),

        PropertyPathExpression::ZeroOrMore(inner) => {
            Ok(PathExpr::ZeroOrMore(Box::new(path_from_spargebra(inner)?)))
        }

        PropertyPathExpression::OneOrMore(inner) => {
            Ok(PathExpr::OneOrMore(Box::new(path_from_spargebra(inner)?)))
        }

        PropertyPathExpression::ZeroOrOne(inner) => {
            Ok(PathExpr::ZeroOrOne(Box::new(path_from_spargebra(inner)?)))
        }

        PropertyPathExpression::Reverse(_) => Err(RpqParseError::UnsupportedPath(
            "Reverse paths are not supported".into(),
        )),

        PropertyPathExpression::NegatedPropertySet(_) => Err(RpqParseError::UnsupportedPath(
            "NegatedPropertySet paths are not supported".into(),
        )),
    }
}

/// Combined error for [`parse_rpq`] and [`extract_rpq`].
#[derive(Debug, Error)]
pub enum RpqParseError {
    #[error("SPARQL syntax error: {0}")]
    Syntax(#[from] SparqlSyntaxError),
    #[error("query extraction error: {0}")]
    Extract(#[from] ExtractError),
    #[error("unsupported path expression for RPQ: {0}")]
    UnsupportedPath(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "http://example.org/";

    fn sparql_with_base(body: &str) -> String {
        format!("BASE <{BASE}> {body}")
    }

    fn parse_and_strip(sparql_body: &str) -> RpqQuery {
        let full = sparql_with_base(sparql_body);
        let mut q = parse_rpq(&full).expect("parse_rpq failed");
        q.strip_base(BASE);
        q
    }

    #[test]
    fn test_plain_triple_bgp() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <knows> ?y . }");
        assert!(matches!(q.subject, Endpoint::Variable(_)));
        assert!(matches!(q.object, Endpoint::Variable(_)));
        assert_eq!(q.path, PathExpr::Label("knows".into()));
    }

    #[test]
    fn test_variable_variable_zero_or_more() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <knows>* ?y . }");
        assert!(matches!(q.path, PathExpr::ZeroOrMore(_)));
    }

    #[test]
    fn test_variable_variable_sequence() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <knows>/<likes> ?y . }");
        assert!(matches!(q.path, PathExpr::Sequence(_, _)));
    }

    #[test]
    fn test_named_variable_sequence() {
        let q = parse_and_strip("SELECT ?y WHERE { <alice> <knows>/<likes> ?y . }");
        assert_eq!(q.subject, Endpoint::Named("alice".into()));
        assert!(matches!(q.path, PathExpr::Sequence(_, _)));
    }

    #[test]
    fn test_three_step_sequence() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <a>/<b>/<c> ?y . }");
        match &q.path {
            PathExpr::Sequence(lhs, _) => {
                assert!(matches!(lhs.as_ref(), PathExpr::Sequence(_, _)));
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn test_variable_named_star() {
        let q = parse_and_strip("SELECT ?x WHERE { ?x <knows>* <bob> . }");
        assert_eq!(q.object, Endpoint::Named("bob".into()));
        assert!(matches!(q.path, PathExpr::ZeroOrMore(_)));
    }

    #[test]
    fn test_alternative_path() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <a>|<b> ?y . }");
        assert!(matches!(q.path, PathExpr::Alternative(_, _)));
    }

    #[test]
    fn test_one_or_more() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <knows>+ ?y . }");
        assert!(matches!(q.path, PathExpr::OneOrMore(_)));
    }

    #[test]
    fn test_zero_or_one() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x <knows>? ?y . }");
        assert!(matches!(q.path, PathExpr::ZeroOrOne(_)));
    }

    #[test]
    fn test_complex_path() {
        let q = parse_and_strip("SELECT ?x ?y WHERE { ?x (<a>/<b>)* ?y . }");
        assert!(matches!(q.path, PathExpr::ZeroOrMore(_)));
    }

    #[test]
    fn test_not_select_returns_error() {
        let sparql = sparql_with_base("ASK { ?x <knows> ?y }");
        let r = parse_rpq(&sparql);
        assert!(matches!(
            r,
            Err(RpqParseError::Extract(ExtractError::NotSelect))
        ));
    }

    #[test]
    fn test_multiple_triples_returns_error() {
        let sparql = sparql_with_base("SELECT ?x ?y WHERE { ?x <a> ?z . ?z <b> ?y . }");
        let r = parse_rpq(&sparql);
        assert!(matches!(
            r,
            Err(RpqParseError::Extract(ExtractError::NotSinglePath))
        ));
    }

    #[test]
    fn test_full_iris_before_strip() {
        let sparql = sparql_with_base("SELECT ?x ?y WHERE { ?x <knows> ?y . }");
        let q = parse_rpq(&sparql).unwrap();
        assert_eq!(q.path, PathExpr::Label("http://example.org/knows".into()));
    }

    #[test]
    fn test_strip_base_removes_prefix() {
        let sparql = sparql_with_base("SELECT ?x ?y WHERE { ?x <knows> ?y . }");
        let mut q = parse_rpq(&sparql).unwrap();
        q.strip_base(BASE);
        assert_eq!(q.path, PathExpr::Label("knows".into()));
    }

    #[test]
    fn test_with_prefix_resolves_prefixed_iris() {
        let query = SparqlParser::new()
            .with_prefix("ex", "http://example.org/")
            .unwrap()
            .parse_query("SELECT ?x ?y WHERE { ?x ex:knows/ex:likes ?y . }")
            .expect("parse with prefix failed");
        let mut q = extract_rpq(&query).expect("extract failed");
        q.strip_base(BASE);
        assert!(matches!(q.path, PathExpr::Sequence(_, _)));
    }

    #[test]
    fn test_negated_property_set_rejected() {
        let sparql = sparql_with_base("SELECT ?x ?y WHERE { ?x !(<knows>) ?y . }");
        let r = parse_rpq(&sparql);
        assert!(matches!(r, Err(RpqParseError::UnsupportedPath(_))));
    }
}
