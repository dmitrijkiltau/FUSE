#!/usr/bin/env bash
# check_lsp_latency_slo.sh — LSP latency regression gate
#
# Runs the lsp_perf_reliability test suite and exits non-zero if any latency
# budget is exceeded.  Intended for use in CI and release preflight checks.
#
# Latency budgets enforced by the tests (50-file workspace):
#   diagnostics incremental : ≤ 500 ms
#   completion warm         : ≤ 200 ms
#   workspace symbol search : ≤ 300 ms
#   progressive first diag  : ≤ 500 ms
#   edit burst drain        : ≤ 5 000 ms
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

step "1/1" "Run LSP latency SLO harness (lsp_perf_reliability)"
"$ROOT/scripts/cargo_env.sh" cargo test \
    -p fusec \
    --test lsp_perf_reliability \
    -- --nocapture 2>&1

printf "\nlsp latency SLO checks passed\n"
