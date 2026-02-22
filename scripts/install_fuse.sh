#!/usr/bin/env bash
set -euo pipefail

PROG_NAME="$(basename "$0")"

# Defaults are overridable for self-hosted distribution.
INSTALL_DIR="${FUSE_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="${FUSE_INSTALL_BASE_URL:-}"
VERSION=""
PLATFORM=""

usage() {
  cat <<EOF
Usage: $PROG_NAME [options]

Install FUSE CLI binaries (fuse + fuse-lsp) for the current platform.

Options:
  --install-dir <path>   Install directory (default: \$HOME/.local/bin)
  --base-url <url>       Base URL that contains fuse-cli-<platform>.tar.gz
  --version <vX.Y.Z>     Version tag when downloading from GitHub Releases
  --platform <name>      Override detected platform (e.g. linux-x64, macos-arm64)
  -h, --help             Show this help

Environment variables:
  FUSE_INSTALL_DIR       Same as --install-dir
  FUSE_INSTALL_BASE_URL  Same as --base-url

Examples:
  curl -fsSL https://fuse.kiltau.dev/install | bash
  curl -fsSL https://fuse.kiltau.dev/install | bash -s -- --install-dir ~/.local/bin
  curl -fsSL https://fuse.kiltau.dev/install | bash -s -- --version v0.4.0
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      INSTALL_DIR="${2:-}"
      if [[ -z "$INSTALL_DIR" ]]; then
        echo "--install-dir requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --base-url)
      BASE_URL="${2:-}"
      if [[ -z "$BASE_URL" ]]; then
        echo "--base-url requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      if [[ -z "$VERSION" ]]; then
        echo "--version requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --platform)
      PLATFORM="${2:-}"
      if [[ -z "$PLATFORM" ]]; then
        echo "--platform requires a value" >&2
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

command -v tar >/dev/null 2>&1 || {
  echo "missing required command: tar" >&2
  exit 1
}

download_file() {
  local url="$1"
  local out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$out"
    return 0
  fi
  if command -v wget >/dev/null 2>&1; then
    wget -qO "$out" "$url"
    return 0
  fi
  echo "missing downloader: install curl or wget" >&2
  return 1
}

detect_platform() {
  local os arch
  case "$(uname -s)" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    *)
      echo "unsupported OS: $(uname -s)" >&2
      return 1
      ;;
  esac

  case "$(uname -m)" in
    x86_64|amd64) arch="x64" ;;
    aarch64|arm64) arch="arm64" ;;
    *)
      echo "unsupported architecture: $(uname -m)" >&2
      return 1
      ;;
  esac

  echo "${os}-${arch}"
}

if [[ -z "$PLATFORM" ]]; then
  PLATFORM="$(detect_platform)"
fi

# Published release artifacts currently use tar.gz for Linux/macOS CLI bundles.
ARTIFACT="fuse-cli-${PLATFORM}.tar.gz"

declare -a base_candidates
if [[ -n "$BASE_URL" ]]; then
  base_candidates+=("${BASE_URL%/}")
elif [[ -n "$VERSION" ]]; then
  tag="${VERSION#v}"
  base_candidates+=("https://github.com/dmitrijkiltau/FUSE/releases/download/v${tag}")
else
  # Prefer self-hosted endpoint first; fall back to GitHub latest.
  base_candidates+=("https://fuse.kiltau.dev/downloads")
  base_candidates+=("https://github.com/dmitrijkiltau/FUSE/releases/latest/download")
fi

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/fuse-install.XXXXXX")"
cleanup() {
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

ARCHIVE="$TMP_ROOT/$ARTIFACT"
DOWNLOADED_URL=""
for base in "${base_candidates[@]}"; do
  url="${base}/${ARTIFACT}"
  if download_file "$url" "$ARCHIVE"; then
    DOWNLOADED_URL="$url"
    break
  fi
done

if [[ -z "$DOWNLOADED_URL" ]]; then
  echo "failed to download $ARTIFACT from all candidate sources:" >&2
  for base in "${base_candidates[@]}"; do
    echo "  - ${base}/${ARTIFACT}" >&2
  done
  exit 1
fi

mkdir -p "$TMP_ROOT/unpack" "$INSTALL_DIR"
tar -xzf "$ARCHIVE" -C "$TMP_ROOT/unpack"

if [[ ! -f "$TMP_ROOT/unpack/fuse" || ! -f "$TMP_ROOT/unpack/fuse-lsp" ]]; then
  echo "unexpected archive layout: expected fuse and fuse-lsp binaries" >&2
  exit 1
fi

install -m 0755 "$TMP_ROOT/unpack/fuse" "$INSTALL_DIR/fuse"
install -m 0755 "$TMP_ROOT/unpack/fuse-lsp" "$INSTALL_DIR/fuse-lsp"

echo "installed from: $DOWNLOADED_URL"
echo "installed binaries:"
echo "  - $INSTALL_DIR/fuse"
echo "  - $INSTALL_DIR/fuse-lsp"

case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    echo "PATH already includes $INSTALL_DIR"
    ;;
  *)
    echo
    echo "Add $INSTALL_DIR to PATH (if needed):"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
