#!/usr/bin/env bash
# Allowlist-package a bindb release archive. Never archive the repo tree.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY_PATH="${1:?usage: package-release.sh <binary-path> <version> <platform-suffix> <out-dir>}"
VERSION="${2:?missing version (e.g. 0.1.0)}"
PLATFORM="${3:?missing platform suffix (e.g. darwin-arm64)}"
OUT_DIR="${4:?missing out dir}"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/bindb-stage.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

mkdir -p "$STAGE" "$OUT_DIR"

BIN_NAME="$(basename "$BINARY_PATH")"
case "$BIN_NAME" in
  bindb|bindb.exe) ;;
  *)
    echo "error: binary must be named bindb or bindb.exe (got $BIN_NAME)" >&2
    exit 1
    ;;
esac

cp "$BINARY_PATH" "$STAGE/$BIN_NAME"
chmod +x "$STAGE/$BIN_NAME" 2>/dev/null || true
cp "$ROOT/packaging/README.txt" "$STAGE/README.txt"
cp "$ROOT/packaging/LICENSE.txt" "$STAGE/LICENSE.txt"

# Source protection — allowlist model + explicit fail-closed checks.
FORBIDDEN=$(find "$STAGE" \( \
  -name '*.rs' -o \
  -name 'Cargo.toml' -o \
  -name 'Cargo.lock' -o \
  -name '.git' -o \
  -path '*/src/*' -o \
  -path '*/src' -o \
  -path '*/tests/*' -o \
  -path '*/tests' \
\) -print 2>/dev/null || true)

if [[ -n "$FORBIDDEN" ]]; then
  echo "error: staging directory contains forbidden source paths:" >&2
  echo "$FORBIDDEN" >&2
  exit 1
fi

# Expected allowlist only.
while IFS= read -r -d '' entry; do
  base="$(basename "$entry")"
  case "$base" in
    bindb|bindb.exe|README.txt|LICENSE.txt) ;;
    *)
      echo "error: unexpected staging entry: $entry" >&2
      exit 1
      ;;
  esac
done < <(find "$STAGE" -mindepth 1 -maxdepth 1 -print0)

ARCHIVE_BASE="bindb-v${VERSION}-${PLATFORM}"
if [[ "$PLATFORM" == windows-* ]]; then
  ARCHIVE="${OUT_DIR}/${ARCHIVE_BASE}.zip"
  (
    cd "$STAGE"
    if command -v zip >/dev/null 2>&1; then
      zip -q "$ARCHIVE" "$BIN_NAME" README.txt LICENSE.txt
    else
      # Git Bash / Windows runners without zip: use PowerShell when available.
      powershell.exe -NoProfile -Command \
        "Compress-Archive -Path '$BIN_NAME','README.txt','LICENSE.txt' -DestinationPath '$ARCHIVE' -Force"
    fi
  )
else
  ARCHIVE="${OUT_DIR}/${ARCHIVE_BASE}.tar.gz"
  tar -C "$STAGE" -czf "$ARCHIVE" "$BIN_NAME" README.txt LICENSE.txt
fi

echo "packed $ARCHIVE"
