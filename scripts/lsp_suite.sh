#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/10" "Run focused LSP contract tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_contracts

step "2/10" "Run navigation/refactor integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_navigation_refactor

step "3/10" "Run signature-help integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_signature_help

step "4/10" "Run completion ranking integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_completion_rank

step "5/10" "Run member completion integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_completion_member

step "6/10" "Run code-action integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_code_actions

step "7/10" "Run workspace-incremental integration tests"
"$ROOT/scripts/lsp_workspace_incremental.sh"

step "8/10" "Run VS Code LSP path-resolution checks"
"$ROOT/scripts/verify_vscode_lsp_resolution.sh"

step "9/10" "Run LSP performance/reliability harness"
"$ROOT/scripts/lsp_perf_reliability.sh"

step "10/10" "Run end-to-end LSP UX smoke test"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_ux

printf "\nlsp suite checks passed\n"
