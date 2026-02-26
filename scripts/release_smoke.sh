#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/23" "Run AST authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

step "2/23" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/23" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/23" "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

step "5/23" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "6/23" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "7/23" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "8/23" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "9/23" "Build package with AOT output path"
"$ROOT/scripts/fuse" build --aot

step "10/23" "Run built AOT binary directly"
"$ROOT/build/app"

step "11/23" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "12/23" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

step "13/23" "DB query-builder backend smoke run"
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend ast
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend native

step "14/23" "Native spawn error propagation smoke"
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

step "15/23" "Collect use-case benchmark metrics"
"$ROOT/scripts/use_case_bench.sh"

step "16/23" "Enforce benchmark regression thresholds"
set +e
"$ROOT/scripts/check_use_case_bench_regression.sh"
bench_status=$?
set -e
if [[ "$bench_status" -ne 0 ]]; then
  echo "benchmark regression gate failed; retrying once to filter transient host jitter"
  "$ROOT/scripts/use_case_bench.sh"
  "$ROOT/scripts/check_use_case_bench_regression.sh"
fi

step "17/23" "Collect AOT startup/throughput benchmark metrics"
"$ROOT/scripts/aot_perf_bench.sh"

step "18/23" "Enforce AOT cold-start SLO thresholds"
set +e
"$ROOT/scripts/check_aot_perf_slo.sh"
aot_perf_status=$?
set -e
if [[ "$aot_perf_status" -ne 0 ]]; then
  echo "AOT performance SLO gate failed; retrying benchmark once to filter transient host jitter"
  "$ROOT/scripts/aot_perf_bench.sh"
  "$ROOT/scripts/check_aot_perf_slo.sh"
fi

step "19/23" "Ensure VS Code extension dependencies are installed"
if [[ ! -d "$ROOT/tools/vscode/node_modules" ]]; then
  (cd "$ROOT/tools/vscode" && npm ci)
fi

step "20/23" "Build and validate VS Code VSIX package"
"$ROOT/scripts/package_vscode_extension.sh" --release

step "21/23" "Run packaging verifier regression checks"
"$ROOT/scripts/packaging_verifier_regression.sh"

step "22/23" "Build host CLI release artifact"
"$ROOT/scripts/package_cli_artifacts.sh" --release

step "23/23" "Build host AOT release artifact and checksum metadata"
"$ROOT/scripts/package_aot_artifact.sh" --release --manifest-path .
SOURCE_DATE_EPOCH="$(git -C "$ROOT" show -s --format=%ct HEAD)" "$ROOT/scripts/generate_release_checksums.sh"

printf "\nrelease smoke checks passed\n"
