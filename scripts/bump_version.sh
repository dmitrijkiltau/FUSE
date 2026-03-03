#!/usr/bin/env bash
# Bump the Fuse toolchain version across all Cargo.toml and VS Code package files.
#
# Usage: scripts/bump_version.sh <version>
# Example: scripts/bump_version.sh 0.9.0
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

DRY_RUN=0
VERSION=""

usage() {
  cat <<'USAGE'
Usage: scripts/bump_version.sh [--dry-run] <version>

Arguments:
  <version>    New version string in x.y.z format (e.g. 0.9.0)

Options:
  --dry-run    Print what would change without writing files
  -h, --help   Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      if [[ -n "$VERSION" ]]; then
        echo "unexpected argument: $1" >&2
        usage
        exit 1
      fi
      VERSION="$1"
      shift
      ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  echo "error: version argument is required" >&2
  usage
  exit 1
fi

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: version must be in x.y.z format, got: $VERSION" >&2
  exit 1
fi

bump_cargo_toml() {
  local file="$1"
  echo "  bumping $file"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "  [dry-run] would patch: $file"
    return 0
  fi
  # Only patch the package version declaration.
  perl -i -pe \
    "if (!\$done && s/^(version\\s*=\\s*\")[^\"]+(\")/\${1}${VERSION}\${2}/) { \$done = 1; }" \
    "$file"
}

bump_package_json_version() {
  local file="$1"
  echo "  bumping $file"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "  [dry-run] would patch: $file"
    return 0
  fi
  # Replace only the top-level package.json version.
  perl -i -pe \
    "if (!\$done && s/(\"version\"\\s*:\\s*\")[^\"]+(\")/\${1}${VERSION}\${2}/) { \$done = 1; }" \
    "$file"
}

bump_package_lock_version() {
  local file="$1"
  echo "  bumping $file"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "  [dry-run] would patch: $file"
    return 0
  fi
  # Update only:
  # - top-level "version"
  # - packages[""].version
  # and leave dependency versions unchanged.
  perl -i -pe '
    if (/^\s*"packages"\s*:\s*{\s*$/) { $in_packages = 1; }
    if ($in_packages && /^\s*""\s*:\s*{\s*$/) { $in_root_pkg = 1; }
    if ($in_root_pkg && /^\s*},\s*$/) { $in_root_pkg = 0; }
    if (!$top_done && s/^(\s*"version"\s*:\s*")[^"]+(")/${1}'"$VERSION"'${2}/) { $top_done = 1; }
    if ($in_root_pkg && !$root_done && s/^(\s*"version"\s*:\s*")[^"]+(")/${1}'"$VERSION"'${2}/) { $root_done = 1; }
  ' "$file"
}

# ---------------------------------------------------------------------------
# detect current version for display
# ---------------------------------------------------------------------------

CURRENT_VERSION=""
if [[ -f "$ROOT/crates/fuse/Cargo.toml" ]]; then
  CURRENT_VERSION="$(grep -m1 '^version' "$ROOT/crates/fuse/Cargo.toml" | sed 's/.*= *"\(.*\)"/\1/')"
fi

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "dry-run: would bump ${CURRENT_VERSION:-?} → $VERSION"
else
  echo "bumping ${CURRENT_VERSION:-?} → $VERSION"
fi

# ---------------------------------------------------------------------------
# Cargo crates
# ---------------------------------------------------------------------------

for manifest in \
  "$ROOT/crates/fuse/Cargo.toml" \
  "$ROOT/crates/fusec/Cargo.toml" \
  "$ROOT/crates/fuse-rt/Cargo.toml"
do
  bump_cargo_toml "$manifest"
done

# Run cargo to regenerate Cargo.lock with updated versions
if [[ "$DRY_RUN" -eq 0 ]]; then
  echo "  regenerating Cargo.lock"
  (cd "$ROOT" && "$ROOT/scripts/cargo_env.sh" cargo generate-lockfile --quiet 2>/dev/null || \
    "$ROOT/scripts/cargo_env.sh" cargo fetch --quiet 2>/dev/null || true)
fi

# ---------------------------------------------------------------------------
# VS Code extension
# ---------------------------------------------------------------------------

bump_package_json_version "$ROOT/tools/vscode/package.json"
bump_package_lock_version "$ROOT/tools/vscode/package-lock.json"

# ---------------------------------------------------------------------------
# summary
# ---------------------------------------------------------------------------

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo ""
  echo "dry-run complete — no files were modified."
else
  echo ""
  echo "version bumped to $VERSION in all locations."
  echo ""
  echo "next steps:"
  echo "  1. review changes with: git diff"
  echo "  2. run preflight:       scripts/release_preflight.sh $VERSION"
  echo "  3. commit:              git add -u && git commit -m \"release: v$VERSION\""
  echo "  4. tag:                 git tag v$VERSION && git push origin main --tags"
fi
