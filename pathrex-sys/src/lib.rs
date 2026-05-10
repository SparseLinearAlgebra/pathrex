//! Raw FFI bindings for SuiteSparse:GraphBLAS and LAGraph.
//!
//! This crate is a `*-sys` crate in the Rust ecosystem sense: it owns the
//! native build (link directives in `build.rs`) and exposes the bindgen-
//! generated symbols verbatim. Higher-level safe wrappers, RAII guards, and
//! Rust APIs live in the `pathrex` crate.
//!
//! Bindings are generated from the LAGraph headers in `deps/LAGraph/include`
//! and the system-installed `GraphBLAS.h`. The generated file
//! `src/lagraph_sys_generated.rs` is checked in; regenerate it with
//! `cargo build --features regenerate-bindings` (requires `libclang`).

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]

include!("lagraph_sys_generated.rs");

use core::fmt;

impl fmt::Display for GrB_Info {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrB_Info::GrB_SUCCESS => write!(f, "GrB_SUCCESS"),
            GrB_Info::GrB_NO_VALUE => write!(f, "GrB_NO_VALUE"),
            GrB_Info::GxB_EXHAUSTED => write!(f, "GxB_EXHAUSTED"),
            GrB_Info::GrB_UNINITIALIZED_OBJECT => write!(f, "GrB_UNINITIALIZED_OBJECT"),
            GrB_Info::GrB_NULL_POINTER => write!(f, "GrB_NULL_POINTER"),
            GrB_Info::GrB_INVALID_VALUE => write!(f, "GrB_INVALID_VALUE"),
            GrB_Info::GrB_INVALID_INDEX => write!(f, "GrB_INVALID_INDEX"),
            GrB_Info::GrB_DOMAIN_MISMATCH => write!(f, "GrB_DOMAIN_MISMATCH"),
            GrB_Info::GrB_DIMENSION_MISMATCH => write!(f, "GrB_DIMENSION_MISMATCH"),
            GrB_Info::GrB_OUTPUT_NOT_EMPTY => write!(f, "GrB_OUTPUT_NOT_EMPTY"),
            GrB_Info::GrB_NOT_IMPLEMENTED => write!(f, "GrB_NOT_IMPLEMENTED"),
            GrB_Info::GrB_ALREADY_SET => write!(f, "GrB_ALREADY_SET"),
            GrB_Info::GrB_PANIC => write!(f, "GrB_PANIC"),
            GrB_Info::GrB_OUT_OF_MEMORY => write!(f, "GrB_OUT_OF_MEMORY"),
            GrB_Info::GrB_INSUFFICIENT_SPACE => write!(f, "GrB_INSUFFICIENT_SPACE"),
            GrB_Info::GrB_INVALID_OBJECT => write!(f, "GrB_INVALID_OBJECT"),
            GrB_Info::GrB_INDEX_OUT_OF_BOUNDS => write!(f, "GrB_INDEX_OUT_OF_BOUNDS"),
            GrB_Info::GrB_EMPTY_OBJECT => write!(f, "GrB_EMPTY_OBJECT"),
            GrB_Info::GxB_JIT_ERROR => write!(f, "GxB_JIT_ERROR"),
            GrB_Info::GxB_GPU_ERROR => write!(f, "GxB_GPU_ERROR"),
            GrB_Info::GxB_OUTPUT_IS_READONLY => write!(f, "GxB_OUTPUT_IS_READONLY"),
        }
    }
}

impl From<i32> for GrB_Info {
    fn from(value: i32) -> Self {
        match value {
            0 => GrB_Info::GrB_SUCCESS,
            1 => GrB_Info::GrB_NO_VALUE,
            7 => GrB_Info::GxB_EXHAUSTED,
            -1 => GrB_Info::GrB_UNINITIALIZED_OBJECT,
            -2 => GrB_Info::GrB_NULL_POINTER,
            -3 => GrB_Info::GrB_INVALID_VALUE,
            -4 => GrB_Info::GrB_INVALID_INDEX,
            -5 => GrB_Info::GrB_DOMAIN_MISMATCH,
            -6 => GrB_Info::GrB_DIMENSION_MISMATCH,
            -7 => GrB_Info::GrB_OUTPUT_NOT_EMPTY,
            -8 => GrB_Info::GrB_NOT_IMPLEMENTED,
            -9 => GrB_Info::GrB_ALREADY_SET,
            -101 => GrB_Info::GrB_PANIC,
            -102 => GrB_Info::GrB_OUT_OF_MEMORY,
            -103 => GrB_Info::GrB_INSUFFICIENT_SPACE,
            -104 => GrB_Info::GrB_INVALID_OBJECT,
            -105 => GrB_Info::GrB_INDEX_OUT_OF_BOUNDS,
            -106 => GrB_Info::GrB_EMPTY_OBJECT,
            -7001 => GrB_Info::GxB_JIT_ERROR,
            -7002 => GrB_Info::GxB_GPU_ERROR,
            -7003 => GrB_Info::GxB_OUTPUT_IS_READONLY,
            _ => unimplemented!("Hope no more GrB status codes!"),
        }
    }
}
