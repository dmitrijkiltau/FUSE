#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT_DIR="$ROOT/scripts"
EXAMPLES_DIR="$ROOT/examples"

if [[ ! -d "$EXAMPLES_DIR" ]]; then
  echo "examples directory not found: $EXAMPLES_DIR" >&2
  exit 1
fi

status=0
while IFS= read -r -d '' file; do
  echo "checking $file"
  if ! "$SCRIPT_DIR/cargo_env.sh" cargo run -p fusec -- --check "$file"; then
    status=1
  fi
done < <(find "$EXAMPLES_DIR" -type f -name '*.fuse' -print0)

exit "$status"
