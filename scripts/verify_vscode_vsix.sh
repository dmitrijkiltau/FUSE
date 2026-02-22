#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"

PLATFORM=""
VSIX=""

usage() {
  cat <<'EOF'
Usage: scripts/verify_vscode_vsix.sh [options]

Options:
  --platform <name>  Target platform directory (e.g. linux-x64)
  --vsix <path>      Path to .vsix package (default: dist/fuse-vscode-<platform>.vsix)
  -h, --help         Show this help
EOF
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
    --vsix)
      VSIX="${2:-}"
      if [[ -z "$VSIX" ]]; then
        echo "--vsix requires a value" >&2
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

if [[ -z "$VSIX" ]]; then
  VSIX="$DIST_DIR/fuse-vscode-${PLATFORM}.vsix"
fi

if [[ "$PLATFORM" == windows-* ]]; then
  BIN_NAME="fuse-lsp.exe"
else
  BIN_NAME="fuse-lsp"
fi

if [[ ! -f "$VSIX" ]]; then
  echo "missing vsix package: $VSIX" >&2
  exit 1
fi

mkdir -p "$ROOT/tmp"
LIST_FILE="$(mktemp "$ROOT/tmp/vsix-list.XXXXXX")"
cleanup() {
  rm -f "$LIST_FILE"
}
trap cleanup EXIT

unzip -l "$VSIX" >"$LIST_FILE"

assert_contains() {
  local needle="$1"
  if ! grep -Fq "$needle" "$LIST_FILE"; then
    echo "VSIX missing expected entry: $needle" >&2
    exit 1
  fi
}

assert_contains "[Content_Types].xml"
assert_contains "extension.vsixmanifest"
assert_contains "extension/package.json"
assert_contains "extension/extension.js"
assert_contains "extension/lsp-path.js"
assert_contains "extension/syntaxes/"
assert_contains "extension/bin/$PLATFORM/$BIN_NAME"

echo "vsix integrity checks passed"
