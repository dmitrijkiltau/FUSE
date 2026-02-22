#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VSCODE_DIR="$ROOT/tools/vscode"
DIST_DIR="$ROOT/dist"

RELEASE=0
SKIP_BUILD=0
SKIP_VERIFY=0
PLATFORM=""

usage() {
  cat <<'EOF'
Usage: scripts/package_vscode_extension.sh [options]

Options:
  --platform <name>   Target platform directory (default: host platform, e.g. linux-x64)
  --release           Build dist binaries in release mode
  --skip-build        Skip scripts/build_dist.sh
  --skip-verify       Skip scripts/verify_vscode_lsp_resolution.sh
  -h, --help          Show this help
EOF
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
    --release)
      RELEASE=1
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-verify)
      SKIP_VERIFY=1
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

if [[ "$PLATFORM" == windows-* ]]; then
  BIN_NAME="fuse-lsp.exe"
else
  BIN_NAME="fuse-lsp"
fi

BUILD_ARGS=()
if [[ "$RELEASE" -eq 1 ]]; then
  BUILD_ARGS+=(--release)
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "[1/6] Building dist binaries..."
  "$ROOT/scripts/build_dist.sh" "${BUILD_ARGS[@]}"
else
  echo "[1/6] Skipping build_dist (requested)."
fi

SRC_BIN=""
if [[ -x "$DIST_DIR/$BIN_NAME" ]]; then
  SRC_BIN="$DIST_DIR/$BIN_NAME"
elif [[ "$BIN_NAME" == "fuse-lsp.exe" && -x "$DIST_DIR/fuse-lsp" ]]; then
  # Allow manually staged windows binaries named without .exe.
  SRC_BIN="$DIST_DIR/fuse-lsp"
elif [[ "$BIN_NAME" == "fuse-lsp" && -x "$DIST_DIR/fuse-lsp.exe" ]]; then
  # Allow manually staged unix binaries named with .exe.
  SRC_BIN="$DIST_DIR/fuse-lsp.exe"
fi

if [[ -z "$SRC_BIN" ]]; then
  echo "missing dist lsp binary; expected $DIST_DIR/$BIN_NAME" >&2
  exit 1
fi

DEST_BIN_DIR="$VSCODE_DIR/bin/$PLATFORM"
DEST_BIN="$DEST_BIN_DIR/$BIN_NAME"
mkdir -p "$DEST_BIN_DIR"
cp "$SRC_BIN" "$DEST_BIN"
chmod +x "$DEST_BIN"
echo "[2/6] Bundled lsp binary: $DEST_BIN"

if [[ "$SKIP_VERIFY" -eq 0 ]]; then
  echo "[3/6] Verifying VS Code LSP path resolution..."
  "$ROOT/scripts/verify_vscode_lsp_resolution.sh"
else
  echo "[3/6] Skipping resolver verification (requested)."
fi

echo "[4/6] Staging extension payload..."
mkdir -p "$ROOT/tmp"
STAGE_DIR="$(mktemp -d "$ROOT/tmp/vscode-package.XXXXXX")"
cleanup() {
  rm -rf "$STAGE_DIR"
}
trap cleanup EXIT

OUT_DIR="$STAGE_DIR/extension"
mkdir -p "$OUT_DIR"

if [[ ! -d "$VSCODE_DIR/node_modules" ]]; then
  echo "missing $VSCODE_DIR/node_modules; run 'cd tools/vscode && npm install'" >&2
  exit 1
fi

cp "$VSCODE_DIR/package.json" "$OUT_DIR/package.json"
cp "$VSCODE_DIR/package-lock.json" "$OUT_DIR/package-lock.json"
cp "$VSCODE_DIR/extension.js" "$OUT_DIR/extension.js"
cp "$VSCODE_DIR/lsp-path.js" "$OUT_DIR/lsp-path.js"
cp "$VSCODE_DIR/language-configuration.json" "$OUT_DIR/language-configuration.json"
cp "$VSCODE_DIR/README.md" "$OUT_DIR/README.md"
cp "$VSCODE_DIR/CHANGELOG.md" "$OUT_DIR/CHANGELOG.md"
cp "$ROOT/LICENSE" "$OUT_DIR/LICENSE"
cp -R "$VSCODE_DIR/syntaxes" "$OUT_DIR/syntaxes"
cp -R "$VSCODE_DIR/node_modules" "$OUT_DIR/node_modules"
mkdir -p "$OUT_DIR/bin/$PLATFORM"
cp "$DEST_BIN" "$OUT_DIR/bin/$PLATFORM/$BIN_NAME"
chmod +x "$OUT_DIR/bin/$PLATFORM/$BIN_NAME"

xml_escape() {
  printf '%s' "$1" \
    | sed -e 's/&/\&amp;/g' \
          -e 's/</\&lt;/g' \
          -e 's/>/\&gt;/g' \
          -e "s/'/\&apos;/g" \
          -e 's/"/\&quot;/g'
}

PKG_JSON="$VSCODE_DIR/package.json"
EXT_NAME="$(node -pe 'const p=require(process.argv[1]); p.name' "$PKG_JSON")"
EXT_PUBLISHER="$(node -pe 'const p=require(process.argv[1]); p.publisher' "$PKG_JSON")"
EXT_VERSION="$(node -pe 'const p=require(process.argv[1]); p.version' "$PKG_JSON")"
EXT_DISPLAY_NAME="$(node -pe 'const p=require(process.argv[1]); p.displayName || p.name' "$PKG_JSON")"
EXT_DESCRIPTION="$(node -pe 'const p=require(process.argv[1]); p.description || ""' "$PKG_JSON")"
EXT_ENGINE="$(node -pe 'const p=require(process.argv[1]); (p.engines && p.engines.vscode) || "*"' "$PKG_JSON")"
EXT_ID="$(xml_escape "${EXT_PUBLISHER}.${EXT_NAME}")"
EXT_PUBLISHER_ESC="$(xml_escape "$EXT_PUBLISHER")"
EXT_VERSION_ESC="$(xml_escape "$EXT_VERSION")"
EXT_DISPLAY_ESC="$(xml_escape "$EXT_DISPLAY_NAME")"
EXT_DESC_ESC="$(xml_escape "$EXT_DESCRIPTION")"
EXT_ENGINE_ESC="$(xml_escape "$EXT_ENGINE")"

cat >"$STAGE_DIR/[Content_Types].xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="json" ContentType="application/json" />
  <Default Extension="vsixmanifest" ContentType="text/xml" />
  <Default Extension="xml" ContentType="text/xml" />
  <Default Extension="md" ContentType="text/markdown" />
  <Default Extension="js" ContentType="application/javascript" />
  <Default Extension="css" ContentType="text/css" />
  <Default Extension="svg" ContentType="image/svg+xml" />
  <Default Extension="txt" ContentType="text/plain" />
</Types>
EOF

cat >"$STAGE_DIR/extension.vsixmanifest" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
  <Metadata>
    <Identity Language="en-US" Id="$EXT_ID" Version="$EXT_VERSION_ESC" Publisher="$EXT_PUBLISHER_ESC" />
    <DisplayName>$EXT_DISPLAY_ESC</DisplayName>
    <Description xml:space="preserve">$EXT_DESC_ESC</Description>
    <Tags>fuse language lsp syntax-highlighting</Tags>
    <Categories>Programming Languages</Categories>
    <GalleryFlags>Public</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="$EXT_ENGINE_ESC" />
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.Details" Path="extension/README.md" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.Changelog" Path="extension/CHANGELOG.md" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.License" Path="extension/LICENSE" />
  </Assets>
</PackageManifest>
EOF

mkdir -p "$DIST_DIR"
ARCHIVE="$DIST_DIR/fuse-vscode-${PLATFORM}.vsix"
(
  cd "$STAGE_DIR"
  rm -f "$ARCHIVE"
  if command -v zip >/dev/null 2>&1; then
    zip -qr "$ARCHIVE" "[Content_Types].xml" extension.vsixmanifest extension
  elif command -v python3 >/dev/null 2>&1; then
    STAGE_DIR="$STAGE_DIR" ARCHIVE="$ARCHIVE" python3 - <<'PY'
import os
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

stage = Path(os.environ["STAGE_DIR"])
archive = Path(os.environ["ARCHIVE"])
with ZipFile(archive, "w", compression=ZIP_DEFLATED) as zf:
    for rel in ["[Content_Types].xml", "extension.vsixmanifest"]:
        zf.write(stage / rel, rel)
    for path in (stage / "extension").rglob("*"):
        if path.is_file():
            zf.write(path, path.relative_to(stage).as_posix())
PY
  elif command -v bsdtar >/dev/null 2>&1; then
    bsdtar --format zip -cf "$ARCHIVE" "[Content_Types].xml" extension.vsixmanifest extension
  else
    echo "missing archiver: install zip, bsdtar, or python3" >&2
    exit 1
  fi
)
echo "[5/6] Created VSIX package: $ARCHIVE"

echo "[6/6] Verifying VSIX contents..."
"$ROOT/scripts/verify_vscode_vsix.sh" --platform "$PLATFORM" --vsix "$ARCHIVE"

echo "done"
