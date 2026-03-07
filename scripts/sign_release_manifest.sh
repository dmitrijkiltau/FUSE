#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

INPUT=""
SIGNATURE_OUTPUT=""
CERTIFICATE_OUTPUT=""

usage() {
  cat <<'USAGE'
Usage: scripts/sign_release_manifest.sh [options]

Options:
  --input <path>         File to sign
  --signature <path>     Detached signature output path (default: <input>.sig)
  --certificate <path>   Fulcio certificate output path (default: <input>.pem)
  -h, --help             Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --input)
      INPUT="${2:-}"
      if [[ -z "$INPUT" ]]; then
        echo "--input requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --signature)
      SIGNATURE_OUTPUT="${2:-}"
      if [[ -z "$SIGNATURE_OUTPUT" ]]; then
        echo "--signature requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --certificate)
      CERTIFICATE_OUTPUT="${2:-}"
      if [[ -z "$CERTIFICATE_OUTPUT" ]]; then
        echo "--certificate requires a value" >&2
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

if [[ -z "$INPUT" ]]; then
  echo "--input is required" >&2
  usage
  exit 1
fi

INPUT="$(abspath "$INPUT")"
if [[ ! -f "$INPUT" ]]; then
  echo "input file not found: $INPUT" >&2
  exit 1
fi

if ! command -v cosign >/dev/null 2>&1; then
  echo "missing cosign in PATH" >&2
  exit 1
fi

if [[ -z "$SIGNATURE_OUTPUT" ]]; then
  SIGNATURE_OUTPUT="${INPUT}.sig"
else
  SIGNATURE_OUTPUT="$(abspath "$SIGNATURE_OUTPUT")"
fi

if [[ -z "$CERTIFICATE_OUTPUT" ]]; then
  CERTIFICATE_OUTPUT="${INPUT}.pem"
else
  CERTIFICATE_OUTPUT="$(abspath "$CERTIFICATE_OUTPUT")"
fi

mkdir -p "$(dirname "$SIGNATURE_OUTPUT")" "$(dirname "$CERTIFICATE_OUTPUT")"

cosign sign-blob \
  --yes \
  --output-signature "$SIGNATURE_OUTPUT" \
  --output-certificate "$CERTIFICATE_OUTPUT" \
  "$INPUT"

echo "wrote signature: $SIGNATURE_OUTPUT"
echo "wrote certificate: $CERTIFICATE_OUTPUT"
