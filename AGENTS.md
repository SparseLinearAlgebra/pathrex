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
│   ├── lib.rs                  # Modules: eval, formats, graph, rpq, sparql, utils, lagraph_sys
│   ├── main.rs                 # Binary entry point (placeholder)
│   ├── lagraph_sys.rs          # FFI module — includes generated bindings
│   ├── lagraph_sys_generated.rs# Bindgen output (checked in, regenerated in CI)
│   ├── utils.rs                # Public helpers: CountingBuilder, CountOutput, VecSource,
│   │                           #   grb_ok! and la_ok! macros, build_graph
│   ├── graph/
│   │   ├── mod.rs              # Core traits (GraphBuilder, GraphDecomposition, GraphSource,
│   │   │                       #   Backend, Graph<B>), error types, RAII wrappers, GrB init
│   │   └── inmemory.rs         # InMemory marker, InMemoryBuilder, InMemoryGraph
│   ├── eval/
│   │   └── mod.rs              # Evaluator, PreparedEvaluator, ResultCount traits
│   ├── rpq/
│   │   ├── mod.rs              # RPQ query types, RpqError, RPQ marker subtraits
│   │   ├── nfarpq.rs           # NfaRpqEvaluator (LAGraph_RegularPathQuery)
│   │   └── rpqmatrix.rs        # Matrix-plan RPQ evaluator
│   ├── sparql/
│   │   └── mod.rs              # parse_rpq / extract_rpq → RpqQuery (spargebra)
│   └── formats/
│       ├── mod.rs              # FormatError enum, re-exports
│       ├── csv.rs              # Csv<R> — CSV → Edge iterator (CsvConfig, ColumnSpec)
│       ├── mm.rs               # MatrixMarket directory loader (vertices.txt, edges.txt, *.txt)
│       └── rdf.rs              # Rdf — unified RDF parser (N-Triples, Turtle) → Edge iterator
├── tests/
│   ├── inmemory_tests.rs       # Integration tests for InMemoryBuilder / InMemoryGraph
│   ├── mm_tests.rs             # Integration tests for MatrixMarket format
│   ├── nfarpq_tests.rs         # Integration tests for NfaRpqEvaluator
│   └── rpqmatrix_tests.rs      # Integration tests for matrix-plan RPQ evaluator
├── pathrex-sys/deps/
│   └── LAGraph/                # Git submodule (SparseLinearAlgebra/LAGraph)
└── .github/workflows/ci.yml   # CI: build GraphBLAS + LAGraph, cargo build & test
```

## Build & Dependencies

### System prerequisites

| Dependency | Purpose |
|---|---|
| **cmake** | Building GraphBLAS and LAGraph from source |
| **git** | Fetching pinned GraphBLAS source at build time |
| **C/C++ toolchain** | Compiling GraphBLAS and LAGraph (gcc or clang) |
| **OpenMP runtime** | Linked dynamically: `libgomp` on Linux, `libomp` on macOS, `/openmp` on MSVC |
| **libclang-dev / clang** | Required by `bindgen` when `regenerate-bindings` feature is active |

SuiteSparse:GraphBLAS no longer needs to be installed system-wide. It is
fetched, built statically, and linked into the binary by `pathrex-sys/build.rs`.

### Building

```bash
# Ensure the LAGraph submodule is present
git submodule update --init --recursive

# Build pathrex. First cold build clones GraphBLAS and runs cmake; takes
# ~2-10 minutes depending on core count. Subsequent builds reuse the
# GraphBLAS source tree under target/.../graphblas-src/ and the cmake
# build dir, so they are incremental.
cargo build

# Run tests (no LD_LIBRARY_PATH needed — everything but the OpenMP
# runtime is statically linked)
cargo test --workspace
```

### How `pathrex-sys/build.rs` handles linking

[`pathrex-sys/build.rs`](pathrex-sys/build.rs) performs three jobs:

1. **GraphBLAS fetch.** Clones SuiteSparse:GraphBLAS at the pin defined by
   the `GRAPHBLAS_TAG` constant (currently `v10.3.1`) into
   `$OUT_DIR/graphblas-src/` via `git clone --depth=1 --branch <tag>`. A
   sentinel file `<dir>/.pathrex-fetched` containing the tag string marks
   a completed clone; if the pin is bumped, the sentinel mismatches and
   the clone is wiped and retried. If the directory exists without a
   sentinel (interrupted earlier clone), it is also wiped before retrying.

2. **Native build + linking.** Drives cmake twice — once for GraphBLAS,
   once for the `pathrex-sys/deps/LAGraph` submodule:

   - GraphBLAS flags: `BUILD_SHARED_LIBS=OFF`, `BUILD_STATIC_LIBS=ON`,
     `GRAPHBLAS_BUILD_STATIC_LIBS=ON` (belt-and-braces),
     `GRAPHBLAS_USE_JIT=OFF` (no runtime C compiler required),
     `GRAPHBLAS_COMPACT=OFF` (full FactoryKernels for performance),
     `GRAPHBLAS_USE_OPENMP=ON`, `GRAPHBLAS_USE_CUDA=OFF`,
     `SUITESPARSE_DEMOS=OFF`, `BUILD_TESTING=OFF`, `Release` profile.
   - LAGraph flags: `BUILD_SHARED_LIBS=OFF`, `BUILD_STATIC_LIBS=ON`,
     `BUILD_TESTING=OFF`, plus `CMAKE_PREFIX_PATH=<graphblas_install>`
     and `GRAPHBLAS_ROOT=<graphblas_install>` so LAGraph's
     `find_package(GraphBLAS)` picks up our static build instead of any
     system one.
   - Static archives land in `$OUT_DIR/.../out/lib/` (or `lib64/` on
     Fedora-family distros — both candidates are probed by `pick_libdir`).
   - Emits `cargo:rustc-link-lib=static=lagraphx`,
     `cargo:rustc-link-lib=static=lagraph`,
     `cargo:rustc-link-lib=static=graphblas`. Order matters: `lagraphx`
     references symbols from `lagraph`'s utility module; both reference
     `graphblas`.
   - Emits OS-specific runtime libraries: `gomp`+`pthread`+`dl`+`m` on
     Linux, `omp`+`pthread` on macOS, nothing explicit on MSVC. Override
     via `RUSTFLAGS` if your toolchain ships a different OpenMP runtime
     (e.g. `libomp` on Linux + clang).

3. **docs.rs guard.** If the `DOCS_RS` environment variable is set, the
   entire native build is skipped. docs.rs sandboxes block all network
   access (so the `git clone` would fail) and have strict time/memory
   limits; rustdoc only needs to compile Rust code, not link or execute
   it.

4. **Optional FFI binding regeneration** (feature `regenerate-bindings`).
   When the feature is active, `regenerate_bindings()` runs `bindgen`
   against `deps/LAGraph/include/LAGraph.h`,
   `deps/LAGraph/include/LAGraphX.h`, and the GraphBLAS install tree's
   `include/suitesparse/GraphBLAS.h`. The generated Rust file is written
   to [`pathrex-sys/src/lagraph_sys_generated.rs`](pathrex-sys/src/lagraph_sys_generated.rs).
   Only a curated allowlist of GraphBLAS/LAGraph types and functions is
   exposed (see the `allowlist_*` calls in `pathrex-sys/build.rs`).

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

[`Csv<R>`](src/formats/csv.rs), [`MatrixMarket`](src/formats/mm.rs), and [`Rdf`](src/formats/rdf.rs)
implement `GraphSource<InMemoryBuilder>` (see [`src/graph/inmemory.rs`](src/graph/inmemory.rs)), so they
can be passed to [`GraphBuilder::load`] and [`Graph::try_from`].

### GraphBuilder trait

[`GraphBuilder`](src/graph/mod.rs:173) accumulates edges and produces a
[`GraphDecomposition`](src/graph/mod.rs:193):

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

[`GraphDecomposition`](src/graph/mod.rs:193) is the read-only query interface:

- [`get_graph(label)`](src/graph/mod.rs:197) — returns `Arc<LagraphGraph>` for a given edge label.
- [`get_node_id(string_id)`](src/graph/mod.rs:200) / [`get_node_name(mapped_id)`](src/graph/mod.rs:203) — bidirectional string ↔ integer dictionary.
- [`num_nodes()`](src/graph/mod.rs:204) — total unique nodes.

### Generic evaluator abstraction (`src/eval/`)

[`src/eval/mod.rs`](src/eval/mod.rs) defines query-language-agnostic evaluator traits:

- [`Evaluator`](src/eval/mod.rs) uses associated types for `Query`, `Result`, `Error`, and
  `Prepared`. The graph backend stays a method-level generic (`G: GraphDecomposition`) so one
  evaluator type can run against any graph backend selected at the call site.
- [`PreparedEvaluator`](src/eval/mod.rs) represents prepared `(query, graph)` state that can be
  executed repeatedly, which is used by benchmark timing loops.
- [`ResultCount`](src/eval/mod.rs) is separate from `Evaluator::Result`; only CLI runners that
  need counts require this bound, leaving room for future evaluators with richer result types.

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

- `vertices.txt` — one line per node: `<node_name> <1-based-index>` on disk; [`get_node_id`](src/graph/mod.rs:200) returns the matching **0-based** matrix index
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

#### `Rdf` — Unified RDF Parser

[`Rdf`](src/formats/rdf.rs) is a unified parser for RDF formats using `oxttl` and `oxrdf`.
It supports both **N-Triples** (`.nt`) and **Turtle** (`.ttl`) formats via the [`RdfFormat`](src/formats/rdf.rs) enum.

Each triple `(subject, predicate, object)` becomes an [`Edge`](src/graph/mod.rs:158) where:

- `source` — subject IRI or blank-node ID (`_:label`).
- `target` — object IRI or blank-node ID; triples whose object is an RDF
  literal yield `Err(FormatError::LiteralAsNode)` (callers may filter these out).
- `label` — full predicate IRI string (including fragment `#…` when present).

Constructor:

- [`Rdf::from_path(path)`](src/formats/rdf.rs) — auto-detects format from file extension (`.nt` → N-Triples, `.ttl` → Turtle). Parses in parallel using memory-mapping and rayon.

Format detection via [`RdfFormat::from_path(path)`](src/formats/rdf.rs):

| Extension | Format |
|---|---|
| `.nt`, `.ntriples` | `RdfFormat::NTriples` |
| `.ttl`, `.turtle` | `RdfFormat::Turtle` |

Example usage:

```rust
use pathrex::formats::Rdf;
use pathrex::graph::{Graph, InMemory};

// Auto-detect from extension
let graph = Graph::<InMemory>::try_from(
    Rdf::from_path("data.ttl")?
)?;
```

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
- [`RpqEvaluator`](src/rpq/mod.rs) — marker subtrait over
  [`Evaluator<Query = RpqQuery, Error = RpqError>`](src/eval/mod.rs), preserving the RPQ-facing
  trait name while the generic evaluator hierarchy lives in `src/eval/`.
- [`PreparedRpq`](src/rpq/mod.rs) — marker subtrait over
  [`PreparedEvaluator<Error = RpqError>`](src/eval/mod.rs).
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

The [`rpq`](src/rpq/mod.rs) module provides an abstraction for evaluating
Regular Path Queries (RPQs) over edge-labeled graphs using GraphBLAS/LAGraph.

Key public items:

- [`Endpoint`](src/rpq/mod.rs) — `Variable(String)` or `Named(String)` (IRI string).
- [`PathExpr`](src/rpq/mod.rs) — `Label`, `Sequence`, `Alternative`, `ZeroOrMore`,
  `OneOrMore`, `ZeroOrOne`.
- [`RpqQuery`](src/rpq/mod.rs) — `{ subject, path, object }` using the types above;
  [`strip_base(&mut self, base)`](src/rpq/mod.rs) removes a shared IRI prefix from
  named endpoints and labels.
- [`RpqEvaluator`](src/rpq/mod.rs) — marker subtrait over
  [`Evaluator<Query = RpqQuery, Error = RpqError>`](src/eval/mod.rs), preserving the RPQ-facing
  trait name while the generic evaluator hierarchy lives in `src/eval/`.
- [`PreparedRpq`](src/rpq/mod.rs) — marker subtrait over
  [`PreparedEvaluator<Error = RpqError>`](src/eval/mod.rs).
- [`RpqError`](src/rpq/mod.rs) — unified error type for RPQ parsing and evaluation:
  `Parse` (SPARQL syntax), `Extract` (query extraction), `UnsupportedPath`,
  `VertexNotFound`, and `Graph` (wraps [`GraphError`](src/graph/mod.rs) for
  label-not-found and GraphBLAS/LAGraph failures).

#### `NfaRpqEvaluator` (`src/rpq/nfarpq.rs`)

[`NfaRpqEvaluator`](src/rpq/nfarpq.rs) implements [`RpqEvaluator`] by:

1. Converting a [`PathExpr`] into an [`Nfa`](src/rpq/nfarpq.rs) via Thompson's
   construction ([`Nfa::from_path_expr()`](src/rpq/nfarpq.rs)).
2. Eliminating ε-transitions via epsilon closure ([`NfaBuilder::epsilon_closure()`](src/rpq/nfarpq.rs)).
3. Building one `LAGraph_Graph` per NFA label transition
   ([`Nfa::build_lagraph_matrices()`](src/rpq/nfarpq.rs)).
4. Calling [`LAGraph_RegularPathQuery`] with the NFA matrices, data-graph
   matrices, start/final states, and source vertices.

`type Result = NfaRpqResult` ([`GraphblasVector`] of reachable targets).

Supported path operators match [`PathExpr`] variants above. `Reverse` and
`NegatedPropertySet` from SPARQL map to [`RpqError::UnsupportedPath`] when they
appear in extracted paths.

Subject/object resolution: [`Endpoint::Variable`] means "all vertices";
[`Endpoint::Named`] resolves to a single vertex via
[`GraphDecomposition::get_node_id()`](src/graph/mod.rs:200).

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

### CLI dispatch (`src/cli/dispatch.rs`)

With the `bench` feature enabled, [`src/cli/dispatch.rs`](src/cli/dispatch.rs) is the single
mapping from [`Algo`](src/cli/args.rs) variants to concrete evaluator types. `dispatch_query`
and `dispatch_bench` each perform one exhaustive `match` per requested algorithm, then call
generic runners (`run_query_for_evaluator<E>` and `run_bench_for_evaluator<E>`) that are
monomorphized for the selected evaluator.

Adding a new algorithm requires a new `Algo` variant, its `Display` arm, one `dispatch_query`
arm, one `dispatch_bench` arm, an `impl Evaluator` for the evaluator type, and an
`impl ResultCount` for any result type used by CLI count reporting.

### FFI layer

[`lagraph_sys`](src/lagraph_sys.rs) exposes raw C bindings for GraphBLAS and
LAGraph. Safe Rust wrappers live in [`graph::mod`](src/graph/mod.rs):

- [`LagraphGraph`](src/graph/mod.rs:48) — RAII wrapper around `LAGraph_Graph` (calls
  `LAGraph_Delete` on drop). Also provides
  [`LagraphGraph::from_coo()`](src/graph/mod.rs:85) to build directly from COO arrays.
- [`GraphblasVector`](src/graph/mod.rs:128) — RAII wrapper around `GrB_Vector`
  (derives `Debug`).
- [`GraphblasMatrix`](src/graph/mod.rs) — RAII wrapper around `GrB_Matrix` (`dup` + `free` on drop).
- [`ensure_grb_init()`](src/graph/wrappers.rs:11) — internal one-time `LAGraph_Init` via
  `std::sync::Once`. Called automatically by RAII-wrapped constructors
  (`LagraphGraph::from_coo`, `LagraphGraph::from_matrix`, `ThreadScope::enter`) and by
  `load_mm_file`. Crate-private; no other code should call it.

### Macros & helpers (`src/utils.rs`)

Two `#[macro_export]` macros handle FFI error mapping:

- [`grb_ok!(expr)`](src/utils.rs:138) — evaluates a GraphBLAS call inside `unsafe`, maps the
  `i32` return to `Result<(), GraphError::GraphBlas(info)>`.
- [`la_ok!(fn::path(args…))`](src/utils.rs:167) — evaluates a LAGraph call, automatically
  appending the required `*mut i8` message buffer, and maps failure to
  `GraphError::LAGraph(info, msg)`.

A convenience function is also provided:

- [`build_graph(edges)`](src/utils.rs:184) — builds an `InMemoryGraph` from a
  slice of `(&str, &str, &str)` triples (source, target, label). Used by
  integration tests.

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
  `graph/inmemory.rs`, `rpq/nfarpq.rs`. All raw pointers are wrapped in
  RAII types that free resources on drop.
- `unsafe impl Send + Sync` is provided for `LagraphGraph`,
  `GraphblasVector`, and `GraphblasMatrix` because GraphBLAS handles are thread-safe after init.
- Unit tests live in `#[cfg(test)] mod tests` blocks inside each module.
  Integration tests that need GraphBLAS live in [`tests/inmemory_tests.rs`](tests/inmemory_tests.rs),
  [`tests/mm_tests.rs`](tests/mm_tests.rs), [`tests/nfarpq_tests.rs`](tests/nfarpq_tests.rs).

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

Tests in `src/formats/csv.rs` and `src/formats/rdf.rs` are pure Rust and need no native dependencies.

Tests in `src/sparql/mod.rs` are pure Rust and need no native dependencies.

Tests in `src/rpq/nfarpq.rs` (NFA construction unit tests) are pure Rust and need no
native dependencies.

Tests in `src/graph/inmemory.rs`, [`tests/inmemory_tests.rs`](tests/inmemory_tests.rs),
[`tests/mm_tests.rs`](tests/mm_tests.rs), [`tests/nfarpq_tests.rs`](tests/nfarpq_tests.rs),
and [`tests/rpqmatrix_tests.rs`](tests/rpqmatrix_tests.rs) call real GraphBLAS/LAGraph and
require the native libraries to be present.

## CI

The GitHub Actions workflow ([`.github/workflows/ci.yml`](.github/workflows/ci.yml))
runs on every push and PR across `stable`, `beta`, and `nightly` toolchains:

1. Checks out with `submodules: recursive` and `lfs: true`.
2. Installs `cmake`, `libclang-dev`, `clang` via apt.
3. `cargo build --workspace --features pathrex-sys/regenerate-bindings` —
   `pathrex-sys/build.rs` clones GraphBLAS at the pinned tag, builds it
   statically, builds LAGraph statically against it, and regenerates FFI
   bindings.
4. `cargo test --workspace --verbose` — runs the full test suite. No
   `LD_LIBRARY_PATH` is needed because GraphBLAS and LAGraph are linked
   statically; only the OpenMP runtime (`libgomp`) is dynamic and is
   already on the default loader path.
