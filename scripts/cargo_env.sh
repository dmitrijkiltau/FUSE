#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/tmp/fuse-target}"
case "$TARGET_DIR" in
  /*) ;;
  *) TARGET_DIR="$ROOT/$TARGET_DIR" ;;
esac
export CARGO_TARGET_DIR="$TARGET_DIR"

# Keep rustc temp files in the active profile deps dir. Some filesystems used
# in local dev reject hard links across directories even on the same mount;
# rustc metadata writes can hit EXDEV unless temp and output stay co-located.
DEFAULT_PROFILE="debug"
if [[ "${1:-}" == "cargo" ]]; then
  for ((i = 1; i <= $#; i++)); do
    arg="${!i}"
    case "$arg" in
      --release)
        DEFAULT_PROFILE="release"
        ;;
      --profile=*)
        profile_value="${arg#--profile=}"
        if [[ -n "$profile_value" ]]; then
          DEFAULT_PROFILE="$profile_value"
        fi
        ;;
      --profile)
        next_index=$((i + 1))
        if (( next_index <= $# )); then
          profile_value="${!next_index}"
          if [[ -n "$profile_value" ]]; then
            DEFAULT_PROFILE="$profile_value"
          fi
        fi
        ;;
    esac
  done
fi

TMP_DIR="${RUSTC_TMPDIR:-$CARGO_TARGET_DIR/$DEFAULT_PROFILE/deps}"
case "$TMP_DIR" in
  /*) ;;
  *) TMP_DIR="$ROOT/$TMP_DIR" ;;
esac
export RUSTC_TMPDIR="$TMP_DIR"

export TMPDIR="$RUSTC_TMPDIR"
export TMP="$RUSTC_TMPDIR"
export TEMP="$RUSTC_TMPDIR"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_BUILD_PIPELINING="${CARGO_BUILD_PIPELINING:-false}"

# In this workspace, parallel test builds can intermittently hit EXDEV when
# rustc writes metadata artifacts. Default to a single build job for tests,
# while still allowing an explicit override via CARGO_BUILD_JOBS.
if [[ "${1:-}" == "cargo" && "${2:-}" == "test" ]]; then
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
fi

mkdir -p "$CARGO_TARGET_DIR" "$RUSTC_TMPDIR"

if [[ $# -eq 0 ]]; then
  echo "usage: $(basename "$0") <command...>"
  echo "example: $(basename "$0") cargo check -p fusec"
  echo "repo: $ROOT"
  exit 1
fi

if [[ "${1:-}" == "cargo" ]]; then
  log_file="$(mktemp "$RUSTC_TMPDIR/cargo-env.XXXXXX.log")"
  max_attempts="${CARGO_ENV_EXDEV_RETRIES:-6}"
  attempt=1
  status=0
  while :; do
    : >"$log_file"
    set +e
    "$@" 2> >(tee "$log_file" >&2)
    status=$?
    set -e
    if [[ $status -eq 0 ]]; then
      break
    fi
    if ! grep -q "Invalid cross-device link" "$log_file"; then
      break
    fi
    if (( attempt >= max_attempts )); then
      break
    fi
    echo "[cargo_env] EXDEV detected; retrying ($attempt/$max_attempts)" >&2
    find "$CARGO_TARGET_DIR" -type f \( -name '*.rmeta' -o -name '*.rmeta.*' \) -delete 2>/dev/null || true
    attempt=$((attempt + 1))
  done

  rm -f "$log_file"
  exit $status
fi

exec "$@"
