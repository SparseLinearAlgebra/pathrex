# AGENTS.md — Pathrex

## Project Overview

**Pathrex** is a Rust library and CLI tool for benchmarking queries on edge-labeled graphs
constrained by regular languages and context-free languages.
It uses **SuiteSparse:GraphBLAS** (via **LAGraph**) for sparse Boolean matrix operations and
decomposes a graph by edge label into one Boolean adjacency matrix per label.

## Repository Layout

```
pathrex/
├── Cargo.toml                  # Crate manifest (edition 2024)
├── build.rs                    # Links LAGraph + LAGraphX; optionally regenerates FFI bindings
├── src/
│   ├── lib.rs                  # Modules: formats, graph, sparql, utils (pub(crate)), lagraph_sys
│   ├── main.rs                 # Binary entry point (placeholder)
│   ├── lagraph_sys.rs          # FFI module — includes generated bindings
│   ├── lagraph_sys_generated.rs# Bindgen output (checked in, regenerated in CI)
│   ├── utils.rs                # Internal helpers: CountingBuilder, CountOutput, VecSource,
│   │                           #   grb_ok! and la_ok! macros
│   ├── graph/
│   │   ├── mod.rs              # Core traits (GraphBuilder, GraphDecomposition, GraphSource,
│   │   │                       #   Backend, Graph<B>), error types, RAII wrappers, GrB init
│   │   └── inmemory.rs         # InMemory marker, InMemoryBuilder, InMemoryGraph
│   ├── rpq/
│   │   ├── mod.rs              # RpqEvaluator (assoc. Result), RpqQuery, Endpoint, PathExpr, RpqError
│   │   └── rpqmatrix.rs        # Matrix-plan RPQ evaluator
│   ├── sparql/
│   │   └── mod.rs              # parse_rpq / extract_rpq → RpqQuery (spargebra)
│   └── formats/
│       ├── mod.rs              # FormatError enum, re-exports
│       ├── csv.rs              # Csv<R> — CSV → Edge iterator (CsvConfig, ColumnSpec)
│       ├── mm.rs               # MatrixMarket directory loader (vertices.txt, edges.txt, *.txt)
│       └── nt.rs               # NTriples<R> — N-Triples → Edge iterator (full predicate IRI labels)
├── tests/
│   ├── inmemory_tests.rs       # Integration tests for InMemoryBuilder / InMemoryGraph
│   ├── mm_tests.rs             # Integration tests for MatrixMarket format
│   └── rpqmatrix_tests.rs      # Integration tests for matrix-plan RPQ evaluator
├── deps/
│   └── LAGraph/                # Git submodule (SparseLinearAlgebra/LAGraph)
└── .github/workflows/ci.yml   # CI: build GraphBLAS + LAGraph, cargo build & test
```

## Build & Dependencies

### System prerequisites

| Dependency | Purpose |
|---|---|
| **SuiteSparse:GraphBLAS** | Sparse matrix engine (`libgraphblas`) |
| **LAGraph** | Graph algorithm library on top of GraphBLAS (`liblagraph`) |
| **cmake** | Building LAGraph from source |
| **libclang-dev / clang** | Required by `bindgen` when `regenerate-bindings` feature is active |

### Building

```bash
# Ensure submodules are present
git submodule update --init --recursive

# Build and install SuiteSparse:GraphBLAS system-wide
git clone --depth 1 https://github.com/DrTimothyAldenDavis/GraphBLAS.git
cd GraphBLAS && make compact && sudo make install && cd ..

# Build LAGraph inside the submodule (no system-wide install required)
cd deps/LAGraph && make && cd ../..

# Build pathrex
cargo build

# Run tests
LD_LIBRARY_PATH=deps/LAGraph/build/src:deps/LAGraph/build/experimental:/usr/local/lib cargo test
```

### How `build.rs` handles linking

[`build.rs`](build.rs) performs two jobs:

1. **Native linking.** It emits six Cargo directives:
   - `cargo:rustc-link-lib=dylib=graphblas` — dynamically links `libgraphblas`.
   - `cargo:rustc-link-search=native=/usr/local/lib` — adds the system GraphBLAS
     install path to the native library search path.
   - `cargo:rustc-link-lib=dylib=lagraph` — dynamically links `liblagraph`.
   - `cargo:rustc-link-search=native=deps/LAGraph/build/src` — adds the
     submodule's core build output to the native library search path.
   - `cargo:rustc-link-lib=dylib=lagraphx` — dynamically links `liblagraphx`
     (experimental algorithms).
   - `cargo:rustc-link-search=native=deps/LAGraph/build/experimental` —
     adds the experimental build output to the native library search path.

   LAGraph does **not** need to be installed system-wide; building the submodule
   in `deps/LAGraph/` is sufficient for compilation and linking.
   SuiteSparse:GraphBLAS **must** be installed system-wide (`sudo make install`).

   At **runtime** the OS dynamic linker (`ld.so`) does not use Cargo's link
   search paths — it only consults `LD_LIBRARY_PATH`, `rpath`, and the system
   library cache. Set `LD_LIBRARY_PATH=/usr/local/lib` after a system-wide
   LAGraph install, or include the submodule build paths if not installing
   system-wide.

2. **Optional FFI binding regeneration** (feature `regenerate-bindings`).
   When the feature is active, [`regenerate_bindings()`](build.rs:20) runs
   `bindgen` against `deps/LAGraph/include/LAGraph.h` and
   `deps/LAGraph/include/LAGraphX.h` (always from the submodule — no system
   path search), plus `GraphBLAS.h` (searched in
   `/usr/local/include/suitesparse` and `/usr/include/suitesparse`). The
   generated Rust file is written to
   [`src/lagraph_sys_generated.rs`](src/lagraph_sys_generated.rs). Only a
   curated allowlist of GraphBLAS/LAGraph types and functions is exposed
   (see the `allowlist_*` calls in [`build.rs`](build.rs:59)).

### Feature flags

| Feature | Effect |
|---|---|
| `regenerate-bindings` | Runs `bindgen` at build time to regenerate `src/lagraph_sys_generated.rs` from `LAGraph.h`, `LAGraphX.h` (both from `deps/LAGraph/include`) and `GraphBLAS.h`. Without this feature the checked-in bindings are used as-is. |

### Pre-generated FFI bindings

The file `src/lagraph_sys_generated.rs` is checked into version control. CI
regenerates it with `--features regenerate-bindings`. **Do not hand-edit this file.**

## Architecture & Key Abstractions

### Edge

[`Edge`](src/graph/mod.rs:158) is the universal currency between format parsers and graph
builders: `{ source: String, target: String, label: String }`.

### GraphSource trait

[`GraphSource<B>`](src/graph/mod.rs:168) is implemented by any data source that knows how to
feed itself into a specific [`GraphBuilder`]:

- [`apply_to(self, builder: B) -> Result<B, B::Error>`](src/graph/mod.rs:169) — consumes the
  source and returns the populated builder.

[`Csv<R>`](src/formats/csv.rs), [`MatrixMarket`](src/formats/mm.rs), and [`NTriples<R>`](src/formats/nt.rs)
implement `GraphSource<InMemoryBuilder>` (see [`src/graph/inmemory.rs`](src/graph/inmemory.rs)), so they
can be passed to [`GraphBuilder::load`] and [`Graph::try_from`].

### GraphBuilder trait

[`GraphBuilder`](src/graph/mod.rs:173) accumulates edges and produces a
[`GraphDecomposition`](src/graph/mod.rs:192):

- [`load<S: GraphSource<Self>>(self, source: S)`](src/graph/mod.rs:183) — primary entry point;
  delegates to `GraphSource::apply_to`.
- [`build(self)`](src/graph/mod.rs:188) — finalise into an immutable graph.

`InMemoryBuilder` also exposes lower-level helpers outside the trait:

- [`push_edge(&mut self, edge: Edge)`](src/graph/inmemory.rs:83) — ingest one edge.
- [`with_stream<I, E>(self, stream: I)`](src/graph/inmemory.rs:93) — consume an
  `IntoIterator<Item = Result<Edge, E>>`.
- [`push_grb_matrix(&mut self, label, matrix: GrB_Matrix)`](src/graph/inmemory.rs:106) — accept
  a pre-built `GrB_Matrix` for a label, wrapping it in an `LAGraph_Graph` immediately.

### Backend trait & Graph\<B\> handle

[`Backend`](src/graph/mod.rs:221) associates a marker type with a concrete builder/graph pair:

```rust
pub trait Backend {
    type Graph: GraphDecomposition;
    type Builder: GraphBuilder<Graph = Self::Graph>;
}
```

[`Graph<B>`](src/graph/mod.rs:233) is a zero-sized handle parameterised by a `Backend`:

- [`Graph::<InMemory>::builder()`](src/graph/mod.rs:238) — returns a fresh `InMemoryBuilder`.
- [`Graph::<InMemory>::try_from(source)`](src/graph/mod.rs:242) — builds a graph from a single
  source in one call.

[`InMemory`](src/graph/inmemory.rs:27) is the concrete backend marker type.

### GraphDecomposition trait

[`GraphDecomposition`](src/graph/mod.rs:192) is the read-only query interface:

- [`get_graph(label)`](src/graph/mod.rs:196) — returns `Arc<LagraphGraph>` for a given edge label.
- [`get_node_id(string_id)`](src/graph/mod.rs:199) / [`get_node_name(mapped_id)`](src/graph/mod.rs:202) — bidirectional string ↔ integer dictionary.
- [`num_nodes()`](src/graph/mod.rs:203) — total unique nodes.

### InMemoryBuilder / InMemoryGraph

[`InMemoryBuilder`](src/graph/inmemory.rs:36) is the primary `GraphBuilder` implementation.
It collects edges in RAM, then [`build()`](src/graph/inmemory.rs:131) calls
GraphBLAS to create one `GrB_Matrix` per label via COO format, wraps each in an
`LAGraph_Graph`, and returns an [`InMemoryGraph`](src/graph/inmemory.rs:174).

Multiple CSV sources can be chained with repeated `.load()` calls; all edges are merged
into a single graph.

**Node ID representation:** Internally, `InMemoryBuilder` uses `HashMap<usize, String>` for
`id_to_node` (changed from `Vec<String>` to support sparse/pre-assigned IDs from MatrixMarket).
The [`set_node_map()`](src/graph/inmemory.rs:67) method allows bulk-installing a node mapping,
which is used by the MatrixMarket loader.

### Format parsers

Three built-in parsers are available, each yielding
`Iterator<Item = Result<Edge, FormatError>>` and pluggable into
`GraphBuilder::load()` via `GraphSource<InMemoryBuilder>` (see [`src/graph/inmemory.rs`](src/graph/inmemory.rs)).
CSV and MatrixMarket edge loaders are available:

#### `Csv<R>`

[`Csv<R>`](src/formats/csv.rs) parses delimiter-separated edge files.

Configuration is via [`CsvConfig`](src/formats/csv.rs:17):

| Field | Default | Description |
|---|---|---|
| `source_column` | `Index(0)` | Column for the source node (by index or name) |
| `target_column` | `Index(1)` | Column for the target node |
| `label_column` | `Index(2)` | Column for the edge label |
| `has_header` | `true` | Whether the first row is a header |
| `delimiter` | `b','` | Field delimiter byte |

[`ColumnSpec`](src/formats/csv.rs:11) is either `Index(usize)` or `Name(String)`.
Name-based lookup requires `has_header: true`.

#### MatrixMarket directory format

[`MatrixMarket`](src/formats/mm.rs) loads an edge-labeled graph from a directory with:

- `vertices.txt` — one line per node: `<node_name> <1-based-index>` on disk; [`get_node_id`](src/graph/mod.rs:199) returns the matching **0-based** matrix index
- `edges.txt` — one line per label: `<label_name> <1-based-index>` (selects `n.txt`)
- `<n>.txt` — MatrixMarket adjacency matrix for label with index `n`

Names in mapping files may be written with SPARQL-style angle brackets (e.g. `<Article1>`).
[`parse_index_map`](src/formats/mm.rs) strips a single pair of surrounding `<`/`>` so
dictionary keys match short labels (`Article1`), aligning with IRIs after
[`RpqQuery::strip_base`](src/rpq/mod.rs) on SPARQL-derived queries.

The loader uses [`LAGraph_MMRead`](src/lagraph_sys.rs) to parse each `.txt` file into a
`GrB_Matrix`, then wraps it in an `LAGraph_Graph`. Vertex indices from `vertices.txt` are
converted to 0-based and installed via [`InMemoryBuilder::set_node_map()`](src/graph/inmemory.rs:67).

Helper functions:

- [`load_mm_file(path)`](src/formats/mm.rs:39) — reads a single MatrixMarket file into a
  `GrB_Matrix`.
- [`parse_index_map(path)`](src/formats/mm.rs:81) — parses `<name> <index>` lines; indices must be **>= 1** and **unique** within the file.

`MatrixMarket` implements `GraphSource<InMemoryBuilder>` in [`src/graph/inmemory.rs`](src/graph/inmemory.rs) (see the `impl` at line 215): `vertices.txt` maps are converted from 1-based file indices to 0-based matrix ids before [`set_node_map`](src/graph/inmemory.rs:67); `edges.txt` indices are unchanged for `n.txt` lookup.

#### `NTriples<R>`

[`NTriples<R>`](src/formats/nt.rs:64) parses [W3C N-Triples](https://www.w3.org/TR/n-triples/)
RDF files using `oxttl` and `oxrdf`. Each triple `(subject, predicate, object)` becomes an
[`Edge`](src/graph/mod.rs:158) where:

- `source` — subject IRI or blank-node ID (`_:label`).
- `target` — object IRI or blank-node ID; triples whose object is an RDF
  literal yield `Err(FormatError::LiteralAsNode)` (callers may filter these out).
- `label` — predicate IRI, transformed by [`LabelExtraction`](src/formats/nt.rs:38):

| Variant | Behaviour |
|---|---|
| `LocalName` (default) | Fragment (`#name`) or last path segment of the predicate IRI |
| `FullIri` | Full predicate IRI string |

Constructors:

- [`NTriples::new(reader)`](src/formats/nt.rs:70) — uses `LabelExtraction::LocalName`.
- [`NTriples::with_label_extraction(reader, strategy)`](src/formats/nt.rs:74) — explicit strategy.

### SPARQL parsing (`src/sparql/mod.rs`)

The [`sparql`](src/sparql/mod.rs) module uses the [`spargebra`](https://crates.io/crates/spargebra)
crate to parse SPARQL 1.1 query strings and build a pathrex-native [`RpqQuery`](src/rpq/mod.rs)
for RPQ evaluators.

**Supported query form:** `SELECT` queries with exactly one triple or property
path pattern in the `WHERE` clause. Relative IRIs such as `<knows>` require a
`BASE` declaration (or `PREFIX` / full IRIs). Example:

```sparql
BASE <http://example.org/>
SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }
```

Key public items:

- [`parse_rpq(sparql)`](src/sparql/mod.rs) — parses a SPARQL string with
  `SparqlParser` and returns an [`RpqQuery`](src/rpq/mod.rs).
- [`extract_rpq(query)`](src/sparql/mod.rs) — validates a parsed [`spargebra::Query`] is a
  `SELECT` with a single path pattern and returns an [`RpqQuery`](src/rpq/mod.rs).
  Use this when you construct a custom [`SparqlParser`](https://docs.rs/spargebra) (e.g. with
  prefix declarations) and call `parse_query` yourself.
- [`ExtractError`](src/sparql/mod.rs) — error enum for extraction failures
  (`NotSelect`, `NotSinglePath`, `UnsupportedSubject`, `UnsupportedObject`,
  `VariablePredicate`). Converts to [`RpqError::Extract`](src/rpq/mod.rs) via `#[from]`.

Call [`RpqQuery::strip_base`](src/rpq/mod.rs) when graph edge labels are short names
and the parsed query contains full IRIs sharing a common prefix.

The module handles spargebra's desugaring of sequence paths (`?x <a>/<b>/<c> ?y`)
from a chain of BGP triples back into a single path expression.

### RPQ evaluation (`src/rpq/`)

The [`rpq`](src/rpq/mod.rs) module provides an abstraction for evaluating
Regular Path Queries (RPQs) over edge-labeled graphs using GraphBLAS/LAGraph.

Key public items:

- [`Endpoint`](src/rpq/mod.rs) — `Variable(String)` or `Named(String)` (IRI string).
- [`PathExpr`](src/rpq/mod.rs) — `Label`, `Sequence`, `Alternative`, `ZeroOrMore`,
  `OneOrMore`, `ZeroOrOne`.
- [`RpqQuery`](src/rpq/mod.rs) — `{ subject, path, object }` using the types above;
  [`strip_base(&mut self, base)`](src/rpq/mod.rs) removes a shared IRI prefix from
  named endpoints and labels.
- [`RpqEvaluator`](src/rpq/mod.rs) — trait with associated type `Result` and
  [`evaluate(query, graph)`](src/rpq/mod.rs) taking `&RpqQuery` and
  [`GraphDecomposition`], returning `Result<Self::Result, RpqError>`.
  Each concrete evaluator exposes its own output type (see below).
- [`RpqError`](src/rpq/mod.rs) — unified error type for RPQ parsing and evaluation:
  `Parse` (SPARQL syntax), `Extract` (query extraction), `UnsupportedPath`,
  `VertexNotFound`, and `Graph` (wraps [`GraphError`](src/graph/mod.rs) for
  label-not-found and GraphBLAS/LAGraph failures).

[`NfaRpqResult`](src/rpq/nfarpq.rs) wraps a [`GraphblasVector`] of reachable **target**
vertices. When the subject is a variable, every vertex is used as a source and
`LAGraph_RegularPathQuery` returns the union of targets — individual `(source, target)`
pairs are not preserved.

#### `RpqMatrixEvaluator` (`src/rpq/rpqmatrix.rs`)

[`RpqMatrixEvaluator`](src/rpq/rpqmatrix.rs) compiles [`PathExpr`] into a Boolean matrix plan
over label adjacency matrices and runs [`LAGraph_RPQMatrix`]. It returns
[`RpqMatrixResult`](src/rpq/rpqmatrix.rs): the path-relation `nnz` plus a
[`GraphblasMatrix`] duplicate of the result matrix (full reachability relation for the path).
Subject/object do not filter the matrix; a named subject is only validated to exist.
Bound objects are not supported yet ([`RpqError::UnsupportedPath`]).
[`NTriples<R>`](src/formats/nt.rs:51) parses [W3C N-Triples](https://www.w3.org/TR/n-triples/)
RDF files using `oxttl` and `oxrdf`. Each triple `(subject, predicate, object)` becomes an
[`Edge`](src/graph/mod.rs:158) where:

- `source` — subject IRI or blank-node ID (`_:label`).
- `target` — object IRI or blank-node ID; triples whose object is an RDF
  literal yield `Err(FormatError::LiteralAsNode)` (callers may filter these out).
- `label` — full predicate IRI string (including fragment `#…` when present).

Constructor:

- [`NTriples::new(reader)`](src/formats/nt.rs:56) — parses the stream; each predicate IRI is copied verbatim to the edge label.
### SPARQL parsing (`src/sparql/mod.rs`)

The [`sparql`](src/sparql/mod.rs) module uses the [`spargebra`](https://crates.io/crates/spargebra)
crate to parse SPARQL 1.1 query strings and extract the single property-path
triple pattern that pathrex's RPQ evaluators operate on.

**Supported query form:** `SELECT` queries with exactly one triple or property
path pattern in the `WHERE` clause, e.g.:

```sparql
SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }
```

Key public items:

- [`parse_query(sparql)`](src/sparql/mod.rs:45) — parses a SPARQL string into a
  [`spargebra::Query`].
- [`extract_path(query)`](src/sparql/mod.rs:67) — validates a parsed `Query` is a
  `SELECT` with a single path pattern and returns a [`PathTriple`](src/sparql/mod.rs:56).
- [`parse_rpq(sparql)`](src/sparql/mod.rs:190) — convenience function combining
  `parse_query` + `extract_path` in one call.
- [`PathTriple`](src/sparql/mod.rs:56) — holds the extracted `subject`
  ([`TermPattern`]), `path` ([`PropertyPathExpression`]), and `object`
  ([`TermPattern`]).
- [`ExtractError`](src/sparql/mod.rs:25) — error enum for extraction failures
  (`NotSelect`, `NotSinglePath`, `UnsupportedSubject`, `UnsupportedObject`,
  `VariablePredicate`).
- [`RpqParseError`](src/sparql/mod.rs:198) — combined error for [`parse_rpq`]
  wrapping both [`SparqlSyntaxError`] and [`ExtractError`].
- [`DEFAULT_BASE_IRI`](src/sparql/mod.rs:38) — `"http://example.org/"`, the
  default base IRI constant.

The module also handles spargebra's desugaring of sequence paths
(`?x <a>/<b>/<c> ?y`) from a chain of BGP triples back into a single
[`PropertyPathExpression::Sequence`].

### FFI layer

[`lagraph_sys`](src/lagraph_sys.rs) exposes raw C bindings for GraphBLAS and
LAGraph. Safe Rust wrappers live in [`graph::mod`](src/graph/mod.rs):

- [`LagraphGraph`](src/graph/mod.rs:48) — RAII wrapper around `LAGraph_Graph` (calls
  `LAGraph_Delete` on drop). Also provides
  [`LagraphGraph::from_coo()`](src/graph/mod.rs:85) to build directly from COO arrays.
- [`GraphblasVector`](src/graph/mod.rs:128) — RAII wrapper around `GrB_Vector`
  (derives `Debug`).
- [`GraphblasMatrix`](src/graph/mod.rs) — RAII wrapper around `GrB_Matrix` (`dup` + `free` on drop).
- [`ensure_grb_init()`](src/graph/mod.rs:39) — one-time `LAGraph_Init` via `std::sync::Once`.

### Macros (`src/utils.rs`)

Two `#[macro_export]` macros handle FFI error mapping:

- [`grb_ok!(expr)`](src/utils.rs:138) — evaluates a GraphBLAS call inside `unsafe`, maps the
  `i32` return to `Result<(), GraphError::GraphBlas(info)>`.
- [`la_ok!(fn::path(args…))`](src/utils.rs:167) — evaluates a LAGraph call, automatically
  appending the required `*mut i8` message buffer, and maps failure to
  `GraphError::LAGraph(info, msg)`.

## Coding Conventions

- **Rust edition 2024**.
- Error handling via `thiserror` derive macros; three main error enums:
  [`GraphError`](src/graph/mod.rs:15), [`FormatError`](src/formats/mod.rs:24),
  and [`RpqError`](src/rpq/mod.rs:78).
- `FormatError` converts into `GraphError` via `#[from] FormatError` on the
  `GraphError::Format` variant.
- `GraphError` converts into `RpqError` via `#[from] GraphError` on the
  `RpqError::Graph` variant, enabling `?` propagation in evaluators.
- Unsafe FFI calls are confined to `lagraph_sys`, `graph/mod.rs`,
  `graph/inmemory.rs`, `rpq/nfarpq.rs`, and `rpq/rpqmatrix.rs`. All raw pointers are wrapped in
  RAII types that free resources on drop.
- `unsafe impl Send + Sync` is provided for `LagraphGraph`,
  `GraphblasVector`, and `GraphblasMatrix` because GraphBLAS handles are thread-safe after init.
- Unit tests live in `#[cfg(test)] mod tests` blocks inside each module.
  Integration tests that need GraphBLAS live in [`tests/inmemory_tests.rs`](tests/inmemory_tests.rs),
  [`tests/mm_tests.rs`](tests/mm_tests.rs), [`tests/nfarpq_tests.rs`](tests/nfarpq_tests.rs),
  and [`tests/rpqmatrix_tests.rs`](tests/rpqmatrix_tests.rs).

## Testing

```bash
# Run all tests (LAGraph installed system-wide)
LD_LIBRARY_PATH=/usr/local/lib cargo test --verbose

# If LAGraph is NOT installed system-wide (only built in the submodule):
LD_LIBRARY_PATH=deps/LAGraph/build/src:deps/LAGraph/build/experimental:/usr/local/lib cargo test --verbose
```

Tests in `src/graph/mod.rs` use `CountingBuilder` / `CountOutput` / `VecSource` from
[`src/utils.rs`](src/utils.rs) — these do **not** call into GraphBLAS and run without
native libraries.

Tests in `src/formats/csv.rs` and `src/formats/nt.rs` are pure Rust and need no native dependencies.

Tests in `src/sparql/mod.rs` are pure Rust and need no native dependencies.

Tests in `src/graph/inmemory.rs` and [`tests/inmemory_tests.rs`](tests/inmemory_tests.rs)
call real GraphBLAS/LAGraph and require the native libraries to be present.

## CI

The GitHub Actions workflow ([`.github/workflows/ci.yml`](.github/workflows/ci.yml))
runs on every push and PR across `stable`, `beta`, and `nightly` toolchains:

1. Checks out with `submodules: recursive`.
2. Installs cmake, libclang-dev, clang.
3. Builds and installs SuiteSparse:GraphBLAS from source (`sudo make install`).
4. Builds and installs LAGraph from the submodule (`sudo make install`).
5. `cargo build --features regenerate-bindings` — rebuilds FFI bindings.
6. `LD_LIBRARY_PATH=/usr/local/lib cargo test --verbose` — runs the full test suite.
