//! Ordinary least squares for full-plan runtime: t_us = theta0 + theta1 W + theta2 S.
//!
//! Lecture basis: MT_06-11, PDF pages 10–12. Features are an application-specific
//! choice. Training is batch-only; inference never changes the coefficients.
//! Explicit OLS objective: stochastic_programming_1.pdf, section 3.2, page 2.
use std::{fs, path::Path};

use thiserror::Error;

const MODEL_HEADER: &str = "pathrex-linear-plan-v1";
const DIM: usize = 3;

/// Pre-execution plan estimates. The intercept is implicitly one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearFeatures {
    /// Sum of the analytical multiplication-work estimates for unique operations.
    pub work: f64,
    /// Sum of estimated operand nonzero counts for unique multiplications.
    pub input_nnz: f64,
}

impl LinearFeatures {
    fn row(self) -> [f64; DIM] {
        [1.0, self.work, self.input_nnz]
    }

    pub(super) fn valid(self) -> bool {
        self.work.is_finite()
            && self.work >= 0.0
            && self.input_nnz.is_finite()
            && self.input_nnz >= 0.0
    }
}

/// One complete plan execution, with features estimated before execution.
#[derive(Clone, Copy, Debug)]
pub struct LinearObservation {
    pub features: LinearFeatures,
    pub elapsed_us: f64,
}

#[derive(Debug, Error)]
pub enum LinearModelError {
    #[error("linear regression needs at least three observations")]
    TooFewObservations,
    #[error("invalid observation {0}: expected finite nonnegative features and positive time")]
    InvalidObservation(usize),
    #[error(
        "training features are rank deficient; supply plans with independently varying work and input nnz"
    )]
    RankDeficient,
    #[error("invalid linear model: {0}")]
    InvalidModel(String),
    #[error("linear model I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Frozen full-plan model. Coefficients are in microseconds, without logarithms.
#[derive(Clone, Debug, PartialEq)]
pub struct LinearCostModel {
    coefficients: [f64; DIM],
}

impl LinearCostModel {
    pub fn from_coefficients(coefficients: [f64; DIM]) -> Result<Self, LinearModelError> {
        if !coefficients.iter().all(|v| v.is_finite()) {
            return Err(LinearModelError::InvalidModel(
                "coefficients must be finite".into(),
            ));
        }
        Ok(Self { coefficients })
    }

    pub fn coefficients(&self) -> [f64; DIM] {
        self.coefficients
    }

    /// Invalid extrapolations request the caller's baseline fallback. Negative
    /// coefficients are permitted by unconstrained ordinary least squares.
    pub fn predict(&self, features: LinearFeatures) -> Option<f64> {
        if !features.valid() {
            return None;
        }
        let value: f64 = self
            .coefficients
            .iter()
            .zip(features.row())
            .map(|(a, b)| a * b)
            .sum();
        (value.is_finite() && value > 0.0).then_some(value)
    }

    /// Minimize sum_n (elapsed_us_n - [1,W_n,S_n] theta)^2.
    ///
    /// Column scaling and pivoted Householder QR improve numerical conditioning;
    /// they do not change the statistical model or add regularization. A
    /// rank-deficient design is rejected instead of fitting arbitrary weights.
    pub fn fit(observations: &[LinearObservation]) -> Result<Self, LinearModelError> {
        let count = observations.len();
        if count < DIM {
            return Err(LinearModelError::TooFewObservations);
        }
        let mut scales = [1.0_f64, 0.0, 0.0];
        let mut target_scale = 0.0_f64;
        for (i, observation) in observations.iter().enumerate() {
            if !observation.features.valid()
                || !observation.elapsed_us.is_finite()
                || observation.elapsed_us <= 0.0
            {
                return Err(LinearModelError::InvalidObservation(i));
            }
            for (scale, value) in scales.iter_mut().zip(observation.features.row()) {
                *scale = scale.max(value.abs());
            }
            target_scale = target_scale.max(observation.elapsed_us);
        }
        if scales.contains(&0.0) {
            return Err(LinearModelError::RankDeficient);
        }
        let mut a = observations
            .iter()
            .map(|o| {
                let row = o.features.row();
                std::array::from_fn::<_, DIM, _>(|j| row[j] / scales[j])
            })
            .collect::<Vec<_>>();
        let mut b = observations
            .iter()
            .map(|o| o.elapsed_us / target_scale)
            .collect::<Vec<_>>();
        let mut permutation = [0, 1, 2];
        let tolerance = 64.0 * f64::EPSILON * count as f64 * (count as f64).sqrt();
        for k in 0..DIM {
            let norm = |j: usize| a[k..].iter().fold(0.0_f64, |n, row| n.hypot(row[j]));
            let pivot = (k..DIM)
                .max_by(|&i, &j| norm(i).total_cmp(&norm(j)))
                .unwrap();
            if norm(pivot) <= tolerance {
                return Err(LinearModelError::RankDeficient);
            }
            for row in &mut a {
                row.swap(k, pivot);
            }
            permutation.swap(k, pivot);
            let column_norm = a[k..].iter().fold(0.0_f64, |n, row| n.hypot(row[k]));
            let alpha = -column_norm.copysign(a[k][k]);
            let mut reflector = a[k..].iter().map(|row| row[k]).collect::<Vec<_>>();
            reflector[0] -= alpha;
            let length = reflector.iter().fold(0.0_f64, |n, &v| n.hypot(v));
            for v in &mut reflector {
                *v /= length;
            }
            for j in k..DIM {
                let dot: f64 = reflector
                    .iter()
                    .zip(&a[k..])
                    .map(|(v, row)| v * row[j])
                    .sum();
                for (row, &v) in a[k..].iter_mut().zip(&reflector) {
                    row[j] -= 2.0 * v * dot;
                }
            }
            let dot: f64 = reflector.iter().zip(&b[k..]).map(|(v, y)| v * y).sum();
            for (y, &v) in b[k..].iter_mut().zip(&reflector) {
                *y -= 2.0 * v * dot;
            }
            a[k][k] = alpha;
            for row in &mut a[k + 1..] {
                row[k] = 0.0;
            }
        }
        let mut solution = [0.0; DIM];
        for k in (0..DIM).rev() {
            let remainder: f64 = (k + 1..DIM).map(|j| a[k][j] * solution[j]).sum();
            solution[k] = (b[k] - remainder) / a[k][k];
        }
        let mut coefficients = [0.0; DIM];
        for j in 0..DIM {
            let scale = scales[permutation[j]];
            let mut coefficient = solution[j] * (target_scale / scale);
            // The ratio can overflow (or underflow) even when the final
            // coefficient is representable. Reorder the same rescaling then.
            if !coefficient.is_finite() || (coefficient == 0.0 && solution[j] != 0.0) {
                coefficient = (solution[j] * target_scale) / scale;
            }
            coefficients[permutation[j]] = coefficient;
        }
        Self::from_coefficients(coefficients)
    }

    /// Versioned text format fixes the feature definitions and time units.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), LinearModelError> {
        let [a, b, c] = self.coefficients;
        fs::write(path, format!("{MODEL_HEADER}\n{a} {b} {c}\n"))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, LinearModelError> {
        let data = fs::read_to_string(path)?;
        let mut tokens = data.split_whitespace();
        if tokens.next() != Some(MODEL_HEADER) {
            return Err(LinearModelError::InvalidModel(
                "unsupported format/version".into(),
            ));
        }
        let mut coefficients = [0.0; DIM];
        for coefficient in &mut coefficients {
            *coefficient = tokens.next().and_then(|v| v.parse().ok()).ok_or_else(|| {
                LinearModelError::InvalidModel("expected three coefficients".into())
            })?;
        }
        if tokens.next().is_some() {
            return Err(LinearModelError::InvalidModel(
                "unexpected trailing data".into(),
            ));
        }
        Self::from_coefficients(coefficients)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(work: f64, input_nnz: f64, elapsed_us: f64) -> LinearObservation {
        LinearObservation {
            features: LinearFeatures { work, input_nnz },
            elapsed_us,
        }
    }

    #[test]
    fn ordinary_least_squares_recovers_coefficients_across_feature_scales() {
        let observations = [(1.0, 2.0), (4.0, 3.0), (2.0, 8.0), (6.0, 7.0), (9.0, 4.0)]
            .map(|(w, s)| observation(w * 1e12, s, 7.0 + 2.0 * w + 3.0 * s));
        let model = LinearCostModel::fit(&observations).unwrap();
        let [a, b, c] = model.coefficients();
        assert!((a - 7.0).abs() < 1e-9);
        assert!((b - 2e-12).abs() < 1e-21);
        assert!((c - 3.0).abs() < 1e-9);
        let predicted = model
            .predict(LinearFeatures {
                work: 3e12,
                input_nnz: 5.0,
            })
            .unwrap();
        assert!((predicted - 28.0).abs() < 1e-9);
    }

    #[test]
    fn finite_coefficients_survive_extreme_but_valid_scaling() {
        let observations = [(1.0, 2.0), (4.0, 3.0), (2.0, 8.0), (6.0, 7.0), (9.0, 4.0)]
            .map(|(w, s)| observation(w * 1e-20, s, 1e290 + 2e286 * w + 3e286 * s));
        let model = LinearCostModel::fit(&observations).unwrap();
        for (actual, expected) in model.coefficients().into_iter().zip([1e290, 2e306, 3e286]) {
            assert!((actual / expected - 1.0).abs() < 1e-7);
        }
    }

    #[test]
    fn varied_noisy_designs_minimize_squared_error_and_ignore_row_order() {
        let mut state = 0x71f0_3bd8_19ce_4201_u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        for wscale in [1e-80, 1e-12, 1.0, 1e12, 1e80] {
            for sscale in [1e-80, 1e-12, 1.0, 1e12, 1e80] {
                let mut observations = (0..40)
                    .map(|_| {
                        let w = 1.0 + 5.0 * next();
                        let s = 1.0 + 7.0 * next();
                        let noise = 0.2 * (next() - 0.5);
                        observation(w * wscale, s * sscale, 5.0 + 2.0 * w + 3.0 * s + noise)
                    })
                    .collect::<Vec<_>>();
                let model = LinearCostModel::fit(&observations).unwrap();
                // Phi^T residual = 0 characterizes the unconstrained minimum;
                // check in normalized coordinates, independent of the QR code.
                let mut gradient = [0.0; DIM];
                for o in &observations {
                    let residual = model.predict(o.features).unwrap() - o.elapsed_us;
                    for (g, x) in gradient.iter_mut().zip([
                        1.0,
                        o.features.work / wscale,
                        o.features.input_nnz / sscale,
                    ]) {
                        *g += residual * x;
                    }
                }
                assert!(gradient.iter().all(|v| v.abs() < 1e-8), "{gradient:?}");
                observations.reverse();
                let reversed = LinearCostModel::fit(&observations).unwrap();
                for o in &observations {
                    assert!(
                        (model.predict(o.features).unwrap()
                            - reversed.predict(o.features).unwrap())
                        .abs()
                            < 1e-9
                    );
                }
            }
        }
    }

    #[test]
    fn least_squares_does_not_constrain_negative_fitted_values() {
        let observations = [
            observation(0.0, 0.0, 1.0),
            observation(1.0, 0.0, 1.0),
            observation(2.0, 0.0, 100.0),
            observation(0.0, 1.0, 1.0),
        ];
        let model = LinearCostModel::fit(&observations).unwrap();
        assert!(model.coefficients()[0] < 0.0);
        assert!(model.predict(observations[0].features).is_none());
        let mut gradient = [0.0; DIM];
        for o in observations {
            let fitted: f64 = model
                .coefficients()
                .iter()
                .zip(o.features.row())
                .map(|(a, b)| a * b)
                .sum();
            for (g, x) in gradient.iter_mut().zip(o.features.row()) {
                *g += (fitted - o.elapsed_us) * x;
            }
        }
        assert!(gradient.iter().all(|v| v.abs() < 1e-9));
    }

    #[test]
    fn noisy_fit_satisfies_least_squares_normal_equations() {
        let observations = [
            observation(1.0, 2.0, 10.0),
            observation(4.0, 3.0, 15.0),
            observation(2.0, 8.0, 26.0),
            observation(6.0, 7.0, 29.0),
            observation(9.0, 4.0, 22.0),
        ];
        let model = LinearCostModel::fit(&observations).unwrap();
        let mut gradient = [0.0; DIM];
        for o in observations {
            let residual = model.predict(o.features).unwrap() - o.elapsed_us;
            for (g, x) in gradient.iter_mut().zip(o.features.row()) {
                *g += residual * x;
            }
        }
        assert!(gradient.iter().all(|v| v.abs() < 1e-9));
    }

    #[test]
    fn rejects_rank_deficient_and_invalid_training_data() {
        assert!(matches!(
            LinearCostModel::fit(&[]),
            Err(LinearModelError::TooFewObservations)
        ));
        let singular = [
            observation(1.0, 2.0, 4.0),
            observation(2.0, 4.0, 5.0),
            observation(3.0, 6.0, 6.0),
        ];
        assert!(matches!(
            LinearCostModel::fit(&singular),
            Err(LinearModelError::RankDeficient)
        ));
        for bad in [
            observation(f64::NAN, 1.0, 1.0),
            observation(1.0, -1.0, 1.0),
            observation(1.0, 1.0, 0.0),
            observation(1.0, 1.0, f64::INFINITY),
        ] {
            assert!(matches!(
                LinearCostModel::fit(&[bad; 3]),
                Err(LinearModelError::InvalidObservation(_))
            ));
        }
    }

    #[test]
    fn model_round_trip_is_exact_and_malformed_models_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.txt");
        let model = LinearCostModel::from_coefficients([1.25, -2e-12, 3.5]).unwrap();
        model.save(&path).unwrap();
        assert_eq!(LinearCostModel::load(&path).unwrap(), model);
        for invalid in [
            "wrong\n1 2 3",
            "pathrex-linear-plan-v1\n1 2",
            "pathrex-linear-plan-v1\n1 NaN 3",
            "pathrex-linear-plan-v1\n1 2 3 extra",
        ] {
            fs::write(&path, invalid).unwrap();
            assert!(LinearCostModel::load(&path).is_err());
        }
        assert!(
            model
                .predict(LinearFeatures {
                    work: 1e20,
                    input_nnz: 0.0
                })
                .is_none()
        );
    }
}
