# pathrex-sys

Native FFI bindings for [SuiteSparse:GraphBLAS] and [LAGraph], used by the
[`pathrex`](https://crates.io/crates/pathrex) crate.

This crate is not intended to be used directly. It exposes a thin Rust
binding layer over the GraphBLAS and LAGraph C APIs. End users should
depend on [`pathrex`](https://crates.io/crates/pathrex) instead, which
provides a safe, idiomatic Rust interface.

## What this crate does

At build time, `pathrex-sys/build.rs`:

1. Clones [SuiteSparse:GraphBLAS] at a pinned tag into `$OUT_DIR` and
   builds it as a static library (`libgraphblas.a`) via cmake.
2. Builds the bundled [LAGraph] source (shipped in `deps/LAGraph/`) as a
   static library against the GraphBLAS just built.
3. Emits `cargo:rustc-link-lib=static=...` directives so downstream
   crates link the static archives.

The first cold build clones GraphBLAS and runs cmake; it takes roughly
2-10 minutes depending on core count. Subsequent builds reuse the
GraphBLAS source tree under `$OUT_DIR/graphblas-src/` and the cmake build
directory.

## System requirements

| Dependency | Purpose |
|---|---|
| **cmake** | Building GraphBLAS and LAGraph from source |
| **git** | Fetching pinned GraphBLAS source at build time |
| **C/C++ toolchain** | Compiling GraphBLAS and LAGraph (gcc or clang) |
| **OpenMP runtime** | Linked dynamically: `libgomp` on Linux, `libomp` on macOS, `/openmp` on MSVC |

## Features

| Feature | Effect |
|---|---|
| `regenerate-bindings` | Regenerates `src/lagraph_sys_generated.rs` via `bindgen` from the LAGraph and GraphBLAS headers at build time. Requires `libclang`. Without this feature the checked-in bindings are used as-is. |

## License

MIT. See [LICENSE](../LICENSE).

LAGraph is bundled under its own BSD-2-Clause license; see
`deps/LAGraph/LICENSE`. SuiteSparse:GraphBLAS is fetched at build time
under the Apache-2.0 license.

[SuiteSparse:GraphBLAS]: https://github.com/DrTimothyAldenDavis/GraphBLAS
[LAGraph]: https://github.com/SparseLinearAlgebra/LAGraph
