#!/usr/bin/env bash
# Publish this crate to Kellnr registry `dpk`. Never crates.io.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -f "$ROOT/env.sh" ]]; then
  # shellcheck source=/dev/null
  source "$ROOT/env.sh"
fi

export PUBLISH_CRATE=true
export CARGO_REGISTRY="${CARGO_REGISTRY:-dpk}"
export CARGO_REGISTRIES_DPK_INDEX="${CARGO_REGISTRIES_DPK_INDEX:-sparse+http://127.0.0.1:31127/api/v1/crates/}"

case "$CARGO_REGISTRY" in
  crates-io|crates.io)
    echo "Refusing to publish to crates.io. Use CARGO_REGISTRY=dpk (Kellnr)." >&2
    exit 1
    ;;
esac

BUILD_ROOT="${BUILD_ROOT:-$(cd "$ROOT/../dpk-build" 2>/dev/null && pwd || true)}"
if [[ -n "$BUILD_ROOT" && -x "$BUILD_ROOT/rust/build-crate.sh" ]] && grep -q 'cargo-publish-registry.sh' "$BUILD_ROOT/rust/build-crate.sh"; then
  exec "$BUILD_ROOT/rust/build-crate.sh"
fi

set +e
OUTPUT=$(cargo publish --registry dpk --allow-dirty 2>&1)
STATUS=$?
set -e
if [[ $STATUS -ne 0 ]]; then
  if echo "$OUTPUT" | grep -qiE 'already exists|previously uploaded|duplicate version'; then
    echo "bindb already on registry dpk; skipping republish"
    exit 0
  fi
  echo "$OUTPUT" >&2
  exit $STATUS
fi
echo "$OUTPUT"
