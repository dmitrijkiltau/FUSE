#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/tmp/fuse-target}"
case "$TARGET_DIR" in
  /*) ;;
  *) TARGET_DIR="$ROOT/$TARGET_DIR" ;;
esac
export CARGO_TARGET_DIR="$TARGET_DIR"

# Keep rustc temp files on the same filesystem as the target dir to avoid EXDEV.
TMP_DIR="${RUSTC_TMPDIR:-$CARGO_TARGET_DIR/tmp}"
case "$TMP_DIR" in
  /*) ;;
  *) TMP_DIR="$ROOT/$TMP_DIR" ;;
esac
export RUSTC_TMPDIR="$TMP_DIR"

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
