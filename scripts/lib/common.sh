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

detect_sha256_tool() {
  if [[ -n "${HASH_TOOL:-}" ]]; then
    printf '%s\n' "$HASH_TOOL"
    return 0
  fi

  if command -v sha256sum >/dev/null 2>&1; then
    HASH_TOOL="sha256sum"
  elif command -v shasum >/dev/null 2>&1; then
    HASH_TOOL="shasum"
  elif command -v openssl >/dev/null 2>&1; then
    HASH_TOOL="openssl"
  else
    echo "missing SHA-256 tool: install sha256sum, shasum, or openssl" >&2
    return 1
  fi

  printf '%s\n' "$HASH_TOOL"
}

sha256_for_file() {
  local file="$1"
  local tool
  tool="$(detect_sha256_tool)"

  case "$tool" in
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

sha256_from_stdin() {
  local tool
  tool="$(detect_sha256_tool)"

  case "$tool" in
    sha256sum)
      sha256sum | awk '{print $1}'
      ;;
    shasum)
      shasum -a 256 | awk '{print $1}'
      ;;
    openssl)
      openssl dgst -sha256 -r | awk '{print $1}'
      ;;
  esac
}

iso_utc_from_epoch() {
  local epoch="$1"
  if date -u -d "@$epoch" +"%Y-%m-%dT%H:%M:%SZ" >/dev/null 2>&1; then
    date -u -d "@$epoch" +"%Y-%m-%dT%H:%M:%SZ"
  else
    date -u -r "$epoch" +"%Y-%m-%dT%H:%M:%SZ"
  fi
}

release_artifact_stem() {
  local name="$1"
  case "$name" in
    *.tar.gz) printf '%s\n' "${name%.tar.gz}" ;;
    *.zip) printf '%s\n' "${name%.zip}" ;;
    *.vsix) printf '%s\n' "${name%.vsix}" ;;
    *.json) printf '%s\n' "${name%.json}" ;;
    *) printf '%s\n' "$name" ;;
  esac
}

list_release_payload_names() {
  local dist_dir="$1"
  local -a names=()
  local path

  while IFS= read -r -d '' path; do
    names+=("$(basename "$path")")
  done < <(find "$dist_dir" -maxdepth 1 -type f \
    \( -name 'fuse-cli-*.tar.gz' -o -name 'fuse-cli-*.zip' \
       -o -name 'fuse-aot-*.tar.gz' -o -name 'fuse-aot-*.zip' \
       -o -name 'fuse-vscode-*.vsix' \) \
    -print0)

  if [[ "${#names[@]}" -eq 0 ]]; then
    return 0
  fi

  printf '%s\n' "${names[@]}" | LC_ALL=C sort -u
}
