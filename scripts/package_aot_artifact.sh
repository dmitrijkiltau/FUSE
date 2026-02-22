#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT/dist"

RELEASE=0
SKIP_BUILD=0
PLATFORM=""
MANIFEST_PATH="$ROOT"
AOT_PATH=""

usage() {
  cat <<'USAGE'
Usage: scripts/package_aot_artifact.sh [options]

Options:
  --platform <name>       Target platform identifier (default: host platform, e.g. linux-x64)
  --manifest-path <path>  Manifest directory or fuse.toml path (default: repo root)
  --aot-path <path>       Expected AOT output path relative to manifest dir
  --release               Build AOT binary in release mode
  --skip-build            Skip `scripts/fuse build --aot`
  -h, --help              Show this help
USAGE
}

abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

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

resolve_manifest_dir() {
  local path="$1"
  if [[ -d "$path" ]]; then
    if [[ ! -f "$path/fuse.toml" ]]; then
      echo "missing fuse.toml in manifest directory: $path" >&2
      return 1
    fi
    printf '%s\n' "$path"
    return 0
  fi
  if [[ -f "$path" ]]; then
    if [[ "$(basename "$path")" != "fuse.toml" ]]; then
      echo "--manifest-path file must be fuse.toml: $path" >&2
      return 1
    fi
    printf '%s\n' "$(dirname "$path")"
    return 0
  fi
  echo "manifest path not found: $path" >&2
  return 1
}

resolve_binary() {
  local base="$1"
  local -a candidates=("$base")

  if [[ "$PLATFORM" == windows-* ]]; then
    if [[ "$base" != *.exe ]]; then
      candidates+=("${base}.exe")
    fi
  elif [[ "$base" == *.exe ]]; then
    candidates+=("${base%.exe}")
  fi

  local candidate
  for candidate in "${candidates[@]}"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform)
      PLATFORM="${2:-}"
      if [[ -z "$PLATFORM" ]]; then
        echo "--platform requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --manifest-path)
      MANIFEST_PATH="${2:-}"
      if [[ -z "$MANIFEST_PATH" ]]; then
        echo "--manifest-path requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --aot-path)
      AOT_PATH="${2:-}"
      if [[ -z "$AOT_PATH" ]]; then
        echo "--aot-path requires a value" >&2
        exit 1
      fi
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

MANIFEST_PATH="$(abspath "$MANIFEST_PATH")"
MANIFEST_DIR="$(resolve_manifest_dir "$MANIFEST_PATH")"

if [[ "$PLATFORM" == windows-* ]]; then
  BIN_NAME="fuse-aot-demo.exe"
  ARCHIVE="$DIST_DIR/fuse-aot-${PLATFORM}.zip"
else
  BIN_NAME="fuse-aot-demo"
  ARCHIVE="$DIST_DIR/fuse-aot-${PLATFORM}.tar.gz"
fi

BUILD_ARGS=(build --manifest-path "$MANIFEST_PATH" --aot)
if [[ "$RELEASE" -eq 1 ]]; then
  BUILD_ARGS+=(--release)
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  "$ROOT/scripts/fuse" "${BUILD_ARGS[@]}"
fi

search_paths=()
if [[ -n "$AOT_PATH" ]]; then
  search_paths+=("$AOT_PATH")
else
  search_paths+=("build/app" ".fuse/build/program.aot")
fi

SRC_BIN=""
for rel_path in "${search_paths[@]}"; do
  if [[ "$rel_path" == /* ]]; then
    candidate="$rel_path"
  else
    candidate="$MANIFEST_DIR/$rel_path"
  fi
  if resolved="$(resolve_binary "$candidate")"; then
    SRC_BIN="$resolved"
    break
  fi
done

if [[ -z "$SRC_BIN" ]]; then
  echo "missing AOT binary after build; looked for: ${search_paths[*]}" >&2
  exit 1
fi

if ! BUILD_INFO="$(FUSE_AOT_BUILD_INFO=1 "$SRC_BIN" 2>/dev/null)"; then
  echo "failed to read AOT build metadata from $SRC_BIN" >&2
  exit 1
fi
if [[ "$BUILD_INFO" != *"mode="* || "$BUILD_INFO" != *"profile="* || "$BUILD_INFO" != *"target="* || "$BUILD_INFO" != *"contract="* ]]; then
  echo "unexpected AOT build metadata output from $SRC_BIN" >&2
  echo "output: $BUILD_INFO" >&2
  exit 1
fi

mkdir -p "$DIST_DIR" "$ROOT/tmp"
STAGE_DIR="$(mktemp -d "$ROOT/tmp/aot-package.XXXXXX")"
cleanup() {
  rm -rf "$STAGE_DIR"
}
trap cleanup EXIT

cp "$SRC_BIN" "$STAGE_DIR/$BIN_NAME"
chmod +x "$STAGE_DIR/$BIN_NAME"
cp "$ROOT/LICENSE" "$STAGE_DIR/LICENSE"
printf '%s\n' "$BUILD_INFO" >"$STAGE_DIR/AOT_BUILD_INFO.txt"

cat >"$STAGE_DIR/README.txt" <<README
Fuse AOT reference bundle (${PLATFORM})

Contents:
- ${BIN_NAME}
- AOT_BUILD_INFO.txt

Source fixture:
- repository fixture manifest (override via --manifest-path)

Quick start:
- chmod +x ${BIN_NAME}
- ./${BIN_NAME}
- FUSE_AOT_BUILD_INFO=1 ./${BIN_NAME}
README

rm -f "$ARCHIVE"
if [[ "$ARCHIVE" == *.zip ]]; then
  if command -v zip >/dev/null 2>&1; then
    (
      cd "$STAGE_DIR"
      zip -qr "$ARCHIVE" "$BIN_NAME" AOT_BUILD_INFO.txt LICENSE README.txt
    )
  elif command -v python3 >/dev/null 2>&1; then
    STAGE_DIR="$STAGE_DIR" ARCHIVE="$ARCHIVE" BIN_NAME="$BIN_NAME" python3 - <<'PY'
import os
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

stage = Path(os.environ["STAGE_DIR"])
archive = Path(os.environ["ARCHIVE"])
bin_name = os.environ["BIN_NAME"]
with ZipFile(archive, "w", compression=ZIP_DEFLATED) as zf:
    for rel in [bin_name, "AOT_BUILD_INFO.txt", "LICENSE", "README.txt"]:
        zf.write(stage / rel, rel)
PY
  elif command -v bsdtar >/dev/null 2>&1; then
    (
      cd "$STAGE_DIR"
      bsdtar --format zip -cf "$ARCHIVE" "$BIN_NAME" AOT_BUILD_INFO.txt LICENSE README.txt
    )
  else
    echo "missing zip archiver: install zip, python3, or bsdtar" >&2
    exit 1
  fi
else
  tar -C "$STAGE_DIR" -czf "$ARCHIVE" "$BIN_NAME" AOT_BUILD_INFO.txt LICENSE README.txt
fi

"$ROOT/scripts/verify_aot_artifact.sh" --platform "$PLATFORM" --archive "$ARCHIVE"

echo "aot artifact created: $ARCHIVE"
