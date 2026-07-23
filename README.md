# bindb

**bindb** is DPK’s home-grown domain database for Binder: an append-only, WAL-first
commit log for objects, edges, versions, namespaces, and rebuildable indexes.

Formerly the in-tree `binder-store` crate. Host-only — plugins must not depend on bindb
(see Binder `boundary-check`).

## Invariants

See [`PRINCIPLES.md`](./PRINCIPLES.md). Short version:

- The WAL commit log is the only canonical store
- Commits are immutable once fsync’d
- Every write is WAL-first
- Indexes and projections are derived and rebuildable

## On-disk layout

Per binder workspace (unchanged for compatibility):

```text
{binder}/.binder/store/
```

## Develop

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo bench --bench store_bench
```

## Consumers

Binder host crates (`binder-runtime`, `dpk-*`, `binder-app`) depend on this crate via
a sibling path checkout (`../bindb`) or a published version when available.
