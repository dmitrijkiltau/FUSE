#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/6" "Run focused LSP contract tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_contracts

step "2/6" "Run navigation/refactor integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_navigation_refactor

step "3/6" "Run signature-help integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_signature_help

step "4/6" "Run completion ranking integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_completion_rank

step "5/6" "Run member completion integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_completion_member

step "6/6" "Run end-to-end LSP UX smoke test"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_ux

printf "\nlsp suite checks passed\n"
