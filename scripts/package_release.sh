#!/usr/bin/env bash
# Single entry point for building all host-platform release artifacts.
# Dispatches to the individual cli / aot / vsix / container-image packaging
# scripts, then generates the combined checksum + metadata files.
#
# Usage: scripts/package_release.sh [options]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

RELEASE=0
SKIP_BUILD=0
SKIP_CONTAINER=0
PUSH_CONTAINER=0
PLATFORM=""
MANIFEST_PATH="$ROOT"
IMAGE="ghcr.io/dmitrijkiltau/fuse-aot-demo"
declare -a CONTAINER_TAGS=()

usage() {
  cat <<'USAGE'
Usage: scripts/package_release.sh [options]

Builds all release artifacts for the host platform: CLI archive, AOT archive,
VS Code VSIX, checksums, and optionally a container image.

Options:
  --platform <name>        Target platform identifier (default: host platform,
                           e.g. linux-x64, macos-arm64, windows-x64)
  --release                Build dist binaries in release mode
  --skip-build             Skip scripts/build_dist.sh (reuse existing dist/ binaries)
  --skip-container         Skip container image packaging (default on non-linux)
  --push-container         Push container image tags to registry after build
  --image <name>           Container image name
                           (default: ghcr.io/dmitrijkiltau/fuse-aot-demo)
  --tag <tag>              Container image tag (repeatable; default: dev)
  --manifest-path <path>   fuse.toml or directory for AOT artifact
                           (default: repo root)
  -h, --help               Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform)
      PLATFORM="${2:-}"
      if [[ -z "$PLATFORM" ]]; then echo "--platform requires a value" >&2; exit 1; fi
      shift 2
      ;;
    --release)
      RELEASE=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-container)
      SKIP_CONTAINER=1
      shift
      ;;
    --push-container)
      PUSH_CONTAINER=1
      shift
      ;;
    --image)
      IMAGE="${2:-}"
      if [[ -z "$IMAGE" ]]; then echo "--image requires a value" >&2; exit 1; fi
      shift 2
      ;;
    --tag)
      tag="${2:-}"
      if [[ -z "$tag" ]]; then echo "--tag requires a value" >&2; exit 1; fi
      CONTAINER_TAGS+=("$tag")
      shift 2
      ;;
    --manifest-path)
      MANIFEST_PATH="${2:-}"
      if [[ -z "$MANIFEST_PATH" ]]; then echo "--manifest-path requires a value" >&2; exit 1; fi
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
  PLATFORM="$(host_platform_dir)"
fi

# Auto-skip container packaging on non-linux hosts (docker Linux containers
# require a Linux daemon; skip silently on macOS/Windows).
if [[ "$SKIP_CONTAINER" -eq 0 ]]; then
  case "$(uname -s)" in
    Linux) ;;
    *) SKIP_CONTAINER=1 ;;
  esac
fi

if [[ ${#CONTAINER_TAGS[@]} -eq 0 ]]; then
  CONTAINER_TAGS=("dev")
fi

# ---------------------------------------------------------------------------
# Shared build args
# ---------------------------------------------------------------------------

BUILD_ARGS=()
if [[ "$RELEASE" -eq 1 ]]; then
  BUILD_ARGS+=(--release)
fi
if [[ "$SKIP_BUILD" -eq 1 ]]; then
  BUILD_ARGS+=(--skip-build)
fi

DIST_DIR="$ROOT/dist"
mkdir -p "$DIST_DIR"

# ---------------------------------------------------------------------------
# 1. CLI artifact
# ---------------------------------------------------------------------------

step "1/5" "Package CLI artifact ($PLATFORM)"
"$ROOT/scripts/package_cli_artifacts.sh" --platform "$PLATFORM" "${BUILD_ARGS[@]}"

# ---------------------------------------------------------------------------
# 2. AOT artifact
# ---------------------------------------------------------------------------

step "2/5" "Package AOT artifact ($PLATFORM)"
AOT_ARGS=(--platform "$PLATFORM" --manifest-path "$MANIFEST_PATH")
AOT_ARGS+=("${BUILD_ARGS[@]}")
"$ROOT/scripts/package_aot_artifact.sh" "${AOT_ARGS[@]}"

# ---------------------------------------------------------------------------
# 3. VS Code VSIX
# ---------------------------------------------------------------------------

step "3/5" "Package VS Code extension ($PLATFORM)"
"$ROOT/scripts/package_vscode_extension.sh" --platform "$PLATFORM" "${BUILD_ARGS[@]}" --skip-build

# ---------------------------------------------------------------------------
# 4. Checksums and metadata
# ---------------------------------------------------------------------------

step "4/5" "Generate checksums and release metadata"
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" show -s --format=%ct HEAD 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH
"$ROOT/scripts/generate_release_checksums.sh"

# ---------------------------------------------------------------------------
# 5. Container image (Linux only, optional)
# ---------------------------------------------------------------------------

if [[ "$SKIP_CONTAINER" -eq 1 ]]; then
  step "5/5" "Container image packaging skipped (--skip-container or non-linux host)"
else
  step "5/5" "Package AOT container image"
  IMAGE_ARGS=(--archive "$DIST_DIR/fuse-aot-linux-x64.tar.gz" --image "$IMAGE")
  for t in "${CONTAINER_TAGS[@]}"; do
    IMAGE_ARGS+=(--tag "$t")
  done
  if [[ "$PUSH_CONTAINER" -eq 1 ]]; then
    IMAGE_ARGS+=(--push)
  fi
  "$ROOT/scripts/package_aot_container_image.sh" "${IMAGE_ARGS[@]}"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo ""
echo "release packaging complete for $PLATFORM"
echo ""
echo "artifacts:"
for f in "$DIST_DIR"/fuse-cli-"$PLATFORM".* \
          "$DIST_DIR"/fuse-aot-"$PLATFORM".* \
          "$DIST_DIR"/fuse-vscode-"$PLATFORM".vsix \
          "$DIST_DIR"/SHA256SUMS \
          "$DIST_DIR"/release-artifacts.json; do
  [[ -e "$f" ]] && echo "  $f"
done
echo ""
echo "next: scripts/release_preflight.sh <version>"
