//! Frozen linear regression on complete sequence plans.
//! Exhaustive parenthesization search; no plans are extracted by other models.
//! No random scenarios, online updates, or adaptive stopping are used.
use std::collections::{HashMap, HashSet};

use egg::{Id, Language, RecExpr};

use super::{
    cost::metaac_matmul_nnz,
    learning::{LinearCostModel, LinearFeatures},
    plan::RpqPlan,
};
use crate::rpq::RpqError;

pub(super) struct LinearCandidate {
    pub expr: RecExpr<RpqPlan>,
    pub features: LinearFeatures,
}

// C_9 = 4862 complete trees. The bound includes endpoint-selector matrices.
// Reject longer chains instead of silently changing the search/training space.
pub(super) const MAX_CHAIN_FACTORS: usize = 10;

fn validate_chain(expr: &RecExpr<RpqPlan>) -> Result<(), RpqError> {
    if expr.is_empty()
        || expr.as_ref().iter().any(|node| {
            !matches!(
                node,
                RpqPlan::Label(_) | RpqPlan::NamedVertex(_) | RpqPlan::Seq(_)
            )
        })
    {
        return Err(RpqError::UnsupportedPath(
            "linear regression currently supports label sequences and fixed endpoints; unions and closures are outside this experiment".into()));
    }
    Ok(())
}

/// Both training and inference use the same pre-execution analytical estimates.
/// Each reachable native operation is charged once, including shared DAG nodes.
fn plan_features(plan: &RecExpr<RpqPlan>, n: usize) -> Result<LinearFeatures, RpqError> {
    validate_chain(plan)?;
    let mut reachable = vec![false; plan.len()];
    reachable[plan.len() - 1] = true;
    for i in (0..plan.len()).rev() {
        if reachable[i] {
            for child in plan[Id::from(i)].children() {
                reachable[usize::from(*child)] = true;
            }
        }
    }
    // [nnz, active rows, active columns], using the same Seq cardinality
    // formulas as Hybrid/MetaAC. No other model's score or closure penalties.
    let mut estimates = vec![[0.0; 3]; plan.len()];
    let n = n as f64;
    let mut features = LinearFeatures {
        work: 0.0,
        input_nnz: 0.0,
    };
    for (i, node) in plan.as_ref().iter().enumerate() {
        if !reachable[i] {
            continue;
        }
        estimates[i] = match node {
            RpqPlan::Label(meta) => [
                meta.nvals as f64,
                meta.nonzero_rows as f64,
                meta.nonzero_cols as f64,
            ],
            RpqPlan::NamedVertex(_) => [1.0, 1.0, 1.0],
            RpqPlan::Seq([a, b]) => {
                let left = estimates[usize::from(*a)];
                let right = estimates[usize::from(*b)];
                features.work += left[0] * right[0] / left[1].max(right[2]).max(1.0);
                features.input_nnz += left[0] + right[0];
                [
                    metaac_matmul_nnz(left[0], right[0], n),
                    left[1].min(n),
                    right[2].min(n),
                ]
            }
            _ => unreachable!("validate_chain rejects non-sequence operators"),
        };
    }
    if !features.valid() {
        return Err(RpqError::UnsupportedPath(
            "nonfinite linear plan features".into(),
        ));
    }
    Ok(features)
}

/// Flatten the root in operand order, expanding shared subexpressions into
/// occurrences. Bound expansion before allocating any parenthesization table.
fn chain_factors(expr: &RecExpr<RpqPlan>) -> Result<Vec<RpqPlan>, RpqError> {
    validate_chain(expr)?;
    let mut pending = vec![Id::from(expr.len() - 1)];
    let mut factors = Vec::new();
    while let Some(id) = pending.pop() {
        match &expr[id] {
            RpqPlan::Seq([left, right]) => {
                pending.push(*right);
                pending.push(*left);
            }
            leaf => {
                if factors.len() == MAX_CHAIN_FACTORS {
                    return Err(RpqError::UnsupportedPath(format!(
                        "linear regression exhaustively searches at most {MAX_CHAIN_FACTORS} matrix factors, including fixed endpoint selectors"
                    )));
                }
                factors.push(leaf.clone());
            }
        }
    }
    Ok(factors)
}

/// Copy reachable nodes, interning identical operations to match the native
/// executor's memoized DAG semantics. Never identify merely equivalent trees.
fn append_plan(
    source: &RecExpr<RpqPlan>,
    target: &mut RecExpr<RpqPlan>,
    interned: &mut HashMap<RpqPlan, Id>,
) -> Id {
    fn copy(
        id: Id,
        source: &RecExpr<RpqPlan>,
        target: &mut RecExpr<RpqPlan>,
        interned: &mut HashMap<RpqPlan, Id>,
        ids: &mut [Option<Id>],
    ) -> Id {
        if let Some(result) = ids[usize::from(id)] {
            return result;
        }
        // Child order, rather than source allocation order, makes equal trees
        // identical even when the subject selector was allocated after the path.
        let node = source[id]
            .clone()
            .map_children(|child| copy(child, source, target, interned, ids));
        let result = *interned
            .entry(node.clone())
            .or_insert_with(|| target.add(node));
        ids[usize::from(id)] = Some(result);
        result
    }
    copy(
        Id::from(source.len() - 1),
        source,
        target,
        interned,
        &mut vec![None; source.len()],
    )
}

fn concatenate(left: &RecExpr<RpqPlan>, right: &RecExpr<RpqPlan>) -> RecExpr<RpqPlan> {
    let mut plan = RecExpr::default();
    let mut interned = HashMap::new();
    let a = append_plan(left, &mut plan, &mut interned);
    let b = append_plan(right, &mut plan, &mut interned);
    plan.add(RpqPlan::Seq([a, b]));
    plan
}

/// Enumerate every binary parenthesization of the ordered factors. Keep all
/// subplans: cardinality estimates depend on their shape, and OLS coefficients
/// may be negative, so pruning to one cheapest subplan is not justified.
/// The canonical original plan is first, for deterministic ties and fallback.
pub(super) fn candidates(
    original: RecExpr<RpqPlan>,
    n: usize,
) -> Result<Vec<LinearCandidate>, RpqError> {
    let factors = chain_factors(&original)?;
    let count = factors.len();
    let mut table = vec![vec![Vec::<RecExpr<RpqPlan>>::new(); count + 1]; count];
    for (i, factor) in factors.into_iter().enumerate() {
        let mut leaf = RecExpr::default();
        leaf.add(factor);
        table[i][i + 1].push(leaf);
    }
    for length in 2..=count {
        for begin in 0..=count - length {
            let end = begin + length;
            let mut plans = Vec::new();
            for (split, suffixes) in table.iter().enumerate().take(end).skip(begin + 1) {
                for left in &table[begin][split] {
                    for right in &suffixes[end] {
                        plans.push(concatenate(left, right));
                    }
                }
            }
            table[begin][end] = plans;
        }
    }
    let mut canonical_original = RecExpr::default();
    append_plan(&original, &mut canonical_original, &mut HashMap::new());
    let mut seen = HashSet::from([canonical_original.clone()]);
    let mut plans = vec![canonical_original];
    for plan in std::mem::take(&mut table[0][count]) {
        if seen.insert(plan.clone()) {
            plans.push(plan);
        }
    }
    plans
        .into_iter()
        .map(|expr| {
            let features = plan_features(&expr, n)?;
            Ok(LinearCandidate { expr, features })
        })
        .collect()
}

pub(super) fn select_plan(
    mut candidates: Vec<LinearCandidate>,
    model: &LinearCostModel,
) -> RecExpr<RpqPlan> {
    let mut best = 0;
    let mut best_time = f64::INFINITY;
    for (index, candidate) in candidates.iter().enumerate() {
        // Invalid extrapolation in any candidate makes the whole comparison
        // unreliable. Keep the original; do not clip or modify OLS weights.
        let Some(time) = model.predict(candidate.features) else {
            return candidates.swap_remove(0).expr;
        };
        if time < best_time {
            best = index;
            best_time = time;
        }
    }
    candidates.swap_remove(best).expr
}

#[cfg(test)]
mod tests {
    use super::super::plan::LabelMeta;
    use super::*;

    fn hybrid(n: usize) -> super::super::cost::HybridCostFn {
        super::super::cost::HybridCostFn {
            n: n as f64,
            star_penalty: 50.0,
            lr_multiplier: 5.0,
        }
    }

    fn chain(length: usize) -> RecExpr<RpqPlan> {
        let mut expr = RecExpr::default();
        let mut root = expr.add(RpqPlan::Label(LabelMeta {
            name: "0".into(),
            nvals: 1,
            nonzero_rows: 1,
            nonzero_cols: 1,
        }));
        for i in 1..length {
            let next = expr.add(RpqPlan::Label(LabelMeta {
                name: i.to_string(),
                nvals: i + 1,
                nonzero_rows: 1,
                nonzero_cols: 1,
            }));
            root = expr.add(RpqPlan::Seq([root, next]));
        }
        expr
    }

    #[test]
    fn exhaustive_search_covers_all_catalan_trees_and_preserves_operand_order() {
        for (length, expected) in [1, 1, 2, 5, 14, 42, 132, 429, 1430, 4862]
            .into_iter()
            .enumerate()
        {
            let original = chain(length + 1);
            let factors = chain_factors(&original).unwrap();
            let bank = candidates(original.clone(), 100).unwrap();
            assert_eq!(bank.len(), expected, "{} factors", length + 1);
            assert_eq!(bank[0].expr, original);
            let unique = bank.iter().map(|c| &c.expr).collect::<HashSet<_>>();
            assert_eq!(unique.len(), expected);
            for candidate in bank {
                assert_eq!(chain_factors(&candidate.expr).unwrap(), factors);
            }
        }
    }

    #[test]
    fn four_factors_have_exactly_the_five_expected_associations() {
        let original = "(/ (/ (/ 1 2) 3) 4)".parse().unwrap();
        let actual = candidates(original, 100)
            .unwrap()
            .into_iter()
            .map(|c| c.expr)
            .collect::<HashSet<_>>();
        let expected = [
            "(/ (/ (/ 1 2) 3) 4)",
            "(/ (/ 1 (/ 2 3)) 4)",
            "(/ (/ 1 2) (/ 3 4))",
            "(/ 1 (/ (/ 2 3) 4))",
            "(/ 1 (/ 2 (/ 3 4)))",
        ]
        .map(|s| s.parse::<RecExpr<RpqPlan>>().unwrap())
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(actual, expected);
        for original in &actual {
            let again = candidates(original.clone(), 100)
                .unwrap()
                .into_iter()
                .map(|c| c.expr)
                .collect::<HashSet<_>>();
            assert_eq!(again, expected);
        }
    }

    #[test]
    fn fitted_cost_can_choose_a_plan_outside_the_former_three_source_bank() {
        use super::super::{cost::PangHybridCostFn, learning::LinearObservation, plan::make_rules};
        use egg::{Extractor, Runner};

        // Synthetic features/targets recover t = 1 + S by ordinary least squares.
        // This checks search expressiveness, not real execution performance.
        let observations =
            [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(work, input_nnz)| {
                LinearObservation {
                    features: LinearFeatures { work, input_nnz },
                    elapsed_us: 1.0 + input_nnz,
                }
            });
        let model = LinearCostModel::fit(&observations).unwrap();
        let mut leaves = Vec::new();
        for (i, (nvals, nonzero_rows, nonzero_cols)) in [
            (3533, 50, 96),
            (1365, 33, 65),
            (6021, 82, 88),
            (616, 52, 18),
        ]
        .into_iter()
        .enumerate()
        {
            let mut leaf = RecExpr::default();
            leaf.add(RpqPlan::Label(LabelMeta {
                name: i.to_string(),
                nvals,
                nonzero_rows,
                nonzero_cols,
            }));
            leaves.push(leaf);
        }
        let ab = concatenate(&leaves[0], &leaves[1]);
        let cd = concatenate(&leaves[2], &leaves[3]);
        let expected = concatenate(&ab, &cd);
        let original = concatenate(&concatenate(&ab, &leaves[2]), &leaves[3]);
        let bank = candidates(original.clone(), 100).unwrap();
        let winner = select_plan(bank, &model);
        assert_eq!(winner, expected);
        assert_ne!(winner, original);

        let runner = Runner::<RpqPlan, ()>::default()
            .with_expr(&original)
            .run(&make_rules());
        let hybrid_plan = Extractor::new(&runner.egraph, hybrid(100))
            .find_best(runner.roots[0])
            .1;
        let pang_plan = Extractor::new(&runner.egraph, PangHybridCostFn::new(100.0))
            .find_best(runner.roots[0])
            .1;
        assert_ne!(winner, hybrid_plan);
        assert_ne!(winner, pang_plan);
        for baseline in [&original, &hybrid_plan, &pang_plan] {
            assert!(
                model.predict(plan_features(&winner, 100).unwrap()).unwrap()
                    < model
                        .predict(plan_features(baseline, 100).unwrap())
                        .unwrap()
            );
        }

        // Negative OLS coefficients are legal: still search complete plans,
        // even when a seemingly worse subplan has a better final prediction.
        let negative = LinearCostModel::from_coefficients([1e9, -1.0, -1.0]).unwrap();
        let bank = candidates(original.clone(), 100).unwrap();
        let expected = bank
            .iter()
            .max_by(|a, b| {
                (a.features.work + a.features.input_nnz)
                    .total_cmp(&(b.features.work + b.features.input_nnz))
            })
            .unwrap()
            .expr
            .clone();
        assert_eq!(select_plan(bank, &negative), expected);
    }

    #[test]
    fn search_bounds_include_endpoints_and_expanded_shared_occurrences() {
        let mut boundary = chain(MAX_CHAIN_FACTORS);
        let selector = boundary.add(RpqPlan::NamedVertex("endpoint".into()));
        boundary.add(RpqPlan::Seq([Id::from(boundary.len() - 2), selector]));
        let error = candidates(boundary, 100).err().unwrap();
        assert!(matches!(error, RpqError::UnsupportedPath(message)
            if message.contains("10 matrix factors") && message.contains("endpoint")));
        assert!(candidates(chain(MAX_CHAIN_FACTORS + 1), 100).is_err());

        // A tiny DAG can represent an exponentially long chain.
        let mut shared = chain(1);
        for _ in 0..4 {
            let root = Id::from(shared.len() - 1);
            shared.add(RpqPlan::Seq([root, root]));
        }
        assert!(candidates(shared, 100).is_err());
    }

    #[test]
    fn repeated_subplans_are_shared_and_unreachable_nodes_removed() {
        let mut original = RecExpr::default();
        let a = original.add(RpqPlan::Label(LabelMeta {
            name: "a".into(),
            nvals: 10,
            nonzero_rows: 5,
            nonzero_cols: 5,
        }));
        let b = original.add(RpqPlan::Label(LabelMeta {
            name: "b".into(),
            nvals: 20,
            nonzero_rows: 5,
            nonzero_cols: 5,
        }));
        let ab1 = original.add(RpqPlan::Seq([a, b]));
        original.add(RpqPlan::Seq([a, a])); // unreachable
        let ab2 = original.add(RpqPlan::Seq([a, b])); // same operation
        original.add(RpqPlan::Seq([ab1, ab2]));
        let bank = candidates(original, 100).unwrap();
        assert_eq!(bank.len(), 5);
        assert_eq!(bank[0].expr.len(), 4); // a, b, shared a*b, root
        assert_eq!(bank[0].features, plan_features(&bank[0].expr, 100).unwrap());
    }

    #[test]
    fn shared_operations_are_charged_once_and_unreachable_nodes_are_ignored() {
        let mut expr = RecExpr::default();
        let a = expr.add(RpqPlan::Label(LabelMeta {
            name: "a".into(),
            nvals: 10,
            nonzero_rows: 5,
            nonzero_cols: 5,
        }));
        let b = expr.add(RpqPlan::Label(LabelMeta {
            name: "b".into(),
            nvals: 20,
            nonzero_rows: 5,
            nonzero_cols: 5,
        }));
        let shared = expr.add(RpqPlan::Seq([a, b]));
        let ab_nnz = metaac_matmul_nnz(10.0, 20.0, 100.0);
        expr.add(RpqPlan::Seq([a, a])); // unreachable
        expr.add(RpqPlan::Seq([shared, shared]));
        let features = plan_features(&expr, 100).unwrap();
        assert!((features.work - (40.0 + ab_nnz * ab_nnz / 5.0)).abs() < 1e-10);
        assert!((features.input_nnz - (30.0 + 2.0 * ab_nnz)).abs() < 1e-10);
    }

    #[test]
    fn frozen_coefficients_change_ranking_and_invalid_predictions_keep_baseline() {
        let bank = || {
            vec![
                LinearCandidate {
                    expr: "1".parse().unwrap(),
                    features: LinearFeatures {
                        work: 10.0,
                        input_nnz: 100.0,
                    },
                },
                LinearCandidate {
                    expr: "2".parse().unwrap(),
                    features: LinearFeatures {
                        work: 20.0,
                        input_nnz: 10.0,
                    },
                },
            ]
        };
        let work_model = LinearCostModel::from_coefficients([1.0, 1.0, 0.0]).unwrap();
        let input_model = LinearCostModel::from_coefficients([1.0, 0.0, 1.0]).unwrap();
        assert_eq!(select_plan(bank(), &work_model), bank()[0].expr);
        assert_eq!(select_plan(bank(), &input_model), bank()[1].expr);
        let bad = LinearCostModel::from_coefficients([1.0, -1.0, 0.0]).unwrap();
        assert_eq!(select_plan(bank(), &bad), bank()[0].expr);
        assert_eq!(input_model.coefficients(), [1.0, 0.0, 1.0]);
        let tied = LinearCostModel::from_coefficients([10.0, 0.0, 0.0]).unwrap();
        assert_eq!(select_plan(bank(), &tied), bank()[0].expr);
        let mut late_invalid = bank();
        late_invalid.push(LinearCandidate {
            expr: "3".parse().unwrap(),
            features: LinearFeatures {
                work: 100.0,
                input_nnz: 0.0,
            },
        });
        let model = LinearCostModel::from_coefficients([50.0, -1.0, 0.0]).unwrap();
        assert_eq!(select_plan(late_invalid, &model), bank()[0].expr);
    }

    #[test]
    fn unsupported_operators_are_explicitly_rejected() {
        assert!(candidates(RecExpr::default(), 100).is_err());
        for expression in [
            "(* 1)",
            "(| 1 2)",
            "(l* 1 2)",
            "(*r 1 2)",
            "(/ 1 (| 2 3))",
            "(/ (* 1) 2)",
            "(* (/ 1 2))",
        ] {
            assert!(candidates(expression.parse().unwrap(), 100).is_err());
        }
    }
}
