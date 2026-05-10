# pathrex

[![CI](https://github.com/SparseLinearAlgebra/pathrex/actions/workflows/ci.yml/badge.svg)](https://github.com/SparseLinearAlgebra/pathrex/actions/workflows/ci.yml)
[![Container](https://img.shields.io/badge/ghcr.io-pathrex-blue?logo=docker)](https://github.com/SparseLinearAlgebra/pathrex/pkgs/container/pathrex)

**Pathrex** is a Rust library and CLI for evaluating and benchmarking
**Path Queries** over edge-labeled graphs.
## Features

- **Two RPQ evaluators** out of the box:
  - `nfarpq` —  runs `LAGraph_RegularPathQuery`.
  - `rpqmatrix` —  runs `LAGraph_RPQMatrix`.
- **Multiple input formats**: MatrixMarket directories, CSV edge lists, and
  RDF (Turtle / N-Triples).
- **SPARQL frontend**: parses `SELECT` queries with a single triple/property-path
  pattern.
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
