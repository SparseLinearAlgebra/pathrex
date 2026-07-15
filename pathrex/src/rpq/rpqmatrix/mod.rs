//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

mod plan;
mod cost;
pub mod eval;
mod expr;
mod optimize;
pub mod result;

pub use eval::RpqMatrixEvaluator;
pub use optimize::OptimizationStrategy;
pub use result::RpqMatrixResult;
