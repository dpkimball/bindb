---
id: private-source-public-binary
title: bindb Phase 1 — private source / public binary distribution
status: superseded
created: 2026-09-02
updated: 2026-09-10
owner: bindb
github_issue: https://github.com/dpkimball/bindb/issues/97
related:
  - https://github.com/dpkimball/bindb/issues/97
  - https://github.com/dpkimball/bindb/issues/99
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

Phase 1 proved install/verify only. Phase 2 adds `info` / `list` (see `phase2-cli-info-list.md` / #101). Kellnr remains the Rust-embed path for writes and rich query.

### Release automation

On push of tag `v*`, [dist](https://github.com/axodotdev/cargo-dist) runs the generated `.github/workflows/release.yml`:

1. `dist plan` — resolve tag, validate config
2. Build release binaries for the two-platform matrix (darwin-arm64, linux-x64)
3. Package each archive with SHA256 checksums via dist's built-in tooling
4. Publish GitHub Release on **`dpkimball/bindb-releases`** only when all platforms succeed

Platforms: `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`. Expand the `targets` list in `dist-workspace.toml` when additional consumers actually need them.

### Public distribution repo

`dpkimball/bindb-releases` contains only documentation, licensing, and release artifacts — never Cargo/Rust sources.

Public install docs must match cargo-dist naming: `bindb-<rust-target-triple>.tar.xz`, per-archive `*.tar.xz.sha256`, and `sha256.sum` (not legacy `darwin-arm64.tar.gz` / `SHA256SUMS`).

### cargo-dist announcement URL drift

`github-releases-repo = "dpkimball/bindb-releases"` uploads assets to the public repo, but cargo-dist still embeds **private** `dpkimball/bindb` download URLs in the release body and `dist-manifest.json` hosting block (upstream: [axodotdev/cargo-dist#2252](https://github.com/axodotdev/cargo-dist/issues/2252)).

Mitigation: `.github/workflows/fix-bindb-releases-announcement.yml` runs after a successful `Release` workflow and rewrites those URLs on `bindb-releases` (requires `GH_RELEASES_TOKEN`).

## Implementation

- **`dist-workspace.toml`** — dist configuration: two targets, `github-releases-repo`, `include` for proprietary license/README.
- **`build.rs`** — injects `BINDB_GIT_SHA` (7-char) and `BINDB_BUILD_DATE` (YYYY-MM-DD) at compile time from CI env or local git/date fallbacks.
- **`src/bin/bindb.rs`** — `--version` / `--help` CLI surface. No operational subcommands in Phase 1.
- **`.github/workflows/release.yml`** — dist-generated; do not hand-edit. Regenerate with `dist generate`.
- **`.github/workflows/fix-bindb-releases-announcement.yml`** — post-publish URL rewrite for public consumers.

## Acceptance criteria

Match [bindb#97](https://github.com/dpkimball/bindb/issues/97).

## Operator setup (one-time)

1. Public repo: [`dpkimball/bindb-releases`](https://github.com/dpkimball/bindb-releases) (README + LICENSE only). Requires at least one commit so a release target exists.
2. Create a fine-grained PAT with **Contents: Read and write** on `dpkimball/bindb-releases` only.
3. Add it as Actions secret **`GH_RELEASES_TOKEN`** on **private** `dpkimball/bindb` (this is the name dist expects for cross-repo releases).
4. After merge: `git tag v0.1.0 && git push origin v0.1.0` (Cargo.toml must already be `0.1.0`).

## Proprietary binary distribution

Users of the public binary are **not** receiving open-source software. The README and release notes must make this unambiguous:

> bindb is proprietary software distributed as a compiled executable.
> Source code is not currently publicly available.

`packaging/LICENSE.txt` is included in every release archive (via dist `include`). See that file for the full binary distribution license.

## Out of scope

Phases 2–5; code signing; Homebrew/curl installer productization; changing Kellnr crate license; open-sourcing.
