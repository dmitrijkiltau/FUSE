#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/common.sh"
ROOT="$(fuse_repo_root "${BASH_SOURCE[0]}")"

step "1/11" "Run focused LSP contract tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_contracts

step "2/11" "Run navigation/refactor integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_navigation_refactor

step "3/11" "Run signature-help integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_signature_help

step "4/11" "Run completion ranking integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_completion_rank

step "5/11" "Run member completion integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_completion_member

step "6/11" "Run code-action integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_code_actions

step "7/11" "Run workspace-incremental integration tests"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_workspace_incremental

step "8/11" "Run VS Code LSP path-resolution checks"
"$ROOT/scripts/verify_vscode_lsp_resolution.sh"

step "9/11" "Run LSP performance/reliability harness"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_perf_reliability

step "10/11" "Run end-to-end LSP UX smoke test"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test lsp_ux

step "11/11" "Run LSP latency SLO regression gate"
"$ROOT/scripts/check_lsp_latency_slo.sh"

printf "\nlsp suite checks passed\n"
