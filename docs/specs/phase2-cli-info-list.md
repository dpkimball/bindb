---
id: phase2-cli-info-list
title: bindb Phase 2 CLI — info and list
status: draft
created: 2026-09-02
updated: 2026-09-02
owner: bindb
github_issue: https://github.com/dpkimball/bindb/issues/101
related:
  - https://github.com/dpkimball/bindb/issues/101
  - https://github.com/dpkimball/bindb/issues/97
---

# bindb Phase 2 CLI — info and list

## Purpose

After Phase 1 install/verify, make the public binary able to **inspect an on-disk store** so a pilot user can confirm their data without embedding the Rust crate.

## Desired behavior

```text
bindb info <store-dir>
bindb list <store-dir> <namespace-prefix>
bindb --version
bindb --help
```

### `info <store-dir>`

Open `store-dir` (must be a bindb store; creates empty store dirs today via `BinderStore::open` — prefer documenting that missing path that is not a store still opens/creates; if path cannot be created, error).

Print (stdout), one field per line:

```text
store: <absolute-or-given path>
wal_bytes: <u64>
commits: <usize>
head: <commit_id>|none
live_objects: <usize>
```

`live_objects` counts live object IDs (all namespaces). Does not dump payloads.

### `list <store-dir> <namespace-prefix>`

Print live object IDs matching the prefix (same semantics as `BinderStore::list` / namespace index), one ID per line, sorted. Empty prefix lists all live IDs. No payloads.

### Errors

- Wrong arity / unknown command → stderr + exit 2 (help hint)
- Open/IO failures → stderr + exit 1

## Out of scope

`get`, writes, Graph IR, vector search, Phase 3 product CLI polish.

## Acceptance

Match [bindb#101](https://github.com/dpkimball/bindb/issues/101).
