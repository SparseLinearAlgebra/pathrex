//! Plan-based RPQ evaluation using `LAGraph_RPQMatrix`.

mod cost;
pub mod eval;
mod expr;
mod optimize;
mod plan;
pub mod result;

pub use eval::RpqMatrixEvaluator;
pub use optimize::OptimizationStrategy;
pub use result::RpqMatrixResult;
