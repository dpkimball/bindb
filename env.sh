#!/usr/bin/env bash
# Local `make b` publishes to DEV Kellnr (NodePort 31127), like binder-sdk → twine on :31126.
# GitHub Actions sets CI=true, so PR / rust-library-ci test jobs do not publish.

if [ -z "${CI:-}" ]; then
  export PUBLISH_CRATE=true
  export CARGO_REGISTRY="${CARGO_REGISTRY:-dpk}"
fi
# Index: dpk-build env.sh (localhost + CARGO_PORT). CI: secrets.PRIVATE_CARGO_URL.
# Token is CARGO_REGISTRIES_DPK_TOKEN or ~/.cargo/credentials.toml — never commit it.
