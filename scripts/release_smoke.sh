#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/common.sh"
ROOT="$(fuse_repo_root "${BASH_SOURCE[0]}")"
USE_CASE_BENCH_MAX_ATTEMPTS="${FUSE_USE_CASE_BENCH_MAX_ATTEMPTS:-3}"
CLEAR_FUSE_CACHE=0
SKIP_BENCH=0

TOTAL_STEPS=22
STEP_INDEX=0

usage() {
  cat <<'USAGE'
Usage: scripts/release_smoke.sh [options]

Runs the full release smoke suite.

Options:
  --clear-fuse-cache  Remove all .fuse-cache directories under the repo before running
  --skip-bench        Skip benchmark collection and perf/SLO enforcement steps
  -h, --help          Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clear-fuse-cache)
      CLEAR_FUSE_CACHE=1
      shift
      ;;
    --skip-bench)
      SKIP_BENCH=1
      shift
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

if ! [[ "$USE_CASE_BENCH_MAX_ATTEMPTS" =~ ^[1-9][0-9]*$ ]]; then
  echo "invalid FUSE_USE_CASE_BENCH_MAX_ATTEMPTS: $USE_CASE_BENCH_MAX_ATTEMPTS" >&2
  exit 1
fi

if [[ "$SKIP_BENCH" -eq 1 ]]; then
  TOTAL_STEPS=18
fi

if [[ "$CLEAR_FUSE_CACHE" -eq 1 ]]; then
  clear_fuse_cache_dirs "$ROOT" "release-smoke"
fi

run_step() {
  STEP_INDEX=$((STEP_INDEX + 1))
  step "${STEP_INDEX}/${TOTAL_STEPS}" "$1"
}

run_step "Check all example files"
"$ROOT/scripts/check_examples.sh"

run_step "Run fusec test suite (includes authority/parity gates)"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

run_step "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

run_step "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

run_step "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

run_step "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

run_step "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

run_step "Build package with warm cache"
"$ROOT/scripts/fuse" build

run_step "Build package with AOT output path"
"$ROOT/scripts/fuse" build --aot

run_step "Run built AOT binary directly"
"$ROOT/build/app"

run_step "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

run_step "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

run_step "DB query-builder backend smoke run"
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend ast
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend native

run_step "Native spawn error propagation smoke"
set +e
native_spawn_output="$("$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/spawn_error.fuse" --backend native 2>&1)"
native_spawn_status=$?
set -e
if [[ "$native_spawn_status" -eq 0 ]]; then
  echo "expected native spawn error smoke to fail"
  exit 1
fi
if [[ "$native_spawn_output" != *"assert failed: boom"* ]]; then
  echo "unexpected native spawn error output:"
  echo "$native_spawn_output"
  exit 1
fi

if [[ "$SKIP_BENCH" -eq 1 ]]; then
  printf "\n[release-smoke] benchmark and perf/SLO steps skipped (--skip-bench)\n"
else
  run_step "Collect use-case benchmark metrics"
  "$ROOT/scripts/use_case_bench.sh"

  run_step "Enforce benchmark regression thresholds"
  bench_attempt=1
  while true; do
    set +e
    "$ROOT/scripts/check_use_case_bench_regression.sh"
    bench_status=$?
    set -e
    if [[ "$bench_status" -eq 0 ]]; then
      break
    fi
    if (( bench_attempt >= USE_CASE_BENCH_MAX_ATTEMPTS )); then
      exit "$bench_status"
    fi
    bench_attempt=$((bench_attempt + 1))
    echo "benchmark regression gate failed; retrying (${bench_attempt}/${USE_CASE_BENCH_MAX_ATTEMPTS}) to filter transient host jitter"
    "$ROOT/scripts/use_case_bench.sh"
  done

  run_step "Collect AOT startup/throughput benchmark metrics"
  "$ROOT/scripts/aot_perf_bench.sh"

  run_step "Enforce AOT cold-start SLO thresholds"
  set +e
  "$ROOT/scripts/check_aot_perf_slo.sh"
  aot_perf_status=$?
  set -e
  if [[ "$aot_perf_status" -ne 0 ]]; then
    echo "AOT performance SLO gate failed; retrying benchmark once to filter transient host jitter"
    "$ROOT/scripts/aot_perf_bench.sh"
    "$ROOT/scripts/check_aot_perf_slo.sh"
  fi
fi

run_step "Ensure VS Code extension dependencies are installed"
if [[ ! -d "$ROOT/tools/vscode/node_modules" ]]; then
  (cd "$ROOT/tools/vscode" && npm ci)
fi

run_step "Build and validate VS Code VSIX package"
"$ROOT/scripts/package_vscode_extension.sh" --release

run_step "Run packaging verifier regression checks"
"$ROOT/scripts/packaging_verifier_regression.sh"

run_step "Build host CLI release artifact and checksum metadata"
"$ROOT/scripts/package_cli_artifacts.sh" --release
"$ROOT/scripts/package_aot_artifact.sh" --release --manifest-path .
SOURCE_DATE_EPOCH="$(git -C "$ROOT" show -s --format=%ct HEAD)" "$ROOT/scripts/generate_release_checksums.sh"

printf "\nrelease smoke checks passed\n"
