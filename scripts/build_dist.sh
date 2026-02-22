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

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/tmp/fuse-target}"
case "$TARGET_DIR" in
  /*) ;;
  *) TARGET_DIR="$ROOT/$TARGET_DIR" ;;
esac

BIN_DIR="$TARGET_DIR/$PROFILE"
DIST_DIR="$ROOT/dist"
mkdir -p "$DIST_DIR"

EXE_SUFFIX=""
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    EXE_SUFFIX=".exe"
    ;;
esac
FUSE_BIN="fuse${EXE_SUFFIX}"
FUSE_LSP_BIN="fuse-lsp${EXE_SUFFIX}"

install_binary() {
  local src="$1"
  local dest="$2"
  local tmp
  tmp="$(mktemp "$DIST_DIR/.tmp.$(basename "$dest").XXXXXX")"
  cp "$src" "$tmp"
  chmod +x "$tmp"
  mv -f "$tmp" "$dest"
}

if [[ ! -x "$BIN_DIR/$FUSE_BIN" ]]; then
  echo "missing binary: $BIN_DIR/$FUSE_BIN"
  exit 1
fi
if [[ ! -x "$BIN_DIR/$FUSE_LSP_BIN" ]]; then
  echo "missing binary: $BIN_DIR/$FUSE_LSP_BIN"
  exit 1
fi

install_binary "$BIN_DIR/$FUSE_BIN" "$DIST_DIR/$FUSE_BIN"
install_binary "$BIN_DIR/$FUSE_LSP_BIN" "$DIST_DIR/$FUSE_LSP_BIN"

echo "dist ready: $DIST_DIR"
