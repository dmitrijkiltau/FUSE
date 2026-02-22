#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"

PLATFORM=""
ARCHIVE=""

usage() {
  cat <<'USAGE'
Usage: scripts/verify_aot_artifact.sh [options]

Options:
  --platform <name>  Target platform identifier (e.g. linux-x64)
  --archive <path>   Path to AOT archive (default: dist/fuse-aot-<platform>.tar.gz|.zip)
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
  AOT_BIN="fuse-aot-demo.exe"
  DEFAULT_ARCHIVE="$DIST_DIR/fuse-aot-${PLATFORM}.zip"
else
  AOT_BIN="fuse-aot-demo"
  DEFAULT_ARCHIVE="$DIST_DIR/fuse-aot-${PLATFORM}.tar.gz"
fi

if [[ -z "$ARCHIVE" ]]; then
  ARCHIVE="$DEFAULT_ARCHIVE"
fi

if [[ ! -f "$ARCHIVE" ]]; then
  echo "missing aot archive: $ARCHIVE" >&2
  exit 1
fi

mkdir -p "$ROOT/tmp"
LIST_FILE="$(mktemp "$ROOT/tmp/aot-archive-list.XXXXXX")"
INFO_FILE="$(mktemp "$ROOT/tmp/aot-build-info.XXXXXX")"
cleanup() {
  rm -f "$LIST_FILE" "$INFO_FILE"
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
    echo "AOT archive missing expected entry: $needle" >&2
    exit 1
  fi
}

assert_entry "$AOT_BIN"
assert_entry "AOT_BUILD_INFO.txt"
assert_entry "LICENSE"
assert_entry "README.txt"

if [[ "$ARCHIVE" == *.zip ]]; then
  unzip -p "$ARCHIVE" "AOT_BUILD_INFO.txt" >"$INFO_FILE"
else
  tar -xOf "$ARCHIVE" "AOT_BUILD_INFO.txt" >"$INFO_FILE"
fi

assert_info_field() {
  local field="$1"
  if ! grep -Eq "(^|[[:space:]])${field}=" "$INFO_FILE"; then
    echo "AOT build info missing expected field: ${field}=" >&2
    exit 1
  fi
}

assert_info_field "target"
assert_info_field "rustc"
assert_info_field "cli"
assert_info_field "mode"
assert_info_field "profile"
assert_info_field "runtime_cache"
assert_info_field "contract"

echo "aot archive integrity checks passed"
