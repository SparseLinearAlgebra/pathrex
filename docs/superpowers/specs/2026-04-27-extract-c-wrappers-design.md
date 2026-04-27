# Design: Extract C Wrappers from `graph/mod.rs`

**Date:** 2026-04-27  
**Status:** Approved

## Motivation

`src/graph/mod.rs` currently mixes two distinct responsibilities:

1. **FFI glue** — RAII wrappers over raw C handles (`LagraphGraph`, `GraphblasVector`, `GraphblasMatrix`), the GraphBLAS/LAGraph init guard, and threading helpers. All of these import from `lagraph_sys` and perform unsafe operations.
2. **Pure Rust abstractions** — the `GraphBuilder`, `GraphDecomposition`, `Backend`, `Graph<B>` traits, `GraphError`, and `Edge`. None of these touch FFI.

Goal: separate FFI glue from pure-Rust abstractions so each file has a single clear purpose.

## Approach

**Option A — `src/graph/wrappers.rs` submodule** (chosen).

A new file `src/graph/wrappers.rs` is created to hold all FFI-touching code. `graph/mod.rs` re-exports everything from it, so all existing consumer imports remain unchanged.

## File Changes

### New: `src/graph/wrappers.rs`

Moved from `graph/mod.rs`:

| Item | Description |
|---|---|
| `GRB_INIT` static + `ensure_grb_init()` | One-time `LAGraph_Init` via `Once` |
| `compute_outer_inner()` | Threading helper |
| `ThreadScope` | RAII guard for `LAGraph_SetNumThreads` |
| `LagraphGraph` | RAII wrapper for `LAGraph_Graph` |
| `GraphblasVector` | RAII wrapper for `GrB_Vector` |
| `GraphblasMatrix` | RAII wrapper for `GrB_Matrix` |

Imports needed in `wrappers.rs`: `crate::{grb_ok, la_ok, lagraph_sys::*}`.

### Modified: `src/graph/mod.rs`

- Add at top: `pub mod wrappers;` and `pub use wrappers::{GraphblasMatrix, GraphblasVector, LagraphGraph, ThreadScope, compute_outer_inner, ensure_grb_init};`
- Remove all extracted code.
- Remove `use crate::{grb_ok, la_ok, lagraph_sys::*}` (no longer needed in this file).
- All traits, `GraphError`, `Edge`, `Graph<B>`, and tests remain unchanged.

### No other files change

All existing consumer imports (`use crate::graph::{LagraphGraph, ...}`) continue to work via the re-exports.

## Data Flow / Dependency Graph

```
lagraph_sys  ──►  graph/wrappers.rs  ──►  graph/mod.rs (re-exports)
                                                │
                                    ┌───────────┴──────────────┐
                                    ▼                          ▼
                             graph/inmemory.rs           rpq/nfarpq.rs
                                                         rpq/rpqmatrix.rs
                                                         formats/mm.rs
```

## Error Handling

No changes to error handling. `GraphError` stays in `graph/mod.rs`.

## Testing

No new tests required. Existing integration tests in `tests/` cover all moved types. Verification: `cargo build` must succeed with zero warnings; optionally run `cargo test` to confirm no regressions.

## Out of Scope

- Changing consumer import paths
- Moving `GraphError` or any traits
- Adding new public API
