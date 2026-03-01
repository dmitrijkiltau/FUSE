#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ARCHIVE="$ROOT/dist/fuse-aot-linux-x64.tar.gz"
IMAGE="ghcr.io/dmitrijkiltau/fuse-aot-demo"
PUSH=0
declare -a TAGS=()

usage() {
  cat <<'USAGE'
Usage: scripts/package_aot_container_image.sh [options]

Options:
  --archive <path>   AOT release archive path (default: dist/fuse-aot-linux-x64.tar.gz)
  --image <name>     Container image name (default: ghcr.io/dmitrijkiltau/fuse-aot-demo)
  --tag <tag>        Image tag (repeatable). Defaults to `dev` if omitted.
  --push             Push tags after local build
  -h, --help         Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --archive)
      ARCHIVE="${2:-}"
      if [[ -z "$ARCHIVE" ]]; then
        echo "--archive requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --image)
      IMAGE="${2:-}"
      if [[ -z "$IMAGE" ]]; then
        echo "--image requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --tag)
      tag="${2:-}"
      if [[ -z "$tag" ]]; then
        echo "--tag requires a value" >&2
        exit 1
      fi
      TAGS+=("$tag")
      shift 2
      ;;
    --push)
      PUSH=1
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

if [[ ${#TAGS[@]} -eq 0 ]]; then
  TAGS=("dev")
fi

if [[ "$ARCHIVE" != /* ]]; then
  ARCHIVE="$ROOT/$ARCHIVE"
fi

if [[ ! -f "$ARCHIVE" ]]; then
  echo "missing archive: $ARCHIVE" >&2
  exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

mkdir -p "$ROOT/tmp"
STAGE_DIR="$(mktemp -d "$ROOT/tmp/aot-image.XXXXXX")"
cleanup() {
  rm -rf "$STAGE_DIR"
}
trap cleanup EXIT

if [[ "$ARCHIVE" == *.zip ]]; then
  if ! command -v unzip >/dev/null 2>&1; then
    echo "unzip is required for zip archives: $ARCHIVE" >&2
    exit 1
  fi
  unzip -q "$ARCHIVE" -d "$STAGE_DIR"
else
  tar -xzf "$ARCHIVE" -C "$STAGE_DIR"
fi

for required in fuse-aot-demo AOT_BUILD_INFO.txt LICENSE README.txt; do
  if [[ ! -f "$STAGE_DIR/$required" ]]; then
    echo "archive missing expected file: $required" >&2
    exit 1
  fi
done

cp "$ROOT/ops/docker/AOT_RELEASE_IMAGE.Dockerfile" "$STAGE_DIR/Dockerfile"

build_args=()
for tag in "${TAGS[@]}"; do
  build_args+=("-t" "${IMAGE}:${tag}")
done

docker build "${build_args[@]}" "$STAGE_DIR"

if [[ "$PUSH" -eq 1 ]]; then
  for tag in "${TAGS[@]}"; do
    docker push "${IMAGE}:${tag}"
  done
fi

printf 'container image built: %s\n' "$IMAGE"
for tag in "${TAGS[@]}"; do
  printf ' - %s:%s\n' "$IMAGE" "$tag"
done
