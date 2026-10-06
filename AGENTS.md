# AGENTS.md — Pathrex

## Project Overview

**Pathrex** is a Rust library and CLI tool for benchmarking queries on edge-labeled graphs
constrained by regular languages and context-free languages.
It uses **SuiteSparse:GraphBLAS** (via **LAGraph**) for sparse Boolean matrix operations and
decomposes a graph by edge label into one Boolean adjacency matrix per label.

## Repository Layout

Paths below are relative to the Pathrex workspace root, not to the enclosing
RPQ_bench repository.

```text
.
├── Cargo.toml                    # Workspace, edition 2024, MSRV 1.90
├── pathrex/
│   ├── Cargo.toml                # Rust API; optional bench feature enables the CLI
│   ├── src/
│   │   ├── lib.rs                # Public modules and pathrex_sys re-export
│   │   ├── bin/pathrex.rs        # CLI entry point (requires bench)
│   │   ├── cli/                  # Arguments, loading, dispatch, benchmarking, output
│   │   ├── eval/mod.rs           # Evaluator, PreparedEvaluator, ResultCount
│   │   ├── graph/
│   │   │   ├── mod.rs            # Graph traits, backend handle, errors
│   │   │   ├── inmemory.rs       # In-memory graph, CSR/CSC stores, metadata
│   │   │   └── wrappers.rs       # Native RAII wrappers and initialization
│   │   ├── rpq/
│   │   │   ├── mod.rs            # Endpoint, PathExpr, RpqQuery, errors
│   │   │   ├── nfarpq.rs         # NFA evaluator
│   │   │   └── rpqmatrix/
│   │   │       ├── eval.rs       # RPQMatrix preparation
│   │   │       ├── expr.rs       # Query translation and native plan materialization
│   │   │       ├── plan.rs       # RpqPlan language and rewrite rules
│   │   │       ├── optimize.rs   # Strategy selection, EGraphOptimizer, extraction
│   │   │       ├── cost.rs       # Cost models
│   │   │       ├── sampling.rs   # Induced-subgraph sampling
│   │   │       ├── stats.rs      # Count-vector statistics
│   │   │       └── result.rs     # PreparedRpqMatrix and execution results
│   │   ├── sparql/mod.rs         # SPARQL to RpqQuery
│   │   ├── formats/              # CSV, MatrixMarket, RDF sources
│   │   └── utils.rs              # FFI error macros and test helpers
│   └── tests/                    # Integration tests and Git LFS fixtures
├── pathrex-sys/
│   ├── Cargo.toml
│   ├── build.rs                  # Fetch/build GraphBLAS, build/link LAGraph
│   ├── src/
│   │   ├── lib.rs                # Raw FFI API
│   │   └── lagraph_sys_generated.rs
│   └── deps/LAGraph/             # Native dependency submodule with RPQ extensions
└── .github/workflows/            # CI and release workflows
```

## Build & Dependencies

The [local build guide](README.md#local-build-linux) is the canonical source of
installation and build commands. Run them from this workspace root.

- MSRV: **Rust 1.90**, inherited by both crates from the workspace manifest.
  Edition 2024 alone would require 1.85, but the current resolved dependency
  set includes `ordered-float 5.5.0` (MSRV 1.90) and RDF/SPARQL dependencies
  requiring 1.87.
- Resolver 3 prefers dependency versions compatible with the declared MSRV.
- Native prerequisites: Git, CMake, a C/C++ compiler and an OpenMP runtime.
  The documented Linux configuration uses GCC and `libgomp`.
- Git LFS is needed to obtain integration-test fixtures.
- `cargo build --workspace` builds the libraries.
  `cargo build --release --features bench` also builds
  `target/release/pathrex`. The CLI is not built without `bench`.
- `pathrex-sys/build.rs` fetches GraphBLAS at `GRAPHBLAS_TAG`
  (currently `v10.3.1`) and builds GraphBLAS and LAGraph statically.
  A system installation of either library and `LD_LIBRARY_PATH` are not required.
  The first native build requires network access; later builds reuse
  `$OUT_DIR/graphblas-src/` and CMake build artifacts.
- `pathrex-sys/regenerate-bindings` is optional and requires Clang/libclang.
  It rewrites
  [`pathrex-sys/src/lagraph_sys_generated.rs`](pathrex-sys/src/lagraph_sys_generated.rs)
  using the bundled LAGraph headers and the just-built GraphBLAS headers.
  Ordinary builds use the checked-in bindings. **Do not hand-edit them.**
- `DOCS_RS` skips native building for rustdoc; it is not an option for building
  a runnable binary.
- `Cargo.lock` is currently ignored. Preserve it for experiments and use
  `--locked` after the initial resolution to retain the dependency set.

## Architecture & Key Abstractions

### Edge

[`Edge`](pathrex/src/graph/mod.rs) is the universal currency between format parsers and graph
builders: `{ source: String, target: String, label: String }`.

### GraphSource trait

[`GraphSource<B>`](pathrex/src/graph/mod.rs) is implemented by any data source that knows how to
feed itself into a specific [`GraphBuilder`]:

- [`apply_to(self, builder: B) -> Result<B, B::Error>`](pathrex/src/graph/mod.rs) — consumes the
  source and returns the populated builder.

[`Csv<R>`](pathrex/src/formats/csv.rs), [`MatrixMarket`](pathrex/src/formats/mm.rs), and [`Rdf`](pathrex/src/formats/rdf.rs)
implement `GraphSource<InMemoryBuilder>` (see [`pathrex/src/graph/inmemory.rs`](pathrex/src/graph/inmemory.rs)), so they
can be passed to [`GraphBuilder::load`] and [`Graph::try_from`].

### GraphBuilder trait

[`GraphBuilder`](pathrex/src/graph/mod.rs) accumulates edges and produces a
[`GraphDecomposition`](pathrex/src/graph/mod.rs):

- [`load<S: GraphSource<Self>>(self, source: S)`](pathrex/src/graph/mod.rs) — primary entry point;
  delegates to `GraphSource::apply_to`.
- [`build(self)`](pathrex/src/graph/mod.rs) — finalise into an immutable graph.

`InMemoryBuilder` also exposes lower-level helpers outside the trait:

- [`push_edge(&mut self, edge: Edge)`](pathrex/src/graph/inmemory.rs) — ingest one edge.
- [`with_stream<I, E>(self, stream: I)`](pathrex/src/graph/inmemory.rs) — consume an
  `IntoIterator<Item = Result<Edge, E>>`.

The MatrixMarket loader uses crate-private `extend_prebuilt`,
`extend_prebuilt_csc`, and `extend_metadata` helpers to install already loaded
matrices and their statistics.

### Backend trait & Graph\<B\> handle

[`Backend`](pathrex/src/graph/mod.rs) associates a marker type with a concrete builder/graph pair:

```rust
pub trait Backend {
    type Graph: GraphDecomposition;
    type Builder: GraphBuilder<Graph = Self::Graph>;
}
```

[`Graph<B>`](pathrex/src/graph/mod.rs) is a zero-sized handle parameterised by a `Backend`:

- [`Graph::<InMemory>::builder()`](pathrex/src/graph/mod.rs) — returns a fresh `InMemoryBuilder`.
- [`Graph::<InMemory>::try_from(source)`](pathrex/src/graph/mod.rs) — builds a graph from a single
  source in one call.

[`InMemory`](pathrex/src/graph/inmemory.rs) is the concrete backend marker type.

### GraphDecomposition trait

[`GraphDecomposition`](pathrex/src/graph/mod.rs) is the read-only query interface:

- [`get_graph(label)`](pathrex/src/graph/mod.rs) — returns `Arc<LagraphGraph>` for a given edge label.
- [`get_node_id(string_id)`](pathrex/src/graph/mod.rs) / [`get_node_name(mapped_id)`](pathrex/src/graph/mod.rs) — bidirectional string ↔ integer dictionary.
- [`num_nodes()`](pathrex/src/graph/mod.rs) — total unique nodes.
- [`get_graph_with_storage(label, storage)`](pathrex/src/graph/mod.rs) — preferred
  CSR/CSC orientation, defaulting to `get_graph` for backends without both.
- [`get_metadata()`](pathrex/src/graph/mod.rs) — optional cached matrix statistics.

### Generic evaluator abstraction (`pathrex/src/eval/`)

[`pathrex/src/eval/mod.rs`](pathrex/src/eval/mod.rs) defines query-language-agnostic evaluator traits:

- [`Evaluator`](pathrex/src/eval/mod.rs) uses associated types for `Query`, `Result`, `Error`, and
  `Prepared`. The graph backend stays a method-level generic (`G: GraphDecomposition`) so one
  evaluator type can run against any graph backend selected at the call site.
- [`PreparedEvaluator`](pathrex/src/eval/mod.rs) represents prepared `(query, graph)` state that can be
  executed repeatedly, which is used by benchmark timing loops.
- [`ResultCount`](pathrex/src/eval/mod.rs) is separate from `Evaluator::Result`; only CLI runners that
  need counts require this bound, leaving room for future evaluators with richer result types.

### InMemoryBuilder / InMemoryGraph

[`InMemoryBuilder`](pathrex/src/graph/inmemory.rs) is the primary `GraphBuilder` implementation.
It collects edges in RAM, then [`build()`](pathrex/src/graph/inmemory.rs) calls
GraphBLAS to create one `GrB_Matrix` per label via COO format, wraps each in an
`LAGraph_Graph`, and returns an [`InMemoryGraph`](pathrex/src/graph/inmemory.rs).

Multiple CSV sources can be chained with repeated `.load()` calls; all edges are merged
into a single graph.

The MatrixMarket loader populates both CSR and CSC matrix stores, along with
`GraphMetadata` containing a `MatrixMetadata` per label: dimension, nonzero
count, and numbers of nonempty rows and columns. `MatrixStatsMode::Basic`
adds row/column count vectors; `Extended` also adds the MNC singleton-related
vectors. The CLI requests `Extended` for MNC and `None` for other strategies.
CSV/RDF loading does not populate these cached statistics or a separate CSC store;
consumers use matrix/statistics fallbacks when needed.

**Node ID representation:** Internally, `InMemoryBuilder` uses `HashMap<usize, String>` for
`id_to_node` (changed from `Vec<String>` to support sparse/pre-assigned IDs from MatrixMarket).
The [`set_node_map()`](pathrex/src/graph/inmemory.rs) method allows bulk-installing a node mapping,
which is used by the MatrixMarket loader.

### Format parsers

Three data sources implement `GraphSource<InMemoryBuilder>` and can be passed to
`GraphBuilder::load()` (see [`pathrex/src/graph/inmemory.rs`](pathrex/src/graph/inmemory.rs)).
CSV and RDF provide edge streams; MatrixMarket loads matrices directly.

#### `Csv<R>`

[`Csv<R>`](pathrex/src/formats/csv.rs) parses delimiter-separated edge files.

Configuration is via [`CsvConfig`](pathrex/src/formats/csv.rs):

| Field | Default | Description |
|---|---|---|
| `source_column` | `Index(0)` | Column for the source node (by index or name) |
| `target_column` | `Index(1)` | Column for the target node |
| `label_column` | `Index(2)` | Column for the edge label |
| `has_header` | `true` | Whether the first row is a header |
| `delimiter` | `b','` | Field delimiter byte |

[`ColumnSpec`](pathrex/src/formats/csv.rs) is either `Index(usize)` or `Name(String)`.
Name-based lookup requires `has_header: true`.

#### MatrixMarket directory format

[`MatrixMarket`](pathrex/src/formats/mm.rs) loads an edge-labeled graph from a directory with:

- `vertices.txt` — one line per node: `<node_name> <1-based-index>` on disk; [`get_node_id`](pathrex/src/graph/mod.rs) returns the matching **0-based** matrix index
- `edges.txt` — one line per label: `<label_name> <1-based-index>` (selects `n.txt`)
- `<n>.txt` — MatrixMarket adjacency matrix for label with index `n`

Names in mapping files may be written with SPARQL-style angle brackets (e.g. `<Article1>`).
[`parse_index_map`](pathrex/src/formats/mm.rs) strips a single pair of surrounding `<`/`>` so
dictionary keys match short labels (`Article1`), aligning with IRIs after
[`RpqQuery::strip_base`](pathrex/src/rpq/mod.rs) on SPARQL-derived queries.

The loader uses [`LAGraph_MMRead`](pathrex-sys/src/lib.rs) to parse each `.txt` file into a
`GrB_Matrix`, then wraps it in an `LAGraph_Graph`. Vertex indices from `vertices.txt` are
converted to 0-based and installed via [`InMemoryBuilder::set_node_map()`](pathrex/src/graph/inmemory.rs).

Helper functions:

- [`load_mm_file(path)`](pathrex/src/graph/wrappers.rs) — reads a single MatrixMarket file
  into a `GraphblasMatrix`; re-exported by `formats::mm`.
- [`parse_index_map(path)`](pathrex/src/formats/mm.rs) — parses `<name> <index>` lines; indices must be **>= 1** and **unique** within the file.

`MatrixMarket` implements `GraphSource<InMemoryBuilder>` in [`pathrex/src/graph/inmemory.rs`](pathrex/src/graph/inmemory.rs): `vertices.txt` maps are converted from 1-based file indices to 0-based matrix ids before [`set_node_map`](pathrex/src/graph/inmemory.rs); `edges.txt` indices are unchanged for `n.txt` lookup.

#### `Rdf` — Unified RDF Parser

[`Rdf`](pathrex/src/formats/rdf.rs) is a unified parser for RDF formats using `oxttl` and `oxrdf`.
It supports both **N-Triples** (`.nt`) and **Turtle** (`.ttl`) formats via the [`RdfFormat`](pathrex/src/formats/rdf.rs) enum.

Each triple `(subject, predicate, object)` becomes an [`Edge`](pathrex/src/graph/mod.rs) where:

- `source` — subject IRI or blank-node ID (`_:label`).
- `target` — object IRI or blank-node ID; triples whose object is an RDF
  literal yield `Err(FormatError::LiteralAsNode)` (callers may filter these out).
- `label` — full predicate IRI string (including fragment `#…` when present).

Constructor:

- [`Rdf::from_path(path)`](pathrex/src/formats/rdf.rs) — auto-detects format from file extension (`.nt` → N-Triples, `.ttl` → Turtle). Parses in parallel using memory-mapping and rayon.

Format detection via [`RdfFormat::from_path(path)`](pathrex/src/formats/rdf.rs):

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

### SPARQL parsing (`pathrex/src/sparql/mod.rs`)

The [`sparql`](pathrex/src/sparql/mod.rs) module uses the [`spargebra`](https://crates.io/crates/spargebra)
crate to parse SPARQL 1.1 query strings and build a pathrex-native [`RpqQuery`](pathrex/src/rpq/mod.rs)
for RPQ evaluators.

**Supported query form:** `SELECT` queries with exactly one triple or property
path pattern in the `WHERE` clause. Relative IRIs such as `<knows>` require a
`BASE` declaration (or `PREFIX` / full IRIs). Example:

```sparql
BASE <http://example.org/>
SELECT ?x ?y WHERE { ?x <knows>/<likes>* ?y . }
```

Key public items:

- [`parse_rpq(sparql)`](pathrex/src/sparql/mod.rs) — parses a SPARQL string with
  `SparqlParser` and returns an [`RpqQuery`](pathrex/src/rpq/mod.rs).
- [`extract_rpq(query)`](pathrex/src/sparql/mod.rs) — validates a parsed [`spargebra::Query`] is a
  `SELECT` with a single path pattern and returns an [`RpqQuery`](pathrex/src/rpq/mod.rs).
  Use this when you construct a custom [`SparqlParser`](https://docs.rs/spargebra) (e.g. with
  prefix declarations) and call `parse_query` yourself.
- [`ExtractError`](pathrex/src/sparql/mod.rs) — error enum for extraction failures
  (`NotSelect`, `NotSinglePath`, `UnsupportedSubject`, `UnsupportedObject`,
  `VariablePredicate`). Converts to [`RpqError::Extract`](pathrex/src/rpq/mod.rs) via `#[from]`.

Call [`RpqQuery::strip_base`](pathrex/src/rpq/mod.rs) when graph edge labels are short names
and the parsed query contains full IRIs sharing a common prefix.

The module handles spargebra's desugaring of sequence paths (`?x <a>/<b>/<c> ?y`)
from a chain of BGP triples back into a single path expression.

### RPQ evaluation (`pathrex/src/rpq/`)

The [`rpq`](pathrex/src/rpq/mod.rs) module provides an abstraction for evaluating
Regular Path Queries (RPQs) over edge-labeled graphs using GraphBLAS/LAGraph.

Key public items:

- [`Endpoint`](pathrex/src/rpq/mod.rs) — `Variable(String)` or `Named(String)` (IRI string).
- [`PathExpr`](pathrex/src/rpq/mod.rs) — `Label`, `Sequence`, `Alternative`, `ZeroOrMore`,
  `OneOrMore`, `ZeroOrOne`.
- [`RpqQuery`](pathrex/src/rpq/mod.rs) — `{ subject, path, object }` using the types above;
  [`strip_base(&mut self, base)`](pathrex/src/rpq/mod.rs) removes a shared IRI prefix from
  named endpoints and labels.
- [`RpqEvaluator`](pathrex/src/rpq/mod.rs) — marker subtrait over
  [`Evaluator<Query = RpqQuery, Error = RpqError>`](pathrex/src/eval/mod.rs), preserving the RPQ-facing
  trait name while the generic evaluator hierarchy lives in `pathrex/src/eval/`.
- [`PreparedRpq`](pathrex/src/rpq/mod.rs) — marker subtrait over
  [`PreparedEvaluator<Error = RpqError>`](pathrex/src/eval/mod.rs).
- [`RpqError`](pathrex/src/rpq/mod.rs) — unified error type for RPQ parsing and evaluation:
  `Parse` (SPARQL syntax), `Extract` (query extraction), `UnsupportedPath`,
  `VertexNotFound`, and `Graph` (wraps [`GraphError`](pathrex/src/graph/mod.rs) for
  label-not-found and GraphBLAS/LAGraph failures).

#### `NfaRpqEvaluator` (`pathrex/src/rpq/nfarpq.rs`)

[`NfaRpqEvaluator`](pathrex/src/rpq/nfarpq.rs) implements [`RpqEvaluator`] by:

1. Building a finite-state machine from `PathExpr` with `rustfst` concatenation,
   union, and closure operations in `build_fst`.
2. Removing ε-transitions with `rustfst::rm_epsilon` and extracting an
   [`Nfa`](pathrex/src/rpq/nfarpq.rs) in `Nfa::from_path_expr`.
3. Building one `LAGraph_Graph` per NFA label transition
   ([`Nfa::build_lagraph_matrices()`](pathrex/src/rpq/nfarpq.rs)).
4. Calling [`LAGraph_RegularPathQuery`] with the NFA matrices, data-graph
   matrices, start/final states, and source vertices.

`type Result = NfaRpqResult` ([`GraphblasVector`] of reachable targets).

Supported path operators match [`PathExpr`] variants above. `Reverse` and
`NegatedPropertySet` from SPARQL map to [`RpqError::UnsupportedPath`] when they
appear in extracted paths.

Subject/object resolution: [`Endpoint::Variable`] means "all vertices";
[`Endpoint::Named`] resolves to a single vertex via
[`GraphDecomposition::get_node_id()`](pathrex/src/graph/mod.rs).
The source constraint selects starting vertices; a fixed object filters the
reachable-target vector after the native call.

[`NfaRpqResult`](pathrex/src/rpq/nfarpq.rs) wraps a [`GraphblasVector`] of reachable **target**
vertices. When the subject is a variable, every vertex is used as a source and
`LAGraph_RegularPathQuery` returns the union of targets — individual `(source, target)`
pairs are not preserved.

#### RpqMatrixEvaluator (`pathrex/src/rpq/rpqmatrix/`)

The [RPQMatrix module](pathrex/src/rpq/rpqmatrix/mod.rs) is a directory, not the
former `rpqmatrix.rs` file. Its pipeline is:

1. [`RpqMatrixEvaluator::prepare`](pathrex/src/rpq/rpqmatrix/eval.rs) selects a
   storage orientation and translates the query with `query_to_expr`.
2. [`query_to_expr`](pathrex/src/rpq/rpqmatrix/expr.rs) builds a
   `RecExpr<RpqPlan>`. A label is a Boolean adjacency matrix; sequence is
   Boolean multiplication; alternative is union; star is reflexive transitive
   closure. One-or-more is translated to a sequence with star.
   Zero-or-one is currently rejected by RPQMatrix.
3. [`EGraphOptimizer`](pathrex/src/rpq/rpqmatrix/optimize.rs) implements the
   internal `RpqOptimizer` interface. Except for `NoOpt`, it expands equivalent
   expressions with `egg::Runner` and the rules in
   [`plan.rs`](pathrex/src/rpq/rpqmatrix/plan.rs), then selects an expression
   with `egg::Extractor` and a cost model from
   [`cost.rs`](pathrex/src/rpq/rpqmatrix/cost.rs).
   The cost model is used for extraction, not to execute the final query.
4. [`materialize_with_storage`](pathrex/src/rpq/rpqmatrix/expr.rs) turns the
   selected expression into a flat `Vec<RPQMatrixPlan>`, borrowing graph matrices
   and creating owned endpoint-selector matrices.
5. [`PreparedRpqMatrix::execute`](pathrex/src/rpq/rpqmatrix/result.rs) calls
   `LAGraph_RPQMatrix`, duplicates the result matrix and releases temporary plan
   results. It can be called repeatedly without rerunning optimization.

**Fixed endpoints are supported and filter the result.** For a path relation
`R`, a named vertex `v` is represented by a diagonal selector `D_v` with a
single true entry at `(v, v)`:

| Subject / object | Matrix expression |
|---|---|
| variable / variable | `R` |
| named `u` / variable | `D_u × R` |
| variable / named `v` | `R × D_v` |
| named `u` / named `v` | `D_u × R × D_v` |

An unknown named vertex produces `RpqError::VertexNotFound`.
For a variable subject and fixed object, preparation requests **CSC** matrices;
other endpoint combinations request **CSR**. This changes storage, not query
semantics. The MatrixMarket loader stores both orientations.
Backends without a CSC copy may return their default matrix through
`GraphDecomposition::get_graph_with_storage`.

[`RpqMatrixResult`](pathrex/src/rpq/rpqmatrix/result.rs) contains the filtered
relation matrix and its pair count `nnz`. Its `ResultCount` implementation
reports the number of distinct reachable targets (nonempty columns), not the
pair count.

The implemented CLI strategies are `none`, `join`, `metaac`, `mnc`,
`hybrid`, `pang-hybrid`, and `sampling`. `RandomOpt`, `Simple`, and
`Wander` remain unsupported internal enum variants. Sampling evaluates
induced-subgraph matrices for estimates; the final selected plan executes on
the full graph.

Fixed-object and fixed-subject/object behavior is covered by
[`pathrex/tests/rpqmatrix_tests.rs`](pathrex/tests/rpqmatrix_tests.rs);
[`eval.rs`](pathrex/src/rpq/rpqmatrix/eval.rs) also tests optimized fixed-endpoint
queries.

### CLI dispatch (`pathrex/src/cli/dispatch.rs`)

With the `bench` feature enabled, [`pathrex/src/cli/dispatch.rs`](pathrex/src/cli/dispatch.rs) is the single
mapping from [`Algo`](pathrex/src/cli/args.rs) variants to concrete evaluator types. `dispatch_query`
and `dispatch_bench` each perform one exhaustive `match` per requested algorithm, then call
generic runners (`run_query_for_evaluator<E>` and `run_bench_for_evaluator<E>`) that are
monomorphized for the selected evaluator.

Adding a new algorithm requires a new `Algo` variant, its `Display` arm, one `dispatch_query`
arm, one `dispatch_bench` arm, an `impl Evaluator` for the evaluator type, and an
`impl ResultCount` for any result type used by CLI count reporting.

### FFI layer

[`lagraph_sys`](pathrex-sys/src/lib.rs) exposes raw C bindings for GraphBLAS and
LAGraph. The API is re-exported as `pathrex::lagraph_sys` by
[`pathrex/src/lib.rs`](pathrex/src/lib.rs). Rust RAII wrappers live in
[`graph::wrappers`](pathrex/src/graph/wrappers.rs):

- [`LagraphGraph`](pathrex/src/graph/wrappers.rs) — RAII wrapper around `LAGraph_Graph` (calls
  `LAGraph_Delete` on drop). Also provides
  [`LagraphGraph::from_coo()`](pathrex/src/graph/wrappers.rs) to build directly from COO arrays.
- [`GraphblasVector`](pathrex/src/graph/wrappers.rs) — RAII wrapper around `GrB_Vector`
  (derives `Debug`).
- [`GraphblasMatrix`](pathrex/src/graph/wrappers.rs) — RAII wrapper around `GrB_Matrix` (`dup` + `free` on drop).
- [`ensure_grb_init()`](pathrex/src/graph/wrappers.rs) — internal one-time `LAGraph_Init` via
  `std::sync::Once`. Called automatically by RAII-wrapped constructors
  (`LagraphGraph::from_coo`, `LagraphGraph::from_matrix`, `ThreadScope::enter`) and by
  `load_mm_file`. Crate-private; no other code should call it.

### Macros & helpers (`pathrex/src/utils.rs`)

Two `#[macro_export]` macros handle FFI error mapping:

- [`grb_ok!(expr)`](pathrex/src/utils.rs) — evaluates a GraphBLAS call inside `unsafe`, maps the
  `i32` return to `Result<(), GraphError::GraphBlas(info)>`.
- [`la_ok!(fn::path(args…))`](pathrex/src/utils.rs) — evaluates a LAGraph call, automatically
  appending the required `*mut i8` message buffer, and maps failure to
  `GraphError::LAGraph(info, msg)`.

A convenience function is also provided:

- [`build_graph(edges)`](pathrex/src/utils.rs) — builds an `InMemoryGraph` from a
  slice of `(&str, &str, &str)` triples (source, target, label). Used by
  integration tests.

## Coding Conventions

- **Rust edition 2024; MSRV 1.90**.
- Error handling via `thiserror` derive macros; three main error enums:
  [`GraphError`](pathrex/src/graph/mod.rs), [`FormatError`](pathrex/src/formats/mod.rs),
  and [`RpqError`](pathrex/src/rpq/mod.rs).
- `FormatError` converts into `GraphError` via `#[from] FormatError` on the
  `GraphError::Format` variant.
- `GraphError` converts into `RpqError` via `#[from] GraphError` on the
  `RpqError::Graph` variant, enabling `?` propagation in evaluators.
- Raw FFI declarations live in `pathrex-sys`. Native calls also occur in the graph
  wrappers, the in-memory loader, NFA evaluation, and RPQMatrix expression,
  statistics, sampling, and execution modules. Resource ownership and cleanup
  must be explicit; prefer the existing RAII wrappers.
- `unsafe impl Send + Sync` is provided for `LagraphGraph`,
  `GraphblasVector`, and `GraphblasMatrix` because GraphBLAS handles are thread-safe after init.
- Unit tests live in `#[cfg(test)] mod tests` blocks inside each module.
  Integration tests that need GraphBLAS live in [`pathrex/tests/inmemory_tests.rs`](pathrex/tests/inmemory_tests.rs),
  [`pathrex/tests/mm_tests.rs`](pathrex/tests/mm_tests.rs), [`pathrex/tests/nfarpq_tests.rs`](pathrex/tests/nfarpq_tests.rs).

## Testing

From the workspace root:

```bash
git lfs pull
cargo test --workspace --features bench
cargo clippy --workspace --all-targets --features bench -- -D warnings
```

No `LD_LIBRARY_PATH` is needed for the statically built GraphBLAS and LAGraph.
Native libraries are built automatically by the sys crate even when running
tests with Cargo. Integration fixtures are under
[`pathrex/tests/testdata/`](pathrex/tests/testdata).

Focused regression checks for fixed endpoints:

```bash
cargo test -p pathrex --features bench --test rpqmatrix_tests test_bound
cargo test -p pathrex --features bench optimizers_preserve_results
```

Use `--locked` when reproducing a previously resolved build.

## CI

The GitHub Actions workflow ([`.github/workflows/ci.yml`](.github/workflows/ci.yml))
runs on every push and PR across `stable`, `beta`, and `nightly` toolchains:

1. Checks out with `submodules: recursive` and `lfs: true`.
2. Installs `cmake`, `libclang-dev`, `clang` via apt.
3. `cargo build --workspace --features pathrex-sys/regenerate-bindings` —
   `pathrex-sys/build.rs` clones GraphBLAS at the pinned tag, builds it
   statically, builds LAGraph statically against it, and regenerates FFI
   bindings.
4. `cargo test --workspace --verbose` — runs tests without the optional CLI
   feature; CLI tests require `--features bench`. No
   `LD_LIBRARY_PATH` is needed because GraphBLAS and LAGraph are linked
   statically; only the OpenMP runtime (`libgomp`) is dynamic and is
   already on the default loader path.

## Releasing

Pushing a `v*.*.*` tag triggers [`.github/workflows/release.yml`](.github/workflows/release.yml):

1. **`docker` job** — builds the image once and pushes the same tags
   (`{version}`, `{major}.{minor}`, `{major}`, `latest`) to two
   registries:
   - `ghcr.io/sparselinearalgebra/pathrex` (authenticated via
     `GITHUB_TOKEN`, no extra setup needed).
   - `docker.io/vanyaglazunov/pathrex` (authenticated via
     `DOCKERHUB_USERNAME` + `DOCKERHUB_TOKEN` repository secrets).
2. **`publish-crates` job** — publishes both crates to crates.io in the
   correct order:
   - Reads `pathrex-sys` version via `cargo metadata`.
   - Skips publishing `pathrex-sys` if that version is already on the
     registry (allows re-tagging when only `pathrex` changed).
   - Polls `cargo search` until the new `pathrex-sys` is indexed
     (up to 5 minutes), then publishes `pathrex` against it.
   - Requires the `CARGO_REGISTRY_TOKEN` repository secret.

### Manual dry-run

To verify the publish path without uploading:

```bash
cargo publish --dry-run -p pathrex-sys
# (pathrex dry-run requires pathrex-sys to be on crates.io first)
```
