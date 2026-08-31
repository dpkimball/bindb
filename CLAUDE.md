# CLAUDE.md

Guidance for agents working in `bindb`.

## What this crate is

Native embedded database and unified storage engine: objects (versioned documents), relationships (directed graph edges), bitemporal history, vectors (HNSW embedding search), and metric-typed observations. The WAL is the only canonical store — every engine is a rebuildable projection of the commit log.

## What it is not

Do not add domain logic, application-level schemas, or business rules — those belong in `binder-core`. Do not add a network layer, HTTP server, or gRPC service — bindb is embedded only; the server lives in `binder-server`. Do not treat indexes as authoritative; they are projections of the WAL and can be dropped and rebuilt without data loss. Do not add cross-database orchestration or join logic.

## Layout

```text
src/
  wal.rs           # commit log — source of truth; all writes go through here
  transaction.rs   # Transaction builder; changes accumulated then committed atomically
  commit.rs        # CommitId, commit chain, parent linkage
  object.rs        # ObjectId, BinderObjectVersion, HEAD lookup
  edge.rs          # EdgeId, BinderEdgeVersion, directed relationships
  query.rs         # get, list, traverse, semantic_query public surface
  vector.rs        # vector index interface
  vector_hnsw.rs   # HNSW approximate nearest-neighbor implementation
  graph_query.rs   # pattern-matched subgraph queries
  index/           # index projections (rebuildable from WAL)
  engine.rs        # coordination across object/graph/temporal/vector engines
  version_store.rs # bitemporal versioned storage
  namespace.rs     # scheme://path ObjectId parsing and namespace routing
  provenance.rs    # source attribution on every write
  types.rs         # shared primitive types
  error.rs         # error types
  lib.rs           # public API surface
```

## Build

```sh
cargo test --locked --all
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
make b
```

## Key constraints

- WAL is authoritative; indexes are projections only — never treat an index as the source of truth
- `commit_buffered` + `flush()` for bulk ingestion; `commit()` for single durable writes
- Never log object payloads or edge content — structural metadata only (IDs, timestamps, actor)
- Published to private Kellnr (`CARGO_REGISTRY=dpk`); `env.sh` sets `PUBLISH_CRATE=true` for `make b`
