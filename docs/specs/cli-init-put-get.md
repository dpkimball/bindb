---
id: cli-init-put-get
title: bindb CLI — init, put, get
status: draft
created: 2026-09-02
updated: 2026-09-02
owner: bindb
github_issue: https://github.com/dpkimball/bindb/issues/103
related:
  - https://github.com/dpkimball/bindb/issues/103
  - https://github.com/dpkimball/bindb/issues/101
---

# bindb CLI — init, put, get

## Purpose

Let a clean machine create and exercise a store with the public binary (no Rust embed, no SQL).

## Commands

```text
bindb init <store-dir>
bindb put  <store-dir> <object-id> <payload>
bindb get  <store-dir> <object-id>
```

Plus existing `info` / `list`.

## Model note

No tables. Store = directory with `commits.wal`. Objects are keyed by string IDs (convention: `scheme://…`).
