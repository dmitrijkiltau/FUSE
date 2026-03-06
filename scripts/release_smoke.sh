#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/common.sh"
ROOT="$(fuse_repo_root "${BASH_SOURCE[0]}")"
USE_CASE_BENCH_MAX_ATTEMPTS="${FUSE_USE_CASE_BENCH_MAX_ATTEMPTS:-3}"
CLEAR_FUSE_CACHE=0

usage() {
  cat <<'USAGE'
Usage: scripts/release_smoke.sh [options]

Runs the full release smoke suite.

Options:
  --clear-fuse-cache  Remove all .fuse-cache directories under the repo before running
  -h, --help          Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clear-fuse-cache)
      CLEAR_FUSE_CACHE=1
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

if [[ "$CLEAR_FUSE_CACHE" -eq 1 ]]; then
  clear_fuse_cache_dirs "$ROOT" "release-smoke"
fi

step "1/22" "Check all example files"
"$ROOT/scripts/check_examples.sh"

step "2/22" "Run fusec test suite (includes authority/parity gates)"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/22" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/22" "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

step "5/22" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "6/22" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "7/22" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "8/22" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "9/22" "Build package with AOT output path"
"$ROOT/scripts/fuse" build --aot

step "10/22" "Run built AOT binary directly"
"$ROOT/build/app"

step "11/22" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "12/22" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

step "13/22" "DB query-builder backend smoke run"
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend ast
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend native

step "14/22" "Native spawn error propagation smoke"
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

step "15/22" "Collect use-case benchmark metrics"
"$ROOT/scripts/use_case_bench.sh"

step "16/22" "Enforce benchmark regression thresholds"
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

step "17/22" "Collect AOT startup/throughput benchmark metrics"
"$ROOT/scripts/aot_perf_bench.sh"

step "18/22" "Enforce AOT cold-start SLO thresholds"
set +e
"$ROOT/scripts/check_aot_perf_slo.sh"
aot_perf_status=$?
set -e
if [[ "$aot_perf_status" -ne 0 ]]; then
  echo "AOT performance SLO gate failed; retrying benchmark once to filter transient host jitter"
  "$ROOT/scripts/aot_perf_bench.sh"
  "$ROOT/scripts/check_aot_perf_slo.sh"
fi

step "19/22" "Ensure VS Code extension dependencies are installed"
if [[ ! -d "$ROOT/tools/vscode/node_modules" ]]; then
  (cd "$ROOT/tools/vscode" && npm ci)
fi

step "20/22" "Build and validate VS Code VSIX package"
"$ROOT/scripts/package_vscode_extension.sh" --release

step "21/22" "Run packaging verifier regression checks"
"$ROOT/scripts/packaging_verifier_regression.sh"

step "22/22" "Build host CLI release artifact and checksum metadata"
"$ROOT/scripts/package_cli_artifacts.sh" --release
"$ROOT/scripts/package_aot_artifact.sh" --release --manifest-path .
SOURCE_DATE_EPOCH="$(git -C "$ROOT" show -s --format=%ct HEAD)" "$ROOT/scripts/generate_release_checksums.sh"

printf "\nrelease smoke checks passed\n"
