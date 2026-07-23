# bindb — Storage Internals

This document covers the WAL, on-disk layout, checkpoint protocol, and crash recovery.
For the high-level architecture see the [README](../README.md).
For implementation invariants see [PRINCIPLES.md](../PRINCIPLES.md).

---

## On-disk layout

```text
{store_path}/
├── commits.wal           ← append-only WAL — the only canonical store
├── checkpoint.log        ← 20-byte records: wal_offset + commit_count + CRC32
├── vector_projection.bin ← persisted VectorIndex snapshot (written on clean close)
└── .dirty                ← crash sentinel; present = last close was unclean
```

The store directory is also used as a process-level lock — only one process may open a
given store path at a time.

---

## WAL frame format

Every commit is serialized as a self-describing frame:

```
┌──────────────────────────────────────────────────────────────┐
│  4 bytes │ magic: b"BNDR"                                    │
│  8 bytes │ entry_id: u64 LE  (monotone counter)             │
│  8 bytes │ data_len: u64 LE                                   │
│  N bytes │ bincode-serialized WalEntryData { commit }         │
│  4 bytes │ CRC32 of the N data bytes only                     │
└──────────────────────────────────────────────────────────────┘
```

Total framing overhead: 24 bytes per commit.

CRC32 covers only the data payload, not the framing header. The magic bytes and entry_id
provide additional corruption detection at the frame boundary.

---

## Write path

### Durable commit (`commit()`)

```mermaid
sequenceDiagram
    participant Caller
    participant BinderStore
    participant WAL
    participant Indexes

    Caller->>BinderStore: commit(tx, actor, message)
    BinderStore->>BinderStore: namespace_registry.check()
    BinderStore->>BinderStore: acquire write_lock
    BinderStore->>BinderStore: build BinderCommit (new ULID)
    BinderStore->>WAL: write_frame() → serialize + CRC32
    WAL->>WAL: fsync()
    WAL-->>BinderStore: Ok
    BinderStore->>Indexes: apply_commit_to_memory()
    BinderStore-->>Caller: Ok(CommitId)
```

The in-memory state is **never updated until after `fsync()` succeeds** (I-3).
A crash between `write_frame()` and `fsync()` leaves a partial frame that WAL replay
will detect and discard via CRC mismatch.

### Buffered commit (`commit_buffered()` + `flush()`)

```mermaid
sequenceDiagram
    participant Caller
    participant BinderStore
    participant WAL

    loop N objects
        Caller->>BinderStore: commit_buffered(tx, actor, message)
        BinderStore->>WAL: write_frame() — no fsync
        BinderStore->>BinderStore: apply_commit_to_memory()
        Note over BinderStore: reads are consistent immediately
    end
    Caller->>BinderStore: flush()
    BinderStore->>WAL: fsync() — single disk barrier for all N commits
```

A crash before `flush()` loses all buffered commits. Reads within the process are
consistent because in-memory state is updated immediately.

---

## Dirty marker protocol

```mermaid
flowchart LR
    OPEN["open()"] -->|"write .dirty"| READY["ready"]
    READY -->|"write commits..."| READY
    READY -->|"close()"| CLOSE["fsync + checkpoint\ndelete .dirty"]
```

On `open()`:

- If `.dirty` is **absent**: the previous close was clean. WAL replay still runs (I-5)
  but may short-circuit via checkpoint.
- If `.dirty` is **present**: the previous process crashed. Full WAL replay is mandatory.

`.dirty` is written before any WAL append and deleted only after a successful `close()`.
A partial WAL write can never produce a `.dirty`-absent dirty state.

---

## Checkpoint log

The checkpoint log is a flat append-only file of 20-byte records:

```
┌──────────────────────────────────────────────┐
│  8 bytes │ wal_offset: u64 LE               │
│  8 bytes │ commit_count: u64 LE             │
│  4 bytes │ CRC32 of the 16 preceding bytes  │
└──────────────────────────────────────────────┘
```

The last valid record (highest offset passing CRC check) is the checkpoint.

In V1 the checkpoint is read at `open()` but WAL replay always starts from the beginning.
The checkpoint infrastructure is in place for Phase 2 when replay-from-checkpoint reduces
startup time on long-lived stores.

---

## Recovery sequence

```mermaid
flowchart TD
    OPEN["BinderStore::open(path)"]
    WRITE_DIRTY["write .dirty marker"]
    READ_CKP["read checkpoint.log\n(find last valid record)"]
    REPLAY["WAL replay\nread from byte 0"]
    FRAME["read frame:\nmagic · entry_id · data_len · data · CRC32"]
    VALID{"CRC32 ok\nand magic ok?"}
    ACCEPT["accept commit\nadvance good_offset"]
    STOP["stop replay\ntruncate WAL at good_offset"]
    REBUILD["rebuild all indexes\nfrom recovered commits"]
    READY["Store ready"]

    OPEN --> WRITE_DIRTY --> READ_CKP --> REPLAY
    REPLAY --> FRAME --> VALID
    VALID -->|yes| ACCEPT --> FRAME
    VALID -->|no| STOP --> REBUILD --> READY
    ACCEPT -->|EOF| REBUILD
```

After recovery:

1. WAL is truncated at the last good frame boundary — future appends never follow a
   corrupt entry.
2. All indexes are rebuilt from the recovered commits.
3. The store is in a consistent state: exactly the commits that were fsynced before the
   crash are present.

---

## Vector projection persistence

The `VectorIndex` is not stored in the WAL (embeddings are large and non-canonical).
On a clean `close()`, the projection is serialized to `vector_projection.bin` so it
does not need to be recomputed on next open.

On open after a crash (`.dirty` present), the stale projection file is discarded and
the caller is expected to re-index from the current objects.
