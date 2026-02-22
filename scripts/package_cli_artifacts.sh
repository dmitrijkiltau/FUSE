#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"

RELEASE=0
SKIP_BUILD=0
PLATFORM=""

usage() {
  cat <<'USAGE'
Usage: scripts/package_cli_artifacts.sh [options]

Options:
  --platform <name>   Target platform identifier (default: host platform, e.g. linux-x64)
  --release           Build dist binaries in release mode
  --skip-build        Skip scripts/build_dist.sh
  -h, --help          Show this help
USAGE
}

host_platform_dir() {
  local os arch
  case "$(uname -s)" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT) os="windows" ;;
    *)
      echo "unsupported host OS for platform detection: $(uname -s)" >&2
      return 1
      ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch="x64" ;;
    aarch64|arm64) arch="arm64" ;;
    *)
      echo "unsupported host arch for platform detection: $(uname -m)" >&2
      return 1
      ;;
  esac
  echo "${os}-${arch}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform)
      PLATFORM="${2:-}"
      if [[ -z "$PLATFORM" ]]; then
        echo "--platform requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --release)
      RELEASE=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$PLATFORM" ]]; then
  PLATFORM="$(host_platform_dir)"
fi

if [[ "$PLATFORM" == windows-* ]]; then
  FUSE_BIN="fuse.exe"
  LSP_BIN="fuse-lsp.exe"
  FALLBACK_FUSE="fuse"
  FALLBACK_LSP="fuse-lsp"
  ARCHIVE="$DIST_DIR/fuse-cli-${PLATFORM}.zip"
else
  FUSE_BIN="fuse"
  LSP_BIN="fuse-lsp"
  FALLBACK_FUSE="fuse.exe"
  FALLBACK_LSP="fuse-lsp.exe"
  ARCHIVE="$DIST_DIR/fuse-cli-${PLATFORM}.tar.gz"
fi

BUILD_ARGS=()
if [[ "$RELEASE" -eq 1 ]]; then
  BUILD_ARGS+=(--release)
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  "$ROOT/scripts/build_dist.sh" "${BUILD_ARGS[@]}"
fi

resolve_src() {
  local preferred="$1"
  local fallback="$2"
  if [[ -x "$DIST_DIR/$preferred" ]]; then
    echo "$DIST_DIR/$preferred"
    return 0
  fi
  if [[ -x "$DIST_DIR/$fallback" ]]; then
    echo "$DIST_DIR/$fallback"
    return 0
  fi
  return 1
}

if ! SRC_FUSE="$(resolve_src "$FUSE_BIN" "$FALLBACK_FUSE")"; then
  echo "missing dist binary: expected $DIST_DIR/$FUSE_BIN" >&2
  exit 1
fi
if ! SRC_LSP="$(resolve_src "$LSP_BIN" "$FALLBACK_LSP")"; then
  echo "missing dist binary: expected $DIST_DIR/$LSP_BIN" >&2
  exit 1
fi

mkdir -p "$DIST_DIR" "$ROOT/tmp"
STAGE_DIR="$(mktemp -d "$ROOT/tmp/cli-package.XXXXXX")"
cleanup() {
  rm -rf "$STAGE_DIR"
}
trap cleanup EXIT

cp "$SRC_FUSE" "$STAGE_DIR/$FUSE_BIN"
cp "$SRC_LSP" "$STAGE_DIR/$LSP_BIN"
cp "$ROOT/LICENSE" "$STAGE_DIR/LICENSE"
chmod +x "$STAGE_DIR/$FUSE_BIN" "$STAGE_DIR/$LSP_BIN"

cat >"$STAGE_DIR/README.txt" <<README
Fuse CLI bundle (${PLATFORM})

Contents:
- ${FUSE_BIN}
- ${LSP_BIN}

Quick start:
- chmod +x ${FUSE_BIN} ${LSP_BIN}
- ./${FUSE_BIN} --help
README

rm -f "$ARCHIVE"
if [[ "$ARCHIVE" == *.zip ]]; then
  if command -v zip >/dev/null 2>&1; then
    (
      cd "$STAGE_DIR"
      zip -qr "$ARCHIVE" "$FUSE_BIN" "$LSP_BIN" LICENSE README.txt
    )
  elif command -v python3 >/dev/null 2>&1; then
    STAGE_DIR="$STAGE_DIR" ARCHIVE="$ARCHIVE" FUSE_BIN="$FUSE_BIN" LSP_BIN="$LSP_BIN" python3 - <<'PY'
import os
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

stage = Path(os.environ["STAGE_DIR"])
archive = Path(os.environ["ARCHIVE"])
fuse_bin = os.environ["FUSE_BIN"]
lsp_bin = os.environ["LSP_BIN"]
with ZipFile(archive, "w", compression=ZIP_DEFLATED) as zf:
    for rel in [fuse_bin, lsp_bin, "LICENSE", "README.txt"]:
        zf.write(stage / rel, rel)
PY
  elif command -v bsdtar >/dev/null 2>&1; then
    (
      cd "$STAGE_DIR"
      bsdtar --format zip -cf "$ARCHIVE" "$FUSE_BIN" "$LSP_BIN" LICENSE README.txt
    )
  else
    echo "missing zip archiver: install zip, python3, or bsdtar" >&2
    exit 1
  fi
else
  tar -C "$STAGE_DIR" -czf "$ARCHIVE" "$FUSE_BIN" "$LSP_BIN" LICENSE README.txt
fi

"$ROOT/scripts/verify_cli_artifact.sh" --platform "$PLATFORM" --archive "$ARCHIVE"

echo "cli artifact created: $ARCHIVE"
