# pathrex

[![Crates.io](https://img.shields.io/crates/v/pathrex.svg)](https://crates.io/crates/pathrex)
[![Docs.rs](https://docs.rs/pathrex/badge.svg)](https://docs.rs/pathrex)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/SparseLinearAlgebra/pathrex/actions/workflows/ci.yml/badge.svg)](https://github.com/SparseLinearAlgebra/pathrex/actions/workflows/ci.yml)
[![Container](https://img.shields.io/badge/ghcr.io-pathrex-blue?logo=docker)](https://github.com/SparseLinearAlgebra/pathrex/pkgs/container/pathrex)
[![Docker Hub](https://img.shields.io/docker/v/vanyaglazunov/pathrex?label=docker.io&logo=docker)](https://hub.docker.com/r/vanyaglazunov/pathrex)

**Pathrex** is a Rust library and CLI for evaluating and benchmarking
**Path Queries** over edge-labeled graphs.

## Architecture

The workspace contains the Rust API and CLI in `pathrex/` and the native
build and FFI bindings in `pathrex-sys/`. See the
[architecture and contributor guide](AGENTS.md#architecture--key-abstractions)
for the current module layout and query preparation pipeline.

## Local build (Linux)

The minimum supported Rust version (MSRV) is **1.90**, not merely the version
that introduced edition 2024. The current dependency set includes
`ordered-float 5.5.0`, which requires Rust 1.90; the RDF/SPARQL dependencies
also require a newer compiler than 1.85. Both workspace crates inherit the
`rust-version` declared in the root `Cargo.toml`.
The workspace uses [Cargo resolver 3](https://doc.rust-lang.org/edition-guide/rust-2024/cargo-resolver.html),
which prefers dependency versions compatible with that declared Rust version.

On Debian/Ubuntu, install the native build tools:

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake git git-lfs
rustup toolchain install 1.90.0 --profile minimal
```

Run the following commands **from the Pathrex workspace root** (the directory
containing this README; `Databases/pathrex` when using RPQ_bench):

```bash
git submodule update --init --recursive
git lfs pull

# CLI and benchmark support; the bench feature is required for the binary.
cargo +1.90.0 build --release --features bench
./target/release/pathrex --help
./target/release/pathrex bench --help

# Unit, integration, and documentation tests, including the CLI.
cargo +1.90.0 test --workspace --features bench
```

For library-only development, use `cargo +1.90.0 build --workspace` instead of
the CLI build above.

Git LFS supplies the integration-test fixtures. An ordinary build uses the
checked-in FFI bindings and **does not require Clang/libclang**. The build script
fetches SuiteSparse:GraphBLAS at the pinned tag `v10.3.1`, builds it and the
LAGraph submodule as static libraries, and links them automatically. No
system-wide GraphBLAS/LAGraph installation or `LD_LIBRARY_PATH` is needed;
the GCC OpenMP runtime (`libgomp` on Linux) remains a dynamic dependency.
The first build requires network access for Cargo dependencies and GraphBLAS
and performs a substantial native compilation. Subsequent builds reuse the
files under `target/`. Reduce Cargo's job count with `-j 2` if memory is limited.

When building from the **RPQ_bench root**, the equivalent CLI build is:

```bash
cargo +1.90.0 build --release --manifest-path Databases/pathrex/Cargo.toml --features bench
Databases/pathrex/target/release/pathrex --help
```

`Cargo.lock` is currently ignored by this repository. Preserve the generated
lockfile with the experiment artifacts and use `--locked` for subsequent builds
and tests to keep dependency versions unchanged. The MSRV statement refers to
the current dependency set, not to every future version allowed by the manifests.

### Optional binding regeneration

Only regenerate bindings when the native API changes:

```bash
sudo apt-get install -y clang libclang-dev
cargo +1.90.0 build --workspace --features pathrex-sys/regenerate-bindings
```

This command rewrites `pathrex-sys/src/lagraph_sys_generated.rs`. Do not edit that
file manually. If bindgen reports `stddef.h` missing, check that Clang and
libclang come from compatible installations and remove stale include-path
overrides such as `BINDGEN_EXTRA_CLANG_ARGS`; ordinary builds can use the
checked-in bindings without regenerating them.

## Features

- **Two RPQ evaluators** out of the box:
  - `nfarpq` —  runs `LAGraph_RegularPathQuery`.
  - `rpqmatrix` —  runs `LAGraph_RPQMatrix`.
- **Multiple input formats**: MatrixMarket directories, CSV edge lists, and
  RDF (Turtle / N-Triples).
- **SPARQL frontend**: parses `SELECT` queries with a single triple/property-path
  pattern.
- **RPQMatrix endpoint constraints**: variable endpoints, a fixed subject, a
  fixed object, or both fixed endpoints; endpoint selectors are part of the plan.
- **Benchmarking** with [`criterion`](https://crates.io/crates/criterion):
  per-query timing, JSON output, checkpoint/resume, optional HTML plots.
- **Reusable Rust library** with backend-agnostic `Graph<B>`, `GraphSource`,
  `GraphBuilder`, and generic `Evaluator` traits.

## Quickstart with Docker

A pre-built image is published to GitHub Container Registry on every release tag.

```bash
docker pull ghcr.io/sparselinearalgebra/pathrex:latest
```

The image's entrypoint forwards arguments to the `pathrex` binary.

### Run a query

Mount a directory containing your graph and queries, then call `query`:

```bash
DATA=<path-to-dir-with-graph-and-queries>
docker run --rm \
  -v "${DATA}:/data:ro" \
  ghcr.io/sparselinearalgebra/pathrex:latest \
  query \
    --graph /data/my-graph \
    --format mm \
    --queries /data/queries.txt \
    --algo nfarpq \
    --algo rpqmatrix
```

### Run a benchmark

```bash
DATA=<path-to-dir-with-graph-and-queries>
RESULTS=<path-to-results-dir>
docker run --rm \
  -v "${DATA}:/data:ro" \
  -v "${RESULTS}:/results" \
  ghcr.io/sparselinearalgebra/pathrex:latest \
  bench \
    --graph /data/my-graph \
    --queries /data/queries.txt \
    --algo nfarpq
```

The entrypoint defaults to:

- `--output /results/bench_results.json`
- `--checkpoint /results/bench_checkpoint.json`
- `--criterion-dir /results/criterion`

Pass any of those flags explicitly to override.

## CLI usage

```text
pathrex <SUBCOMMAND> [OPTIONS]

Subcommands:
  query   Run queries once and report result counts
  bench   Benchmark RPQ evaluators with criterion
```

### Common options

| Flag | Description |
|---|---|
| `-g`, `--graph <PATH>` | Path to the graph (directory for `mm`, file for `csv`/`rdf`). |
| `-f`, `--format <mm\|csv\|rdf>` | Input format. Defaults to `mm`. |
| `-q`, `--queries <FILE>` | Queries file (see format below). |
| `-a`, `--algo <nfarpq\|rpqmatrix>` | Algorithm(s). Repeat to run several. |
| `-b`, `--base-iri [<IRI>]` | Optional `BASE <iri>` to prepend to each query. Bare `--base-iri` uses `http://example.org/`. |
| `-p`, `--rpqmatrix-optimizer <NAME>` | RPQMatrix optimizer: `none`, `join`, `metaac`, `mnc`, `hybrid`, `pang-hybrid`, or `sampling`. |


`query` adds `-o, --output <FILE>` to write JSON.

`bench` adds `--output`, `--checkpoint`, `--resume`, `--criterion-dir`,
`--plots`, `--sample-size`, `--warm-up`, `--measurement`. See
`pathrex bench --help` for details.

### Queries file format

One query per line:

```text
<id>,<sparql_pattern>
```

The pattern is wrapped into a full SPARQL query at load time:

- with `--base-iri <iri>`: `BASE <iri> SELECT * WHERE { <pattern> . }`
- without:                 `SELECT * WHERE { <pattern> . }`

Example `queries.txt`:

```text
q1,?x <knows>/<likes>* ?y
q2,?x (<a>|<b>)+ ?y
```

## License

Licensed under the MIT License. See [`LICENSE`](LICENSE).
