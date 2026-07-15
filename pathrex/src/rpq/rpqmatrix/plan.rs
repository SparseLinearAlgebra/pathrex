use std::{fmt::Display, str::FromStr};

use egg::{Id, define_language, rewrite};

#[derive(Clone, Hash, Ord, Eq, PartialEq, PartialOrd, Debug)]
pub(super) struct LabelMeta {
    pub name: String,
    pub nvals: usize,
    pub nonzero_rows: usize,
    pub nonzero_cols: usize,
}

impl FromStr for LabelMeta {
    type Err = <usize as FromStr>::Err;
    // This is needed for the builtin egg parser. Only used in tests.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(LabelMeta {
            name: "-".to_string(),
            nvals: s.parse()?,
            nonzero_rows: s.parse()?,
            nonzero_cols: s.parse()?,
        })
    }
}

impl Display for LabelMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.name, self.nvals)
    }
}

define_language! {
    pub enum RpqPlan {
        Label(LabelMeta),
        NamedVertex(String),
        "/" = Seq([Id; 2]),
        "|" = Alt([Id; 2]),
        "*" = Star([Id; 1]),
        "l*" = LStar([Id; 2]),
        "*r" = RStar([Id; 2]),
    }
}

pub(super) fn make_rules() -> Vec<egg::Rewrite<RpqPlan, ()>> {
    vec![
        rewrite!("assoc-sec-1"; "(/ ?a (/ ?b ?c))" => "(/ (/ ?a ?b) ?c)"),
        rewrite!("assoc-sec-2"; "(/ (/ ?a ?b) ?c)" => "(/ ?a (/ ?b ?c))"),
        rewrite!("commute-alt"; "(| ?a ?b)" => "(| ?b ?a)"),
        rewrite!("assoc-alt"; "(| ?a (| ?b ?c))" => "(| (| ?a ?b) ?c)"),
        rewrite!("distribute-1"; "(/ ?a (| ?b ?c))" => "(| (/ ?a ?b) (/ ?a ?c))"),
        rewrite!("distribute-2"; "(/ (| ?a ?b) ?c)" => "(| (/ ?a ?c) (/ ?b ?c))"),
        rewrite!("distribute-3"; "(| (/ ?a ?b) (/ ?a ?c))" => "(/ ?a (| ?b ?c))"),
        rewrite!("distribute-4"; "(| (/ ?a ?c) (/ ?b ?c))" => "(/ (| ?a ?b) ?c)"),
        rewrite!("build-lstar"; "(/ (* ?a) ?b)" => "(l* ?a ?b)"),
        rewrite!("build-rstar"; "(/ ?a (* ?b))" => "(*r ?a ?b)"),
    ]
}
