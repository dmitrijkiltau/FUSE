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
done < <(
  find "$EXAMPLES_DIR" -type f -name '*.fuse' \
    ! -path "$EXAMPLES_DIR/reference-service/*" \
    ! -path "$EXAMPLES_DIR/strict_arch_demo/*" \
    ! -path "$EXAMPLES_DIR/dep_import/*" \
    -print0
)

check_package_example() {
  local label="$1"
  shift
  echo "checking package $label"
  if ! "$SCRIPT_DIR/fuse" check "$@"; then
    status=1
  fi
}

check_package_example "examples/reference-service" --manifest-path "$EXAMPLES_DIR/reference-service"
check_package_example "examples/strict_arch_demo (--strict-architecture)" --manifest-path "$EXAMPLES_DIR/strict_arch_demo" --strict-architecture
check_package_example "examples/dep_import" --manifest-path "$EXAMPLES_DIR/dep_import"

exit "$status"
