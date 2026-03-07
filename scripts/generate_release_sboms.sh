#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

DIST_DIR="$ROOT/dist"
OUTPUT_DIR=""
BASE_URL="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-dmitrijkiltau/fuse}"

usage() {
  cat <<'USAGE'
Usage: scripts/generate_release_sboms.sh [options]

Options:
  --dist <path>        Artifact directory (default: dist)
  --output-dir <path>  SBOM output directory (default: <dist>)
  --base-url <url>     Base URL for SPDX document namespaces
  SOURCE_DATE_EPOCH    Optional unix timestamp for deterministic created metadata
  -h, --help           Show this help
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
    --output-dir)
      if [[ -z "${2:-}" ]]; then
        echo "--output-dir requires a value" >&2
        exit 1
      fi
      OUTPUT_DIR="$(abspath "$2")"
      shift 2
      ;;
    --base-url)
      if [[ -z "${2:-}" ]]; then
        echo "--base-url requires a value" >&2
        exit 1
      fi
      BASE_URL="${2:-}"
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

if [[ -z "$OUTPUT_DIR" ]]; then
  OUTPUT_DIR="$DIST_DIR"
fi

if [[ ! -d "$DIST_DIR" ]]; then
  echo "dist directory not found: $DIST_DIR" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR" "$ROOT/tmp"

GENERATED_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
if [[ -n "${SOURCE_DATE_EPOCH:-}" ]]; then
  if [[ ! "${SOURCE_DATE_EPOCH}" =~ ^[0-9]+$ ]]; then
    echo "SOURCE_DATE_EPOCH must be an integer unix timestamp" >&2
    exit 1
  fi
  GENERATED_AT="$(iso_utc_from_epoch "$SOURCE_DATE_EPOCH")"
fi

json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

archive_entries() {
  local archive="$1"
  if [[ "$archive" == *.tar.gz ]]; then
    tar -tzf "$archive"
  else
    unzip -Z1 "$archive"
  fi | LC_ALL=C sort -u
}

payload_names=()
while IFS= read -r name; do
  [[ -n "$name" ]] && payload_names+=("$name")
done < <(list_release_payload_names "$DIST_DIR")

if [[ "${#payload_names[@]}" -eq 0 ]]; then
  echo "no release artifacts found in $DIST_DIR" >&2
  exit 1
fi

for name in "${payload_names[@]}"; do
  archive="$DIST_DIR/$name"
  stem="$(release_artifact_stem "$name")"
  output="$OUTPUT_DIR/$stem.spdx.json"
  archive_sha="$(sha256_for_file "$archive")"
  namespace="${BASE_URL%/}/releases/sbom/${stem}/${archive_sha}"

  entries=()
  while IFS= read -r entry; do
    [[ -n "$entry" ]] && entries+=("$entry")
  done < <(archive_entries "$archive")

  if [[ "${#entries[@]}" -eq 0 ]]; then
    echo "could not enumerate archive contents for $archive" >&2
    exit 1
  fi

  tmp="$(mktemp "$ROOT/tmp/release-sbom.XXXXXX")"
  {
    printf '{\n'
    printf '  "spdxVersion": "SPDX-2.3",\n'
    printf '  "dataLicense": "CC0-1.0",\n'
    printf '  "SPDXID": "SPDXRef-DOCUMENT",\n'
    printf '  "name": "%s",\n' "$(json_escape "${stem} SBOM")"
    printf '  "documentNamespace": "%s",\n' "$(json_escape "$namespace")"
    printf '  "creationInfo": {\n'
    printf '    "created": "%s",\n' "$GENERATED_AT"
    printf '    "creators": ["Tool: fuse scripts/generate_release_sboms.sh"]\n'
    printf '  },\n'
    printf '  "packages": [\n'
    printf '    {\n'
    printf '      "SPDXID": "SPDXRef-Package",\n'
    printf '      "name": "%s",\n' "$(json_escape "$stem")"
    printf '      "downloadLocation": "NOASSERTION",\n'
    printf '      "packageFileName": "%s",\n' "$(json_escape "$name")"
    printf '      "filesAnalyzed": false,\n'
    printf '      "checksums": [\n'
    printf '        {"algorithm": "SHA256", "checksumValue": "%s"}\n' "$archive_sha"
    printf '      ],\n'
    printf '      "licenseConcluded": "NOASSERTION",\n'
    printf '      "licenseDeclared": "NOASSERTION",\n'
    printf '      "copyrightText": "NOASSERTION"\n'
    printf '    }\n'
    printf '  ],\n'
    printf '  "files": [\n'
    for i in "${!entries[@]}"; do
      entry="${entries[$i]}"
      comma=","
      if [[ "$i" -eq $((${#entries[@]} - 1)) ]]; then
        comma=""
      fi
      printf '    {"SPDXID": "SPDXRef-File-%s", "fileName": "%s", "licenseConcluded": "NOASSERTION", "copyrightText": "NOASSERTION"}%s\n' \
        "$((i + 1))" \
        "$(json_escape "$entry")" \
        "$comma"
    done
    printf '  ],\n'
    printf '  "relationships": [\n'
    printf '    {"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES", "relatedSpdxElement": "SPDXRef-Package"}'
    if [[ "${#entries[@]}" -gt 0 ]]; then
      printf ',\n'
    else
      printf '\n'
    fi
    for i in "${!entries[@]}"; do
      comma=","
      if [[ "$i" -eq $((${#entries[@]} - 1)) ]]; then
        comma=""
      fi
      printf '    {"spdxElementId": "SPDXRef-Package", "relationshipType": "CONTAINS", "relatedSpdxElement": "SPDXRef-File-%s"}%s\n' \
        "$((i + 1))" \
        "$comma"
    done
    printf '  ]\n'
    printf '}\n'
  } >"$tmp"
  mv -f "$tmp" "$output"
  echo "wrote sbom: $output"
done
