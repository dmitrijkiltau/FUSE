#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fuse_release_integrity.XXXXXX")"
DIST_DIR="$TMP_DIR/dist"

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

mkdir -p "$DIST_DIR"

echo "[1/6] Create fixture release payloads..."
CLI_STAGE="$TMP_DIR/cli"
mkdir -p "$CLI_STAGE"
printf 'bin' >"$CLI_STAGE/fuse"
printf 'bin' >"$CLI_STAGE/fuse-lsp"
printf 'license' >"$CLI_STAGE/LICENSE"
printf 'readme' >"$CLI_STAGE/README.txt"
tar -C "$CLI_STAGE" -czf "$DIST_DIR/fuse-cli-linux-x64.tar.gz" fuse fuse-lsp LICENSE README.txt

AOT_STAGE="$TMP_DIR/aot"
mkdir -p "$AOT_STAGE"
printf 'bin' >"$AOT_STAGE/fuse-aot-demo"
printf 'mode=aot profile=release target=x rustc=y cli=z runtime_cache=1 contract=aot-v1\n' >"$AOT_STAGE/AOT_BUILD_INFO.txt"
printf 'license' >"$AOT_STAGE/LICENSE"
printf 'readme' >"$AOT_STAGE/README.txt"
tar -C "$AOT_STAGE" -czf "$DIST_DIR/fuse-aot-linux-x64.tar.gz" fuse-aot-demo AOT_BUILD_INFO.txt LICENSE README.txt

VSIX_STAGE="$TMP_DIR/vsix"
mkdir -p "$VSIX_STAGE/extension/bin/linux-x64"
mkdir -p "$VSIX_STAGE/extension/syntaxes"
printf '<types />' >"$VSIX_STAGE/[Content_Types].xml"
printf '<manifest />' >"$VSIX_STAGE/extension.vsixmanifest"
printf '{}' >"$VSIX_STAGE/extension/package.json"
printf 'module.exports = {};\n' >"$VSIX_STAGE/extension/extension.js"
printf 'module.exports = {};\n' >"$VSIX_STAGE/extension/lsp-path.js"
printf '{}' >"$VSIX_STAGE/extension/syntaxes/fuse.tmLanguage.json"
printf 'bin' >"$VSIX_STAGE/extension/bin/linux-x64/fuse-lsp"
create_zip_with_entries "$VSIX_STAGE" "$DIST_DIR/fuse-vscode-linux-x64.vsix" \
  "[Content_Types].xml" \
  "extension.vsixmanifest" \
  "extension/package.json" \
  "extension/extension.js" \
  "extension/lsp-path.js" \
  "extension/syntaxes/fuse.tmLanguage.json" \
  "extension/bin/linux-x64/fuse-lsp"

echo "[2/6] Generate checksums, SBOMs, and provenance..."
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_checksums.sh" --dist "$DIST_DIR"
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_sboms.sh" --dist "$DIST_DIR" --output-dir "$DIST_DIR" --base-url "https://example.test/fuse"
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_provenance.sh" \
  --dist "$DIST_DIR" \
  --output "$DIST_DIR/release-provenance.json" \
  --repository "example/fuse" \
  --ref "refs/tags/v0.9.4" \
  --tag "v0.9.4" \
  --sha "deadbeef" \
  --workflow-name "Release Artifacts" \
  --workflow-path ".github/workflows/release-artifacts.yml" \
  --workflow-ref "refs/tags/v0.9.4" \
  --event "push" \
  --actor "ci" \
  --run-id "1" \
  --run-attempt "1"
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_checksums.sh" --dist "$DIST_DIR"

echo "[3/6] Validate the integrity bundle..."
"$ROOT/scripts/verify_release_integrity.sh" \
  --dist "$DIST_DIR" \
  --workflow-name "Release Artifacts" \
  --workflow-path ".github/workflows/release-artifacts.yml" \
  --expected-ref "refs/tags/v0.9.4" \
  --expected-tag "v0.9.4" \
  --expected-sha "deadbeef" \
  --require-provenance

echo "[4/6] Validate metadata includes integrity sidecars..."
node -e '
const fs = require("fs");
const path = process.argv[1];
const data = JSON.parse(fs.readFileSync(path, "utf8"));
if (!Array.isArray(data.integrityArtifacts) || data.integrityArtifacts.length < 5) {
  throw new Error("expected integrityArtifacts entries");
}
' "$DIST_DIR/release-artifacts.json"

echo "[5/6] Confirm tampered provenance fails verification..."
node -e '
const fs = require("fs");
const path = process.argv[1];
const data = JSON.parse(fs.readFileSync(path, "utf8"));
data.commit = "tampered";
fs.writeFileSync(path, JSON.stringify(data, null, 2) + "\n");
' "$DIST_DIR/release-provenance.json"
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_checksums.sh" --dist "$DIST_DIR"
assert_fails_with \
  "provenance commit mismatch" \
  "$ROOT/scripts/verify_release_integrity.sh" \
  --dist "$DIST_DIR" \
  --workflow-name "Release Artifacts" \
  --workflow-path ".github/workflows/release-artifacts.yml" \
  --expected-ref "refs/tags/v0.9.4" \
  --expected-tag "v0.9.4" \
  --expected-sha "deadbeef" \
  --require-provenance

echo "[6/6] Restore provenance and ensure verification passes again..."
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_provenance.sh" \
  --dist "$DIST_DIR" \
  --output "$DIST_DIR/release-provenance.json" \
  --repository "example/fuse" \
  --ref "refs/tags/v0.9.4" \
  --tag "v0.9.4" \
  --sha "deadbeef" \
  --workflow-name "Release Artifacts" \
  --workflow-path ".github/workflows/release-artifacts.yml" \
  --workflow-ref "refs/tags/v0.9.4" \
  --event "push" \
  --actor "ci" \
  --run-id "1" \
  --run-attempt "1"
SOURCE_DATE_EPOCH=1700000000 "$ROOT/scripts/generate_release_checksums.sh" --dist "$DIST_DIR"
"$ROOT/scripts/verify_release_integrity.sh" \
  --dist "$DIST_DIR" \
  --workflow-name "Release Artifacts" \
  --workflow-path ".github/workflows/release-artifacts.yml" \
  --expected-ref "refs/tags/v0.9.4" \
  --expected-tag "v0.9.4" \
  --expected-sha "deadbeef" \
  --require-provenance

echo "release integrity regression checks passed"
