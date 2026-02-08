#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="debug"
BUILD_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      PROFILE="release"
      BUILD_ARGS+=("--release")
      shift
      ;;
    *)
      echo "unknown option: $1"
      echo "usage: scripts/build_dist.sh [--release]"
      exit 1
      ;;
  esac
done

"$ROOT/scripts/cargo_env.sh" cargo build -p fuse "${BUILD_ARGS[@]}"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --bin fuse-lsp "${BUILD_ARGS[@]}"

BIN_DIR="$ROOT/tmp/fuse-target/$PROFILE"
DIST_DIR="$ROOT/dist"
mkdir -p "$DIST_DIR"

if [[ ! -x "$BIN_DIR/fuse" ]]; then
  echo "missing binary: $BIN_DIR/fuse"
  exit 1
fi
if [[ ! -x "$BIN_DIR/fuse-lsp" ]]; then
  echo "missing binary: $BIN_DIR/fuse-lsp"
  exit 1
fi

cp "$BIN_DIR/fuse" "$DIST_DIR/fuse"
cp "$BIN_DIR/fuse-lsp" "$DIST_DIR/fuse-lsp"
chmod +x "$DIST_DIR/fuse" "$DIST_DIR/fuse-lsp"

echo "dist ready: $DIST_DIR"
