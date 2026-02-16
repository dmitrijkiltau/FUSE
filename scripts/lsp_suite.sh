#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/3" "Run focused LSP contract tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_contracts

step "2/3" "Run signature-help integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_signature_help

step "3/3" "Run end-to-end LSP UX smoke test"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_ux

printf "\nlsp suite checks passed\n"
