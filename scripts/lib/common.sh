#!/usr/bin/env bash

fuse_repo_root() {
  local script_path="$1"
  (cd "$(dirname "$script_path")/.." && pwd)
}

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

clear_fuse_cache_dirs() {
  local root="$1"
  local label="${2:-cache}"
  local -a dirs=()
  local dir

  while IFS= read -r -d '' dir; do
    dirs+=("$dir")
  done < <(find "$root" -type d -name .fuse-cache -print0 2>/dev/null)

  if [[ "${#dirs[@]}" -eq 0 ]]; then
    printf "[%s] no .fuse-cache directories found under %s\n" "$label" "$root"
    return 0
  fi

  for dir in "${dirs[@]}"; do
    rm -rf "$dir"
  done

  printf "[%s] removed %d .fuse-cache director%s under %s\n" \
    "$label" \
    "${#dirs[@]}" \
    "$( [[ "${#dirs[@]}" -eq 1 ]] && printf 'y' || printf 'ies' )" \
    "$root"
}

# Resolve a path to absolute. Relative paths are anchored to $ROOT, which must
# be set in the calling script before sourcing this file.
abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

# Detect the host platform identifier (e.g. linux-x64, macos-arm64, windows-x64).
host_platform_dir() {
  local os arch
  case "$(uname -s)" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT) os="windows" ;;
    *)
      echo "unsupported host OS for platform detection: $(uname -s)" >&2
      return 1
      ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch="x64" ;;
    aarch64|arm64) arch="arm64" ;;
    *)
      echo "unsupported host arch for platform detection: $(uname -m)" >&2
      return 1
      ;;
  esac
  echo "${os}-${arch}"
}
