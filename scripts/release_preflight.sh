#!/usr/bin/env bash
# Full pre-tag release preflight: runs every gate required before tagging a release.
# Exits 0 on a clean release-ready tree; exits non-zero with actionable diagnostics
# on any failure.
#
# Usage: scripts/release_preflight.sh <version>
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

VERSION=""
SKIP_BENCH=0
SKIP_GUIDE_REGEN=0
WORKSPACE_PUBLISH_CHECKS=0
CLEAR_FUSE_CACHE=0

usage() {
  cat <<'USAGE'
Usage: scripts/release_preflight.sh [options] <version>

Runs the full pre-tag release checklist for the given version and exits
non-zero with actionable output on any failure.

Arguments:
  <version>          Expected release version (e.g. 0.9.0)

Options:
  --skip-bench       Skip AOT SLO and benchmark regression checks
                     (use when bench artifacts are not available locally)
  --skip-guide-regen Skip regenerating guide docs (use when already up to date)
  --workspace-publish-checks
                     Run optional workspace publish-readiness checks
  --clear-fuse-cache Remove all .fuse-cache directories under the repo before preflight
                     and before the nested release smoke step
  -h, --help         Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-bench)
      SKIP_BENCH=1
      shift
      ;;
    --skip-guide-regen)
      SKIP_GUIDE_REGEN=1
      shift
      ;;
    --workspace-publish-checks)
      WORKSPACE_PUBLISH_CHECKS=1
      shift
      ;;
    --clear-fuse-cache)
      CLEAR_FUSE_CACHE=1
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

PASS=0
FAIL=0
STEPS=()
FAILURES=()

preflight_step() {
  local label="$1"
  shift
  printf "\n[preflight] %s\n" "$label"
  if "$@"; then
    PASS=$((PASS + 1))
    STEPS+=("  ✓ $label")
  else
    local exit_code=$?
    FAIL=$((FAIL + 1))
    STEPS+=("  ✗ $label (exit $exit_code)")
    FAILURES+=("$label")
  fi
}

run_release_smoke() {
  local -a args=()
  if [[ "$CLEAR_FUSE_CACHE" -eq 1 ]]; then
    args+=(--clear-fuse-cache)
  fi
  "$ROOT/scripts/release_smoke.sh" "${args[@]}"
}

if [[ "$CLEAR_FUSE_CACHE" -eq 1 ]]; then
  preflight_step "clear .fuse-cache directories" \
    clear_fuse_cache_dirs "$ROOT" "preflight"
fi

# ---------------------------------------------------------------------------
# 1. Version bump verification
# ---------------------------------------------------------------------------

check_versions() {
  local ok=0

  check_file_version() {
    local file="$1"
    local pattern="$2"
    local actual
    actual="$(grep -m1 -oP "$pattern" "$file" 2>/dev/null || true)"
    if [[ "$actual" != "$VERSION" ]]; then
      echo "  version mismatch in $file: expected $VERSION, found '${actual:-<none>}'" >&2
      ok=1
    fi
  }

  check_file_version "$ROOT/crates/fuse/Cargo.toml"      '(?<=version = ")[^"]+'
  check_file_version "$ROOT/crates/fusec/Cargo.toml"     '(?<=version = ")[^"]+'
  check_file_version "$ROOT/crates/fuse-rt/Cargo.toml"   '(?<=version = ")[^"]+'
  check_file_version "$ROOT/tools/vscode/package.json"   '(?<="version": ")[^"]+'

  return "$ok"
}

preflight_step "version bump — all locations match $VERSION" check_versions

# ---------------------------------------------------------------------------
# 2. CHANGELOG.md contains an entry for the new version
# ---------------------------------------------------------------------------

check_changelog() {
  if ! grep -qF "## [$VERSION]" "$ROOT/CHANGELOG.md" && \
     ! grep -qF "## $VERSION" "$ROOT/CHANGELOG.md"; then
    echo "  CHANGELOG.md does not contain an entry for $VERSION" >&2
    echo "  add a '## [$VERSION] — ...' section before running preflight" >&2
    return 1
  fi
}

preflight_step "CHANGELOG.md contains $VERSION entry" check_changelog

# ---------------------------------------------------------------------------
# 3. Guide docs are up to date
# ---------------------------------------------------------------------------

if [[ "$SKIP_GUIDE_REGEN" -eq 1 ]]; then
  printf "\n[preflight] guide regeneration skipped (--skip-guide-regen)\n"
else
  check_guides() {
    local tmp
    tmp="$(mktemp -d)"
    cp "$ROOT/guides/reference.md"          "$tmp/reference.md"         2>/dev/null || true
    cp "$ROOT/guides/onboarding.md"         "$tmp/onboarding.md"        2>/dev/null || true
    cp "$ROOT/guides/boundary-contracts.md" "$tmp/boundary-contracts.md" 2>/dev/null || true

    "$ROOT/scripts/generate_guide_docs.sh" >/dev/null 2>&1 || {
      echo "  generate_guide_docs.sh failed" >&2
      rm -rf "$tmp"
      return 1
    }

    local changed=0
    for f in reference.md onboarding.md boundary-contracts.md; do
      if ! diff -q "$tmp/$f" "$ROOT/guides/$f" >/dev/null 2>&1; then
        echo "  guides/$f is out of date — commit the regenerated version" >&2
        changed=1
      fi
    done
    rm -rf "$tmp"
    return "$changed"
  }

  preflight_step "guide docs up to date" check_guides
fi

# ---------------------------------------------------------------------------
# 4. Optional workspace publish-readiness gate
# ---------------------------------------------------------------------------

if [[ "$WORKSPACE_PUBLISH_CHECKS" -eq 1 ]]; then
  check_workspace_publish_readiness() {
    "$ROOT/scripts/fuse" deps publish-check --manifest-path "$ROOT"
  }

  preflight_step "workspace publish-readiness (fuse deps publish-check)" \
    check_workspace_publish_readiness
else
  printf "\n[preflight] workspace publish-readiness skipped (use --workspace-publish-checks)\n"
fi

# ---------------------------------------------------------------------------
# 5. Authority parity gate
# ---------------------------------------------------------------------------

preflight_step "authority parity (authority_parity.sh)" \
  "$ROOT/scripts/authority_parity.sh"

# ---------------------------------------------------------------------------
# 6. Release smoke
# ---------------------------------------------------------------------------

preflight_step "release smoke (release_smoke.sh)" run_release_smoke

# ---------------------------------------------------------------------------
# 7. AOT SLO check
# ---------------------------------------------------------------------------

if [[ "$SKIP_BENCH" -eq 1 ]]; then
  printf "\n[preflight] AOT SLO check skipped (--skip-bench)\n"
else
  check_aot_slo() {
    local metrics="$ROOT/.fuse/bench/aot_perf_metrics.json"
    if [[ ! -f "$metrics" ]]; then
      echo "  missing $metrics — run scripts/aot_perf_bench.sh first or use --skip-bench" >&2
      return 1
    fi
    "$ROOT/scripts/check_aot_perf_slo.sh"
  }

  preflight_step "AOT performance SLO (check_aot_perf_slo.sh)" check_aot_slo
fi

# ---------------------------------------------------------------------------
# 8. Use-case benchmark regression gate
# ---------------------------------------------------------------------------

if [[ "$SKIP_BENCH" -eq 1 ]]; then
  printf "\n[preflight] benchmark regression check skipped (--skip-bench)\n"
else
  check_bench_regression() {
    if [[ ! -f "$ROOT/scripts/check_use_case_bench_regression.sh" ]]; then
      echo "  check_use_case_bench_regression.sh not found — skipping" >&2
      return 0
    fi
    "$ROOT/scripts/check_use_case_bench_regression.sh"
  }

  preflight_step "use-case benchmark regression (check_use_case_bench_regression.sh)" \
    check_bench_regression
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo ""
echo "======================================================================"
echo "release preflight v$VERSION — summary"
echo "======================================================================"
for s in "${STEPS[@]}"; do
  echo "$s"
done
echo ""
echo "passed: $PASS   failed: $FAIL"
echo ""

if [[ "$FAIL" -gt 0 ]]; then
  echo "PREFLIGHT FAILED — resolve the following before tagging:"
  for f in "${FAILURES[@]}"; do
    echo "  • $f"
  done
  echo ""
  exit 1
fi

echo "PREFLIGHT PASSED — tree is release-ready for v$VERSION"
echo ""
echo "next steps:"
echo "  1. build artifacts:  scripts/package_release.sh --release"
echo "  2. commit:           git add -u && git commit -m 'release: v$VERSION'"
echo "  3. tag:              git tag v$VERSION && git push origin main --tags"
