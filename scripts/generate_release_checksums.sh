#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"
OUTPUT=""
METADATA=""

usage() {
  cat <<'USAGE'
Usage: scripts/generate_release_checksums.sh [options]

Options:
  --dist <path>      Artifact directory (default: dist)
  --output <path>    SHA256 output path (default: <dist>/SHA256SUMS)
  --metadata <path>  JSON metadata path (default: <dist>/release-artifacts.json)
  -h, --help         Show this help
USAGE
}

abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dist)
      if [[ -z "${2:-}" ]]; then
        echo "--dist requires a value" >&2
        exit 1
      fi
      DIST_DIR="$(abspath "$2")"
      shift 2
      ;;
    --output)
      if [[ -z "${2:-}" ]]; then
        echo "--output requires a value" >&2
        exit 1
      fi
      OUTPUT="$(abspath "$2")"
      shift 2
      ;;
    --metadata)
      if [[ -z "${2:-}" ]]; then
        echo "--metadata requires a value" >&2
        exit 1
      fi
      METADATA="$(abspath "$2")"
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

if [[ -z "$OUTPUT" ]]; then
  OUTPUT="$DIST_DIR/SHA256SUMS"
fi
if [[ -z "$METADATA" ]]; then
  METADATA="$DIST_DIR/release-artifacts.json"
fi

if [[ ! -d "$DIST_DIR" ]]; then
  echo "dist directory not found: $DIST_DIR" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUTPUT")" "$(dirname "$METADATA")" "$ROOT/tmp"

HASH_TOOL=""
if command -v sha256sum >/dev/null 2>&1; then
  HASH_TOOL="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  HASH_TOOL="shasum"
elif command -v openssl >/dev/null 2>&1; then
  HASH_TOOL="openssl"
else
  echo "missing SHA-256 tool: install sha256sum, shasum, or openssl" >&2
  exit 1
fi

sha256_for_file() {
  local file="$1"
  case "$HASH_TOOL" in
    sha256sum)
      sha256sum "$file" | awk '{print $1}'
      ;;
    shasum)
      shasum -a 256 "$file" | awk '{print $1}'
      ;;
    openssl)
      openssl dgst -sha256 -r "$file" | awk '{print $1}'
      ;;
  esac
}

shopt -s nullglob
candidates=(
  "$DIST_DIR"/fuse-cli-*.tar.gz
  "$DIST_DIR"/fuse-cli-*.zip
  "$DIST_DIR"/fuse-vscode-*.vsix
)
shopt -u nullglob

names=()
for path in "${candidates[@]}"; do
  if [[ -f "$path" ]]; then
    names+=("$(basename "$path")")
  fi
done

if [[ "${#names[@]}" -eq 0 ]]; then
  echo "no release artifacts found in $DIST_DIR" >&2
  exit 1
fi

mapfile -t sorted_names < <(printf '%s\n' "${names[@]}" | LC_ALL=C sort -u)

declare -a hashes
declare -a sizes

TMP_SUMS="$(mktemp "$ROOT/tmp/checksums.XXXXXX")"
for name in "${sorted_names[@]}"; do
  path="$DIST_DIR/$name"
  hash="$(sha256_for_file "$path")"
  size="$(wc -c < "$path" | tr -d '[:space:]')"
  hashes+=("$hash")
  sizes+=("$size")
  printf '%s  %s\n' "$hash" "$name" >>"$TMP_SUMS"
done
mv -f "$TMP_SUMS" "$OUTPUT"

json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

TMP_META="$(mktemp "$ROOT/tmp/checksum-meta.XXXXXX")"
{
  printf '{\n'
  printf '  "generatedAtUtc": "%s",\n' "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  printf '  "artifacts": [\n'
  for i in "${!sorted_names[@]}"; do
    name="${sorted_names[$i]}"
    hash="${hashes[$i]}"
    size="${sizes[$i]}"
    comma=","
    if [[ "$i" -eq $((${#sorted_names[@]} - 1)) ]]; then
      comma=""
    fi
    printf '    {"name": "%s", "sha256": "%s", "size": %s}%s\n' \
      "$(json_escape "$name")" \
      "$hash" \
      "$size" \
      "$comma"
  done
  printf '  ]\n'
  printf '}\n'
} >"$TMP_META"
mv -f "$TMP_META" "$METADATA"

echo "wrote checksums: $OUTPUT"
echo "wrote metadata: $METADATA"
