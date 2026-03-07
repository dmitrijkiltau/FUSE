#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

DIST_DIR="$ROOT/dist"
OUTPUT="$DIST_DIR/release-provenance.json"
REPOSITORY="${GITHUB_REPOSITORY:-}"
REF="${GITHUB_REF:-}"
TAG="${GITHUB_REF_NAME:-}"
SHA="${GITHUB_SHA:-}"
WORKFLOW_NAME="${GITHUB_WORKFLOW:-}"
WORKFLOW_PATH=".github/workflows/release-artifacts.yml"
WORKFLOW_REF="${GITHUB_REF:-}"
EVENT_NAME="${GITHUB_EVENT_NAME:-}"
ACTOR="${GITHUB_ACTOR:-}"
RUN_ID="${GITHUB_RUN_ID:-}"
RUN_ATTEMPT="${GITHUB_RUN_ATTEMPT:-}"

usage() {
  cat <<'USAGE'
Usage: scripts/generate_release_provenance.sh [options]

Options:
  --dist <path>           Artifact directory (default: dist)
  --output <path>         Provenance output path (default: <dist>/release-provenance.json)
  --repository <owner/repo>
                          Repository slug used for auditing fields
  --ref <git-ref>         Git ref that produced the artifacts (e.g. refs/tags/v0.9.4)
  --tag <tag>             Release tag name (e.g. v0.9.4)
  --sha <commit>          Release commit SHA
  --workflow-name <name>  Workflow display name
  --workflow-path <path>  Workflow file path (default: .github/workflows/release-artifacts.yml)
  --workflow-ref <ref>    Workflow ref encoded into signing identity / audit trail
  --event <name>          GitHub event name
  --actor <name>          Triggering actor
  --run-id <id>           Workflow run id
  --run-attempt <n>       Workflow run attempt
  SOURCE_DATE_EPOCH       Optional unix timestamp for deterministic generatedAtUtc metadata
  -h, --help              Show this help
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
    --repository)
      REPOSITORY="${2:-}"
      if [[ -z "$REPOSITORY" ]]; then
        echo "--repository requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --ref)
      REF="${2:-}"
      if [[ -z "$REF" ]]; then
        echo "--ref requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --tag)
      TAG="${2:-}"
      if [[ -z "$TAG" ]]; then
        echo "--tag requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --sha)
      SHA="${2:-}"
      if [[ -z "$SHA" ]]; then
        echo "--sha requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --workflow-name)
      WORKFLOW_NAME="${2:-}"
      if [[ -z "$WORKFLOW_NAME" ]]; then
        echo "--workflow-name requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --workflow-path)
      WORKFLOW_PATH="${2:-}"
      if [[ -z "$WORKFLOW_PATH" ]]; then
        echo "--workflow-path requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --workflow-ref)
      WORKFLOW_REF="${2:-}"
      if [[ -z "$WORKFLOW_REF" ]]; then
        echo "--workflow-ref requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --event)
      EVENT_NAME="${2:-}"
      if [[ -z "$EVENT_NAME" ]]; then
        echo "--event requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --actor)
      ACTOR="${2:-}"
      if [[ -z "$ACTOR" ]]; then
        echo "--actor requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --run-id)
      RUN_ID="${2:-}"
      if [[ -z "$RUN_ID" ]]; then
        echo "--run-id requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --run-attempt)
      RUN_ATTEMPT="${2:-}"
      if [[ -z "$RUN_ATTEMPT" ]]; then
        echo "--run-attempt requires a value" >&2
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

if [[ ! -d "$DIST_DIR" ]]; then
  echo "dist directory not found: $DIST_DIR" >&2
  exit 1
fi

if [[ "$REF" == refs/tags/* && -z "$TAG" ]]; then
  TAG="${REF#refs/tags/}"
fi

required_fields=(
  REPOSITORY
  REF
  TAG
  SHA
  WORKFLOW_NAME
  WORKFLOW_PATH
  WORKFLOW_REF
  EVENT_NAME
  ACTOR
  RUN_ID
  RUN_ATTEMPT
)

for field in "${required_fields[@]}"; do
  if [[ -z "${!field}" ]]; then
    echo "missing required provenance field: ${field}" >&2
    exit 1
  fi
done

if [[ ! -f "$DIST_DIR/SHA256SUMS" ]]; then
  echo "missing checksums manifest: $DIST_DIR/SHA256SUMS" >&2
  exit 1
fi

payload_names=()
while IFS= read -r name; do
  [[ -n "$name" ]] && payload_names+=("$name")
done < <(list_release_payload_names "$DIST_DIR")

if [[ "${#payload_names[@]}" -eq 0 ]]; then
  echo "no release artifacts found in $DIST_DIR" >&2
  exit 1
fi

sbom_names=()
while IFS= read -r -d '' path; do
  sbom_names+=("$(basename "$path")")
done < <(find "$DIST_DIR" -maxdepth 1 -type f -name '*.spdx.json' -print0)

if [[ "${#sbom_names[@]}" -gt 0 ]]; then
  mapfile -t sbom_names < <(printf '%s\n' "${sbom_names[@]}" | LC_ALL=C sort -u)
fi

if [[ "${#sbom_names[@]}" -ne "${#payload_names[@]}" ]]; then
  echo "expected one SBOM per payload before generating provenance" >&2
  exit 1
fi

find_payload_for_sbom() {
  local sbom_name="$1"
  local stem="${sbom_name%.spdx.json}"
  local payload
  for payload in "${payload_names[@]}"; do
    if [[ "$(release_artifact_stem "$payload")" == "$stem" ]]; then
      printf '%s\n' "$payload"
      return 0
    fi
  done
  return 1
}

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

json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

mkdir -p "$(dirname "$OUTPUT")" "$ROOT/tmp"
tmp="$(mktemp "$ROOT/tmp/release-provenance.XXXXXX")"

{
  printf '{\n'
  printf '  "schemaVersion": 1,\n'
  printf '  "generatedAtUtc": "%s",\n' "$GENERATED_AT"
  if [[ -n "$SOURCE_EPOCH" ]]; then
    printf '  "sourceDateEpoch": %s,\n' "$SOURCE_EPOCH"
  fi
  printf '  "repository": "%s",\n' "$(json_escape "$REPOSITORY")"
  printf '  "ref": "%s",\n' "$(json_escape "$REF")"
  printf '  "tag": "%s",\n' "$(json_escape "$TAG")"
  printf '  "commit": "%s",\n' "$(json_escape "$SHA")"
  printf '  "workflow": {\n'
  printf '    "name": "%s",\n' "$(json_escape "$WORKFLOW_NAME")"
  printf '    "path": "%s",\n' "$(json_escape "$WORKFLOW_PATH")"
  printf '    "ref": "%s",\n' "$(json_escape "$WORKFLOW_REF")"
  printf '    "event": "%s",\n' "$(json_escape "$EVENT_NAME")"
  printf '    "actor": "%s",\n' "$(json_escape "$ACTOR")"
  printf '    "runId": "%s",\n' "$(json_escape "$RUN_ID")"
  printf '    "runAttempt": %s\n' "$RUN_ATTEMPT"
  printf '  },\n'
  printf '  "artifacts": [\n'
  for i in "${!payload_names[@]}"; do
    name="${payload_names[$i]}"
    comma=","
    if [[ "$i" -eq $((${#payload_names[@]} - 1)) ]]; then
      comma=""
    fi
    printf '    {"name": "%s", "sha256": "%s", "size": %s}%s\n' \
      "$(json_escape "$name")" \
      "$(sha256_for_file "$DIST_DIR/$name")" \
      "$(wc -c < "$DIST_DIR/$name" | tr -d '[:space:]')" \
      "$comma"
  done
  printf '  ],\n'
  printf '  "sboms": [\n'
  for i in "${!sbom_names[@]}"; do
    name="${sbom_names[$i]}"
    payload_name="$(find_payload_for_sbom "$name" || true)"
    if [[ -z "$payload_name" ]]; then
      echo "could not map SBOM to payload: $name" >&2
      rm -f "$tmp"
      exit 1
    fi
    comma=","
    if [[ "$i" -eq $((${#sbom_names[@]} - 1)) ]]; then
      comma=""
    fi
    printf '    {"name": "%s", "subject": "%s", "sha256": "%s", "size": %s}%s\n' \
      "$(json_escape "$name")" \
      "$(json_escape "$payload_name")" \
      "$(sha256_for_file "$DIST_DIR/$name")" \
      "$(wc -c < "$DIST_DIR/$name" | tr -d '[:space:]')" \
      "$comma"
  done
  printf '  ],\n'
  printf '  "checksums": {\n'
  printf '    "name": "SHA256SUMS",\n'
  printf '    "sha256": "%s",\n' "$(sha256_for_file "$DIST_DIR/SHA256SUMS")"
  printf '    "size": %s\n' "$(wc -c < "$DIST_DIR/SHA256SUMS" | tr -d '[:space:]')"
  printf '  }\n'
  printf '}\n'
} >"$tmp"

mv -f "$tmp" "$OUTPUT"
echo "wrote provenance: $OUTPUT"
