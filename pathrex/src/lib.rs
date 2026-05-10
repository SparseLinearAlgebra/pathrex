pub mod eval;
pub mod formats;
pub mod graph;
pub mod rpq;
pub mod sparql;
#[allow(unused_unsafe, dead_code)]
pub mod utils;

/// Re-export of the [`pathrex_sys`] FFI crate under the historical name.
///
/// Internal modules and integration tests reach the raw GraphBLAS / LAGraph
/// bindings through `crate::lagraph_sys` (and `pathrex::lagraph_sys` from
/// outside the crate). The bindings themselves now live in the dedicated
/// `pathrex-sys` crate; this re-export keeps existing call sites working.
pub use pathrex_sys as lagraph_sys;

#[cfg(feature = "bench")]
pub mod cli;
