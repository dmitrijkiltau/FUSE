#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/tmp/fuse-target}"
export RUSTC_TMPDIR="${RUSTC_TMPDIR:-$ROOT/tmp/fuse-tmp}"
export TMPDIR="$RUSTC_TMPDIR"
export TMP="$RUSTC_TMPDIR"
export TEMP="$RUSTC_TMPDIR"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

mkdir -p "$CARGO_TARGET_DIR" "$RUSTC_TMPDIR"

if [[ $# -eq 0 ]]; then
  echo "usage: $(basename "$0") <command...>"
  echo "example: $(basename "$0") cargo check -p fusec"
  echo "repo: $ROOT"
  exit 1
fi

exec "$@"
