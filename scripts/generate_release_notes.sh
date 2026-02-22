#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG="$ROOT/CHANGELOG.md"
OUTPUT="$ROOT/dist/RELEASE_NOTES.md"
VERSION=""
TAG=""

usage() {
  cat <<'EOF'
Usage: scripts/generate_release_notes.sh [options]

Options:
  --version <x.y.z>    Release version to extract from CHANGELOG (default: latest in CHANGELOG)
  --tag <name>         Release tag name (default: v<version>)
  --changelog <path>   Changelog path (default: CHANGELOG.md)
  --output <path>      Output markdown file (default: dist/RELEASE_NOTES.md)
  -h, --help           Show this help
EOF
}

abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      if [[ -z "$VERSION" ]]; then
        echo "--version requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --tag)
      TAG="${2:-}"
      if [[ -z "$TAG" ]]; then
        echo "--tag requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --changelog)
      if [[ -z "${2:-}" ]]; then
        echo "--changelog requires a value" >&2
        exit 1
      fi
      CHANGELOG="$(abspath "$2")"
      shift 2
      ;;
    --output)
      if [[ -z "${2:-}" ]]; then
        echo "--output requires a value" >&2
        exit 1
      fi
      OUTPUT="$(abspath "$2")"
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

if [[ ! -f "$CHANGELOG" ]]; then
  echo "missing changelog: $CHANGELOG" >&2
  exit 1
fi

if [[ -z "$VERSION" ]]; then
  VERSION="$(sed -n 's/^## \[\([0-9][0-9.]*\)\].*/\1/p' "$CHANGELOG" | head -n1)"
fi

if [[ -z "$VERSION" ]]; then
  echo "could not infer release version from $CHANGELOG" >&2
  exit 1
fi

if [[ -z "$TAG" ]]; then
  TAG="v$VERSION"
fi

mkdir -p "$ROOT/tmp" "$(dirname "$OUTPUT")"
SECTION_FILE="$(mktemp "$ROOT/tmp/release-notes-section.XXXXXX")"
cleanup() {
  rm -f "$SECTION_FILE"
}
trap cleanup EXIT

awk -v version="$VERSION" '
  $0 ~ "^## \\[" version "\\]" {
    in_section = 1
  }
  in_section {
    if ($0 ~ "^## \\[" && $0 !~ "^## \\[" version "\\]") {
      exit
    }
    print
  }
' "$CHANGELOG" >"$SECTION_FILE"

if ! grep -q "^## \[$VERSION\]" "$SECTION_FILE"; then
  echo "missing changelog section for version $VERSION in $CHANGELOG" >&2
  exit 1
fi

cat >"$OUTPUT" <<EOF
# FUSE $TAG

This release note draft was auto-generated from \`$(basename "$CHANGELOG")\`.

$(cat "$SECTION_FILE")

## Artifacts

- \`fuse-cli-<platform>.tar.gz|.zip\`
- \`fuse-aot-<platform>.tar.gz|.zip\`
- \`fuse-vscode-<platform>.vsix\`
- \`SHA256SUMS\`
- \`release-artifacts.json\`

## Verification

\`\`\`bash
sha256sum -c SHA256SUMS
\`\`\`
EOF

echo "generated release notes: $OUTPUT"
