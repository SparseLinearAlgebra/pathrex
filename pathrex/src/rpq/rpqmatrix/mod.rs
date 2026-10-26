//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

mod cost;
pub mod eval;
mod expr;
mod learning;
mod optimize;
mod plan;
pub mod result;
mod sampling;
pub(crate) mod stats;
mod stochastic;

pub use eval::RpqMatrixEvaluator;
pub use learning::{LinearCostModel, LinearFeatures, LinearModelError, LinearObservation};
pub use optimize::OptimizationStrategy;
pub use result::RpqMatrixResult;
