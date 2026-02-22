#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fuse_packaging_verify.XXXXXX")"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

create_zip_with_entries() {
  local stage_dir="$1"
  local archive="$2"
  shift 2
  local entries=("$@")

  if command -v python3 >/dev/null 2>&1; then
    STAGE_DIR="$stage_dir" ARCHIVE="$archive" ENTRIES="$(
      printf "%s\n" "${entries[@]}"
    )" python3 - <<'PY'
import os
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

stage = Path(os.environ["STAGE_DIR"])
archive = Path(os.environ["ARCHIVE"])
entries = [line for line in os.environ["ENTRIES"].splitlines() if line]
with ZipFile(archive, "w", compression=ZIP_DEFLATED) as zf:
    for rel in entries:
        zf.write(stage / rel, rel)
PY
  elif command -v zip >/dev/null 2>&1; then
    (
      cd "$stage_dir"
      zip -q "$archive" "${entries[@]}"
    )
  elif command -v bsdtar >/dev/null 2>&1; then
    (
      cd "$stage_dir"
      bsdtar --format zip -cf "$archive" "${entries[@]}"
    )
  else
    echo "missing zip archiver: install python3, zip, or bsdtar" >&2
    exit 1
  fi
}

assert_fails_with() {
  local needle="$1"
  shift
  local log_file
  log_file="$(mktemp "$TMP_DIR/fail-log.XXXXXX")"
  set +e
  "$@" >"$log_file" 2>&1
  local status=$?
  set -e
  if [[ "$status" -eq 0 ]]; then
    echo "expected failure but command succeeded: $*" >&2
    cat "$log_file" >&2
    rm -f "$log_file"
    exit 1
  fi
  if ! grep -Fq "$needle" "$log_file"; then
    echo "expected failure output to include: $needle" >&2
    cat "$log_file" >&2
    rm -f "$log_file"
    exit 1
  fi
  rm -f "$log_file"
}

echo "[1/6] Validate Windows CLI archive verifier accepts .exe layout..."
CLI_GOOD_STAGE="$TMP_DIR/cli-good"
mkdir -p "$CLI_GOOD_STAGE"
printf 'bin' >"$CLI_GOOD_STAGE/fuse.exe"
printf 'bin' >"$CLI_GOOD_STAGE/fuse-lsp.exe"
printf 'license' >"$CLI_GOOD_STAGE/LICENSE"
printf 'readme' >"$CLI_GOOD_STAGE/README.txt"
CLI_GOOD_ZIP="$TMP_DIR/fuse-cli-windows-x64.zip"
create_zip_with_entries "$CLI_GOOD_STAGE" "$CLI_GOOD_ZIP" \
  "fuse.exe" "fuse-lsp.exe" "LICENSE" "README.txt"
"$ROOT/scripts/verify_cli_artifact.sh" --platform windows-x64 --archive "$CLI_GOOD_ZIP"

echo "[2/6] Validate Windows CLI archive verifier rejects missing .exe entries..."
CLI_BAD_STAGE="$TMP_DIR/cli-bad"
mkdir -p "$CLI_BAD_STAGE"
printf 'bin' >"$CLI_BAD_STAGE/fuse"
printf 'bin' >"$CLI_BAD_STAGE/fuse-lsp"
printf 'license' >"$CLI_BAD_STAGE/LICENSE"
printf 'readme' >"$CLI_BAD_STAGE/README.txt"
CLI_BAD_ZIP="$TMP_DIR/fuse-cli-windows-x64-bad.zip"
create_zip_with_entries "$CLI_BAD_STAGE" "$CLI_BAD_ZIP" \
  "fuse" "fuse-lsp" "LICENSE" "README.txt"
assert_fails_with \
  "CLI archive missing expected entry: fuse.exe" \
  "$ROOT/scripts/verify_cli_artifact.sh" --platform windows-x64 --archive "$CLI_BAD_ZIP"

echo "[3/6] Validate Windows VSIX verifier accepts bundled fuse-lsp.exe..."
VSIX_GOOD_STAGE="$TMP_DIR/vsix-good"
mkdir -p "$VSIX_GOOD_STAGE/extension/bin/windows-x64"
mkdir -p "$VSIX_GOOD_STAGE/extension/syntaxes"
printf '<types />' >"$VSIX_GOOD_STAGE/[Content_Types].xml"
printf '<manifest />' >"$VSIX_GOOD_STAGE/extension.vsixmanifest"
printf '{}' >"$VSIX_GOOD_STAGE/extension/package.json"
printf 'module.exports = {};\n' >"$VSIX_GOOD_STAGE/extension/extension.js"
printf 'module.exports = {};\n' >"$VSIX_GOOD_STAGE/extension/lsp-path.js"
printf '{}' >"$VSIX_GOOD_STAGE/extension/syntaxes/fuse.tmLanguage.json"
printf 'bin' >"$VSIX_GOOD_STAGE/extension/bin/windows-x64/fuse-lsp.exe"
VSIX_GOOD="$TMP_DIR/fuse-vscode-windows-x64.vsix"
create_zip_with_entries "$VSIX_GOOD_STAGE" "$VSIX_GOOD" \
  "[Content_Types].xml" \
  "extension.vsixmanifest" \
  "extension/package.json" \
  "extension/extension.js" \
  "extension/lsp-path.js" \
  "extension/syntaxes/fuse.tmLanguage.json" \
  "extension/bin/windows-x64/fuse-lsp.exe"
"$ROOT/scripts/verify_vscode_vsix.sh" --platform windows-x64 --vsix "$VSIX_GOOD"

echo "[4/6] Validate Windows VSIX verifier rejects missing fuse-lsp.exe..."
VSIX_BAD_STAGE="$TMP_DIR/vsix-bad"
mkdir -p "$VSIX_BAD_STAGE/extension/bin/windows-x64"
mkdir -p "$VSIX_BAD_STAGE/extension/syntaxes"
printf '<types />' >"$VSIX_BAD_STAGE/[Content_Types].xml"
printf '<manifest />' >"$VSIX_BAD_STAGE/extension.vsixmanifest"
printf '{}' >"$VSIX_BAD_STAGE/extension/package.json"
printf 'module.exports = {};\n' >"$VSIX_BAD_STAGE/extension/extension.js"
printf 'module.exports = {};\n' >"$VSIX_BAD_STAGE/extension/lsp-path.js"
printf '{}' >"$VSIX_BAD_STAGE/extension/syntaxes/fuse.tmLanguage.json"
printf 'bin' >"$VSIX_BAD_STAGE/extension/bin/windows-x64/fuse-lsp"
VSIX_BAD="$TMP_DIR/fuse-vscode-windows-x64-bad.vsix"
create_zip_with_entries "$VSIX_BAD_STAGE" "$VSIX_BAD" \
  "[Content_Types].xml" \
  "extension.vsixmanifest" \
  "extension/package.json" \
  "extension/extension.js" \
  "extension/lsp-path.js" \
  "extension/syntaxes/fuse.tmLanguage.json" \
  "extension/bin/windows-x64/fuse-lsp"
assert_fails_with \
  "VSIX missing expected entry: extension/bin/windows-x64/fuse-lsp.exe" \
  "$ROOT/scripts/verify_vscode_vsix.sh" --platform windows-x64 --vsix "$VSIX_BAD"

echo "[5/6] Validate Windows AOT archive verifier accepts expected payload..."
AOT_GOOD_STAGE="$TMP_DIR/aot-good"
mkdir -p "$AOT_GOOD_STAGE"
printf 'bin' >"$AOT_GOOD_STAGE/fuse-aot-demo.exe"
printf 'target=x rustc=y cli=z runtime_cache=1 contract=aot-v1\n' >"$AOT_GOOD_STAGE/AOT_BUILD_INFO.txt"
printf 'license' >"$AOT_GOOD_STAGE/LICENSE"
printf 'readme' >"$AOT_GOOD_STAGE/README.txt"
AOT_GOOD_ZIP="$TMP_DIR/fuse-aot-windows-x64.zip"
create_zip_with_entries "$AOT_GOOD_STAGE" "$AOT_GOOD_ZIP" \
  "fuse-aot-demo.exe" "AOT_BUILD_INFO.txt" "LICENSE" "README.txt"
"$ROOT/scripts/verify_aot_artifact.sh" --platform windows-x64 --archive "$AOT_GOOD_ZIP"

echo "[6/6] Validate Windows AOT archive verifier rejects missing build info fields..."
AOT_BAD_STAGE="$TMP_DIR/aot-bad"
mkdir -p "$AOT_BAD_STAGE"
printf 'bin' >"$AOT_BAD_STAGE/fuse-aot-demo.exe"
printf 'target=x rustc=y cli=z runtime_cache=1\n' >"$AOT_BAD_STAGE/AOT_BUILD_INFO.txt"
printf 'license' >"$AOT_BAD_STAGE/LICENSE"
printf 'readme' >"$AOT_BAD_STAGE/README.txt"
AOT_BAD_ZIP="$TMP_DIR/fuse-aot-windows-x64-bad.zip"
create_zip_with_entries "$AOT_BAD_STAGE" "$AOT_BAD_ZIP" \
  "fuse-aot-demo.exe" "AOT_BUILD_INFO.txt" "LICENSE" "README.txt"
assert_fails_with \
  "AOT build info missing expected field: contract=" \
  "$ROOT/scripts/verify_aot_artifact.sh" --platform windows-x64 --archive "$AOT_BAD_ZIP"

echo "packaging verifier regression checks passed"
