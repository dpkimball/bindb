#!/usr/bin/env bash
# Local `make b` publishes to DEV Kellnr (NodePort 31127), like binder-sdk → twine on :31126.
# GitHub Actions sets CI=true, so PR / rust-library-ci test jobs do not publish.

if [ -z "${CI:-}" ]; then
  export PUBLISH_CRATE=true
  export CARGO_REGISTRY="${CARGO_REGISTRY:-dpk}"
  export CARGO_REGISTRIES_DPK_INDEX="${CARGO_REGISTRIES_DPK_INDEX:-sparse+http://127.0.0.1:31127/api/v1/crates/}"
  # Homelab bootstrap token — same value as dpk-infra charts/kellnr values.yaml adminToken.
  export CARGO_REGISTRIES_DPK_TOKEN="${CARGO_REGISTRIES_DPK_TOKEN:-1heqlaq4Dx502qYVjknOz9z9HazPudiM}"
fi
