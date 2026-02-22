#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"

PLATFORM=""
ARCHIVE=""

usage() {
  cat <<'USAGE'
Usage: scripts/verify_cli_artifact.sh [options]

Options:
  --platform <name>  Target platform identifier (e.g. linux-x64)
  --archive <path>   Path to CLI archive (default: dist/fuse-cli-<platform>.tar.gz|.zip)
  -h, --help         Show this help
USAGE
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
    --archive)
      ARCHIVE="${2:-}"
      if [[ -z "$ARCHIVE" ]]; then
        echo "--archive requires a value" >&2
        exit 1
      fi
      shift 2
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
  echo "--platform is required" >&2
  usage
  exit 1
fi

if [[ "$PLATFORM" == windows-* ]]; then
  FUSE_BIN="fuse.exe"
  LSP_BIN="fuse-lsp.exe"
  DEFAULT_ARCHIVE="$DIST_DIR/fuse-cli-${PLATFORM}.zip"
else
  FUSE_BIN="fuse"
  LSP_BIN="fuse-lsp"
  DEFAULT_ARCHIVE="$DIST_DIR/fuse-cli-${PLATFORM}.tar.gz"
fi

if [[ -z "$ARCHIVE" ]]; then
  ARCHIVE="$DEFAULT_ARCHIVE"
fi

if [[ ! -f "$ARCHIVE" ]]; then
  echo "missing cli archive: $ARCHIVE" >&2
  exit 1
fi

mkdir -p "$ROOT/tmp"
LIST_FILE="$(mktemp "$ROOT/tmp/cli-archive-list.XXXXXX")"
cleanup() {
  rm -f "$LIST_FILE"
}
trap cleanup EXIT

if [[ "$ARCHIVE" == *.zip ]]; then
  unzip -Z1 "$ARCHIVE" >"$LIST_FILE"
else
  tar -tzf "$ARCHIVE" >"$LIST_FILE"
fi

assert_entry() {
  local needle="$1"
  if ! grep -Fxq "$needle" "$LIST_FILE"; then
    echo "CLI archive missing expected entry: $needle" >&2
    exit 1
  fi
}

assert_entry "$FUSE_BIN"
assert_entry "$LSP_BIN"
assert_entry "LICENSE"
assert_entry "README.txt"

echo "cli archive integrity checks passed"
