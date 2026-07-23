# Binder Store — Implementation Invariants

These invariants must hold at every point in the implementation. They are not aspirational.
If an implementation choice would violate any invariant, the implementation is wrong.

---

## The Ten Invariants

**I-1. The commit log is the only canonical store.**
Every BinderCommit appended to the WAL is permanent and authoritative. No other structure
is canonical. If the commit log says it happened, it happened.

**I-2. Commits are immutable once written.**
A BinderCommit that has been fsync'd to the WAL is never modified, compacted, or deleted.
There is no UPDATE, no DELETE, no rewrite. The log is append-only forever.

**I-3. Every write is WAL-first.**
The in-memory state is never updated until the WAL entry has been fsync'd to disk.
A commit that is in memory but not on disk does not exist.

**I-4. The dirty marker controls crash recovery.**
The store creates `.dirty` on open and deletes it on clean close. If `.dirty` exists at
open time, the previous shutdown was unclean. WAL replay is mandatory in that case.

**I-5. WAL replay is always safe to run.**
Replaying the WAL on a clean store produces the same result as reading the WAL on a dirty
store. Replay is idempotent. Running it when unnecessary is safe; skipping it when necessary
is a bug.

**I-6. Partial WAL entries are silently dropped.**
If a WAL entry is incomplete (truncated, missing checksum, checksum mismatch), replay stops
at that boundary. The partial entry is never visible. The store state reflects only complete,
checksum-verified entries.

**I-7. No projection is ever canonical.**
HEAD indexes, graph indexes, and all future projections are derived state. They can always
be dropped and rebuilt from the commit log. If a projection disagrees with the commit log,
the commit log is correct.

**I-8. Every projection is independently rebuildable.**
No projection may depend on another projection for its rebuild. Each projection rebuilds
directly from the commit log. Cascading projection dependencies are forbidden.

**I-9. Namespace ownership is enforced at commit time.**
An object may only be written to a namespace owned by the committing actor. A commit that
violates namespace ownership is rejected before it reaches the WAL. (V1: single namespace,
single actor — enforcement is a no-op but the check must exist.)

**I-10. The public API is narrower than the implementation.**
Internal structures (WalEntry, WalEntryData, CheckpointRecord, etc.) are never exposed
through the public crate API. Callers see: BinderStore, BinderCommit, Transaction, CommitId,
ObjectId, EdgeId, StoreError.

---

## Canonical vs Non-Canonical Data

| Data | Canonical? | Notes |
|------|-----------|-------|
| WAL (commit log) | **Yes** | Source of truth; never overwritten |
| BinderCommit in memory | No | Rebuilt from WAL on open |
| HEAD object index | No | Rebuilt from commit log |
| Graph lookup index | No | Rebuilt from commit log |
| Text / vector index | No | Phase 2; rebuilt from commit log |
| Checkpoint record | No | Optimization; commit log is still canonical |

---

## Commit Durability Contract

A `commit()` call returns `Ok(commit_id)` if and only if:
1. The transaction is non-empty (at least one object or edge change).
2. The WAL entry has been written and **fsync'd** to disk.
3. The in-memory commit list and head pointer have been updated.

If any of those three steps fail, `commit()` returns `Err`. The store state after a failed
commit is identical to the state before the commit began — no partial state is ever visible.

---

## WAL Recovery Guarantees

On open with a dirty marker present:

1. Read WAL from the beginning (or from last checkpoint offset if a checkpoint exists).
2. For each entry: verify magic bytes, read entry_id and data_len, read `data_len` bytes,
   verify crc32 over the data bytes, deserialize.
3. If magic, length, or checksum is wrong: stop replay. All entries before this point are
   recovered. The truncated entry and everything after it are discarded.
4. Rebuild in-memory commit list and head pointer from recovered entries.
5. Truncate the WAL file at the last good entry boundary.
6. Write a fresh checkpoint.
7. Delete `.dirty`.

After recovery, the store is in a consistent state. The recovered commits are the exact set
of commits that were successfully fsync'd before the crash.

**Binary WAL entry format:**

```
[4 bytes]  magic: b"BNDR"
[8 bytes]  entry_id: u64 little-endian
[8 bytes]  data_len: u64 little-endian
[N bytes]  data: bincode-serialized WalEntryData (N == data_len)
[4 bytes]  checksum: crc32 of the data bytes only
```

Total overhead per entry: 24 bytes of framing.

---

## Projection Rebuild Rules

1. To rebuild any projection, scan `BinderStore::scan_commits()` from the beginning.
2. Apply each commit's `object_changes` and `relationship_changes` to the projection in
   commit order (ascending by `timestamp_ms`, tie-broken by `commit_id` lexicographic order).
3. Never read another projection during a rebuild.
4. A projection rebuild may be triggered by: store open after crash, explicit
   `rebuild_indexes()` call, or detection of index corruption.
5. A projection is not considered valid until the rebuild completes without error.

---

## Namespace Ownership Rules

- Every object type belongs to exactly one namespace.
- A namespace is owned by exactly one registered binder (plugin or built-in).
- A commit may only write objects in namespaces owned by its `actor`.
- SAME_AS edges cross namespace boundaries freely — they are relationship assertions,
  not namespace members.
- In V1: the single actor owns all namespaces. The ownership check is a no-op but must
  be structurally present so Phase 2 can enforce it without refactoring.

---

## V1 Non-Goals

These are explicitly out of scope for Phase 13 (V1 Binder Store):

- **Encryption at rest** — out of scope until secure multi-user mode is required.
- **Compaction** — the WAL is never compacted in V1. Growth is unbounded but acceptable
  for a single-user desktop with checkpointing.
- **Network replication** — Binder Store is local-first. No sync protocol in V1.
- **Concurrent writers** — one process, one write lock. No multi-process access.
- **Text search index** — Phase 2. Commit log stores the data; index comes later.
- **Vector/embedding index** — Phase 2 (dpk-semantic).
- **Timeline projections** — Phase 2.
- **Permission enforcement** — replaced by namespace ownership; full RBAC is Phase 2.
- **Vacuum / reclaim** — no storage reclaim in V1.
- **Schema migrations** — V1 stores raw bytes. Schema evolution is a Phase 2 concern.
