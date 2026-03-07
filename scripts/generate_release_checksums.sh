#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"
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
  SOURCE_DATE_EPOCH  Optional unix timestamp for deterministic generatedAtUtc metadata
  -h, --help         Show this help
USAGE
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

GENERATED_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
SOURCE_EPOCH=""
if [[ -n "${SOURCE_DATE_EPOCH:-}" ]]; then
  if [[ ! "${SOURCE_DATE_EPOCH}" =~ ^[0-9]+$ ]]; then
    echo "SOURCE_DATE_EPOCH must be an integer unix timestamp" >&2
    exit 1
  fi
  SOURCE_EPOCH="${SOURCE_DATE_EPOCH}"
  GENERATED_AT="$(iso_utc_from_epoch "$SOURCE_EPOCH")"
fi

names=()
while IFS= read -r name; do
  [[ -n "$name" ]] && names+=("$name")
done < <(list_release_payload_names "$DIST_DIR")

if [[ "${#names[@]}" -eq 0 ]]; then
  echo "no release artifacts found in $DIST_DIR" >&2
  exit 1
fi

sorted_names=("${names[@]}")

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
detect_integrity_names() {
  local -a discovered=()
  local path

  while IFS= read -r -d '' path; do
    discovered+=("$(basename "$path")")
  done < <(find "$DIST_DIR" -maxdepth 1 -type f \
    \( -name 'SHA256SUMS' -o -name 'SHA256SUMS.sig' -o -name 'SHA256SUMS.pem' \
       -o -name 'release-provenance.json' -o -name 'release-provenance.sig' \
       -o -name 'release-provenance.pem' -o -name '*.spdx.json' \) \
    -print0)

  if [[ "${#discovered[@]}" -eq 0 ]]; then
    return 0
  fi

  printf '%s\n' "${discovered[@]}" | LC_ALL=C sort -u
}

integrity_names=()
while IFS= read -r name; do
  [[ -n "$name" ]] && integrity_names+=("$name")
done < <(detect_integrity_names)

integrity_kind() {
  local name="$1"
  case "$name" in
    SHA256SUMS) printf 'checksums\n' ;;
    *.spdx.json) printf 'sbom\n' ;;
    *.sig) printf 'signature\n' ;;
    *.pem) printf 'certificate\n' ;;
    release-provenance.json) printf 'provenance\n' ;;
    *) printf 'auxiliary\n' ;;
  esac
}

integrity_subject() {
  local name="$1"
  case "$name" in
    SHA256SUMS.sig|SHA256SUMS.pem) printf 'SHA256SUMS\n' ;;
    release-provenance.sig|release-provenance.pem) printf 'release-provenance.json\n' ;;
    *.spdx.json)
      local stem="${name%.spdx.json}"
      local payload
      for payload in "${sorted_names[@]}"; do
        if [[ "$(release_artifact_stem "$payload")" == "$stem" ]]; then
          printf '%s\n' "$payload"
          return 0
        fi
      done
      ;;
  esac
}

{
  printf '{\n'
  printf '  "generatedAtUtc": "%s",\n' "$GENERATED_AT"
  if [[ -n "$SOURCE_EPOCH" ]]; then
    printf '  "sourceDateEpoch": %s,\n' "$SOURCE_EPOCH"
  fi
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
  printf '  ]'
  if [[ "${#integrity_names[@]}" -gt 0 ]]; then
    printf ',\n'
    printf '  "integrityArtifacts": [\n'
    for i in "${!integrity_names[@]}"; do
      name="${integrity_names[$i]}"
      hash="$(sha256_for_file "$DIST_DIR/$name")"
      size="$(wc -c < "$DIST_DIR/$name" | tr -d '[:space:]')"
      kind="$(integrity_kind "$name")"
      subject="$(integrity_subject "$name" || true)"
      comma=","
      if [[ "$i" -eq $((${#integrity_names[@]} - 1)) ]]; then
        comma=""
      fi
      printf '    {"name": "%s", "kind": "%s", "sha256": "%s", "size": %s' \
        "$(json_escape "$name")" \
        "$kind" \
        "$hash" \
        "$size"
      if [[ -n "$subject" ]]; then
        printf ', "subject": "%s"' "$(json_escape "$subject")"
      fi
      printf '}%s\n' "$comma"
    done
    printf '  ]\n'
  else
    printf '\n'
  fi
  printf '}\n'
} >"$TMP_META"
mv -f "$TMP_META" "$METADATA"

echo "wrote checksums: $OUTPUT"
echo "wrote metadata: $METADATA"
