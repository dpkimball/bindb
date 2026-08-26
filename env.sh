#!/usr/bin/env bash
# Local `make b` publishes to DEV Kellnr, like binder-sdk → twine on :31126.
# GitHub Actions sets CI=true, so PR / rust-library-ci test jobs do not publish.
# Index: dpk-build env.sh (localhost + CARGO_PORT). CI: secrets.PRIVATE_CARGO_URL.

if [ -z "${CI:-}" ]; then
  export PUBLISH_CRATE=true
  export CARGO_REGISTRY="${CARGO_REGISTRY:-dpk}"
fi
# Token is CARGO_REGISTRIES_DPK_TOKEN or ~/.cargo/credentials.toml — never commit it.
