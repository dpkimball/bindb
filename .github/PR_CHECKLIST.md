# Pull Request Checklist

**For AI assistants and humans:** complete this **before** `gh pr create` (or before
asking for review on an already-open PR). Paste a checked copy into
`docs/issues/<N>/validation.md` when using the Delivery Agent.

Do not skip local lint/test/build. CI is confirmation, not the first gate.

## Pre-PR requirements

### Git workflow
- [ ] On a feature branch (never `master`)
- [ ] Fetched latest `origin/master`; branch is rebased or mergeable
- [ ] No parallel feature branch for the same issue / overlapping paths
- [ ] Issue exists; PR body will include `Implements #N` (or `Closes #N` only when AC is fully met on merge)
- [ ] Do **not** merge unless the human explicitly asks in the same message

### Code quality
- [ ] `cargo fmt --all -- --check` passes (or repo `make lint` / `dpk-build` lint step)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] No secrets, API keys, or credentials in the diff

### Testing
- [ ] `cargo test --workspace` passes (or `make test` / deliver test step)
- [ ] New behavior has regression coverage (unit and/or integration under `crates/*/tests`)
- [ ] Optional integrations: confirm no-op when env unset (feature flags or env vars absent do not fail commits)

### Build (`make b`)
- [ ] `make b` succeeds from the repo root
  - Expected: `dpk-build deliver` → lint → test → build
  - Image/deploy may skip per `dpk.toml` (`skip.deploy`); that is OK — still report the outcome
  - If `Makefile.common` is missing: ambient `BUILD_ROOT` pollution; this Makefile forces the sibling `../dpk-build` path — do not set `BUILD_ROOT` to the project cwd
- [ ] Record exact command + exit code / summary in the PR Test plan (and `validation.md` if present)

### Post-PR
- [ ] CI green (or failures explained with next action)
- [ ] PR Test plan checks off what was actually run — not aspirational boxes
