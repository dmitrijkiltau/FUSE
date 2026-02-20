#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/1" "Run LSP performance/reliability harness"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_perf_reliability

printf "\nlsp performance/reliability checks passed\n"
