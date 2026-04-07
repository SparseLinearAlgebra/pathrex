use std::{cmp::Ordering, fmt::Display, str::FromStr};
use egg::{Id, define_language};
use egg::*;

#[derive(Clone, Hash, Ord, Eq, PartialEq, PartialOrd, Debug)]
pub struct LabelMeta {
    pub name: String,
    pub nvals: usize,
    pub rreduce_nvals: usize,
    pub creduce_nvals: usize,
}

impl FromStr for LabelMeta {
    type Err = <usize as FromStr>::Err;
    // This is needed for the builtin egg parser. Only used in tests.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(LabelMeta {
            name: "-".to_string(),
            nvals: s.parse()?,
            rreduce_nvals: s.parse()?,
            creduce_nvals: s.parse()?,
        })
    }
}

impl Display for LabelMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.name, self.nvals)
    }
}

define_language! {
pub(super) enum RpqPlan {
    Label(LabelMeta),
    "/" = Seq([egg::Id; 2]),
    "|" = Alt([egg::Id; 2]),
    "*" = Star([egg::Id; 1]),
    "l*" = LStar([egg::Id; 2]),
    "*r" = RStar([egg::Id; 2]),
} }

pub fn make_rules() -> Vec<egg::Rewrite<RpqPlan, ()>> {
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

// pub fn make_stupid_rules() -> Vec<egg::Rewrite<Plan, ()>> {
//     vec![
//         rewrite!("assoc-sec-1"; "(/ ?a (/ ?b ?c))" => "(/ (/ ?a ?b) ?c)"),
//         rewrite!("assoc-sec-2"; "(/ (/ ?a ?b) ?c)" => "(/ ?a (/ ?b ?c))"),
//         rewrite!("commute-alt"; "(| ?a ?b)" => "(| ?b ?a)"),
//         rewrite!("assoc-alt"; "(| ?a (| ?b ?c))" => "(| (| ?a ?b) ?c)"),
//     ]
// }

pub struct RandomCostFn;
impl CostFunction<RpqPlan> for RandomCostFn {
    type Cost = f64;
    fn cost<C>(&mut self, _enode: &RpqPlan, _costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        rand::random()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardCost {
    pub score: f64,
    pub nnz: f64,
    pub nnz_r: f64,
    pub nnz_c: f64,
}

impl Eq for CardCost {}

impl PartialOrd for CardCost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CardCost {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.score.total_cmp(&other.score) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.nnz.total_cmp(&other.nnz) {
            Ordering::Equal => {}
            ord => return ord,
        }
        match self.nnz_r.total_cmp(&other.nnz_r) {
            Ordering::Equal => {}
            ord => return ord,
        }
        self.nnz_c.total_cmp(&other.nnz_c)
    }
}

pub struct CardinalityCostFn {
    pub n: f64,
    pub star_penalty: f64,
    pub lr_multiplier: f64,
}

// TODO: check value intervals
impl CostFunction<RpqPlan> for CardinalityCostFn {
    type Cost = CardCost;

    fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
    where
        C: FnMut(Id) -> Self::Cost,
    {
        match enode {
            RpqPlan::Label(meta) => CardCost {
                score: 0.0,
                nnz: meta.nvals as f64,
                nnz_r: meta.rreduce_nvals as f64,
                nnz_c: meta.creduce_nvals as f64,
            },

            RpqPlan::Seq([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let op_cost = (ca.nnz * cb.nnz) / denom;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::Alt([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let overlap = (ca.nnz * cb.nnz) / (self.n * self.n);
                let op_cost = ca.nnz + cb.nnz - overlap;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = (ca.nnz + cb.nnz - overlap).min(self.n * self.n).max(0.0);

                let nnz_r_est = (ca.nnz_r + cb.nnz_r - (ca.nnz_r * cb.nnz_r) / self.n)
                    .min(self.n)
                    .max(0.0);

                let nnz_c_est = (ca.nnz_c + cb.nnz_c - (ca.nnz_c * cb.nnz_c) / self.n)
                    .min(self.n)
                    .max(0.0);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: nnz_r_est,
                    nnz_c: nnz_c_est,
                }
            }

            RpqPlan::Star([a]) => {
                let ca = costs(*a);

                let penalty = self.star_penalty * ca.nnz.max(1.0);
                let score = ca.score + penalty;

                CardCost {
                    score,
                    nnz: self.n * self.n,
                    nnz_r: self.n,
                    nnz_c: self.n,
                }
            }

            RpqPlan::LStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;
                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }

            RpqPlan::RStar([a, b]) => {
                let ca = costs(*a);
                let cb = costs(*b);

                let denom = ca.nnz_r.max(cb.nnz_c).max(1.0);
                let base = (ca.nnz * cb.nnz) / denom;

                let op_cost = self.lr_multiplier * base;
                let score = ca.score + cb.score + op_cost;

                let nnz_est = self.lr_multiplier * ca.nnz * cb.nnz / (self.n * self.n);

                CardCost {
                    score,
                    nnz: nnz_est,
                    nnz_r: ca.nnz_r.min(self.n), // TODO: better reduce estimators
                    nnz_c: cb.nnz_c.min(self.n), // TODO: better reduce estimators
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    pub struct CostFn;
    impl CostFunction<RpqPlan> for CostFn {
        type Cost = f64;
        fn cost<C>(&mut self, enode: &RpqPlan, mut costs: C) -> Self::Cost
        where
            C: FnMut(Id) -> Self::Cost,
        {
            match enode {
                RpqPlan::Label(meta) => meta.nvals as f64,
                RpqPlan::Seq(args) => costs(args[0]).min(costs(args[1])).powf(1.1),
                RpqPlan::Alt(args) => costs(args[0]).min(costs(args[1])).powf(1.1),
                RpqPlan::Star(args) => costs(args[0]).powi(3),
                RpqPlan::LStar(args) => costs(args[0]) * costs(args[1]),
                RpqPlan::RStar(args) => costs(args[0]) * costs(args[1]),
            }
        }
    }

    fn test_simplify(s: String) -> String {
        let expr = s.parse().unwrap();
        let runner = Runner::default().with_expr(&expr).run(&make_rules());
        let cost_func = CostFn;
        let extractor = Extractor::new(&runner.egraph, cost_func);
        extractor.find_best(runner.roots[0]).1.to_string()
    }

    #[test]
    fn test_basic_seq_1() {
        expect![[r#"(/ "(-, 1)" (/ "(-, 2)" (/ "(-, 3)" "(-, 4)")))"#]]
            .assert_eq(test_simplify("(/ (/ (/ 1 2) 3) 4)".to_string()).as_str());
    }

    #[test]
    fn test_basic_seq_2() {
        expect![[r#"(/ "(-, 4)" (/ "(-, 3)" (/ "(-, 2)" "(-, 1)")))"#]]
            .assert_eq(test_simplify("(/ (/ (/ 4 3) 2) 1)".to_string()).as_str());
    }

    #[test]
    fn test_basic_alt_1() {
        expect![[r#"(| "(-, 2)" (| "(-, 4)" (| "(-, 1)" "(-, 3)")))"#]]
            .assert_eq(test_simplify("(| (| (| 1 2) 3) 4)".to_string()).as_str());
    }

    #[test]
    fn test_basic_alt_2() {
        expect![[r#"(| "(-, 3)" (| "(-, 1)" (| "(-, 4)" "(-, 2)")))"#]]
            .assert_eq(test_simplify("(| (| (| 4 3) 2) 1)".to_string()).as_str());
    }
}
