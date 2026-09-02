---
id: private-source-public-binary
title: bindb Phase 1 — private source / public binary distribution
status: draft
created: 2026-09-02
updated: 2026-09-02
owner: bindb
github_issue: https://github.com/dpkimball/bindb/issues/97
related:
  - https://github.com/dpkimball/bindb/issues/97
---

# bindb Phase 1 — private source / public binary distribution

## Purpose

Prove that `bindb` can be consumed as a **public compiled binary** without exposing the private source repository.

## Current behavior

- `dpkimball/bindb` is a private Rust library published to Kellnr (`dpk`).
- No public `bindb` executable, no `*-releases` repo, no tag→multi-OS GitHub Release pipeline.

## Desired behavior

### Public executable contract

```text
bindb --version
bindb --help
```

`--version` prints (stdout):

```text
bindb <semver>
commit: <7-char sha or unknown>
built: <YYYY-MM-DD or unknown>
```

No private repository URLs or secrets. Errors go to stderr. Non-zero exit on failure. No telemetry. No embedded credentials.

Phase 1 does **not** expand operational store subcommands; Kellnr remains the Rust-embed path. The binary exists so install/verify/integration can be proven.

### Release automation

On push of tag `v*`:

1. Validate tag semver == `Cargo.toml` `version`
2. `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked`
3. Build release binaries for the five-platform matrix
4. Allowlist-package each archive + generate `SHA256SUMS`
5. Source-guard staging directory (fail on forbidden paths)
6. Publish GitHub Release on **`dpkimball/bindb-releases`** only when all platforms succeed

### Public distribution repo

`dpkimball/bindb-releases` contains only documentation, licensing, and release artifacts — never Cargo/Rust sources.

## Acceptance criteria

Match [bindb#97](https://github.com/dpkimball/bindb/issues/97).

## Operator setup (one-time)

1. Public repo: [`dpkimball/bindb-releases`](https://github.com/dpkimball/bindb-releases) (README + LICENSE only).
2. Create a fine-grained PAT (or GitHub App installation token) with **Contents: Read and write** on `dpkimball/bindb-releases` only.
3. Add it as Actions secret `RELEASE_DISTRIBUTION_TOKEN` on **private** `dpkimball/bindb`.
4. After merge: `git tag v0.1.0 && git push origin v0.1.0` (Cargo.toml must already be `0.1.0`).

## Out of scope

Phases 2–5; code signing; Homebrew/curl installer productization; changing Kellnr crate license; open-sourcing.
