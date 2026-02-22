#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ITERATIONS=2

usage() {
  cat <<'USAGE'
Usage: scripts/reliability_repeat.sh [options]

Options:
  --iterations <n>  Number of repeated reliability iterations (default: 2)
  -h, --help        Show this help

Notes:
  Benchmark collection uses scripts/use_case_bench.sh --median-of-3.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --iterations)
      ITERATIONS="${2:-}"
      if [[ -z "$ITERATIONS" ]]; then
        echo "--iterations requires a value" >&2
        exit 1
      fi
      if ! [[ "$ITERATIONS" =~ ^[0-9]+$ ]] || [[ "$ITERATIONS" -lt 1 ]]; then
        echo "--iterations must be a positive integer" >&2
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

step() {
  printf "\n[repeat %s/%s] %s\n" "$1" "$ITERATIONS" "$2"
}

run_benchmark_gate_with_retry() {
  local iteration="$1"
  set +e
  "$ROOT/scripts/use_case_bench.sh" --median-of-3
  local bench_status=$?
  set -e
  if [[ "$bench_status" -ne 0 ]]; then
    echo "benchmark collection failed on iteration $iteration; retrying once"
    "$ROOT/scripts/use_case_bench.sh" --median-of-3
  fi

  set +e
  "$ROOT/scripts/check_use_case_bench_regression.sh"
  local gate_status=$?
  set -e
  if [[ "$gate_status" -ne 0 ]]; then
    echo "benchmark regression gate failed on iteration $iteration; retrying once"
    "$ROOT/scripts/use_case_bench.sh" --median-of-3
    "$ROOT/scripts/check_use_case_bench_regression.sh"
  fi
}

for ((i = 1; i <= ITERATIONS; i++)); do
  step "$i" "authority parity"
  "$ROOT/scripts/authority_parity.sh"

  step "$i" "lsp suite"
  "$ROOT/scripts/lsp_suite.sh"

  step "$i" "benchmark gate path"
  run_benchmark_gate_with_retry "$i"
done

printf "\nreliability repeat checks passed (%s iteration(s))\n" "$ITERATIONS"
