#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/21" "Run AST authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

step "2/21" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/21" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/21" "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

step "5/21" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "6/21" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "7/21" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "8/21" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "9/21" "Build package with AOT output path"
"$ROOT/scripts/fuse" build --aot

step "10/21" "Run built AOT binary directly"
"$ROOT/build/app"

step "11/21" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "12/21" "Backend smoke run (VM)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend vm

step "13/21" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

step "14/21" "DB query-builder backend smoke run"
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend ast
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend vm
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend native

step "15/21" "Native spawn error propagation smoke"
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

step "16/21" "Collect use-case benchmark metrics"
"$ROOT/scripts/use_case_bench.sh"

step "17/21" "Enforce benchmark regression thresholds"
set +e
"$ROOT/scripts/check_use_case_bench_regression.sh"
bench_status=$?
set -e
if [[ "$bench_status" -ne 0 ]]; then
  echo "benchmark regression gate failed; retrying once to filter transient host jitter"
  "$ROOT/scripts/use_case_bench.sh"
  "$ROOT/scripts/check_use_case_bench_regression.sh"
fi

step "18/21" "Ensure VS Code extension dependencies are installed"
if [[ ! -d "$ROOT/tools/vscode/node_modules" ]]; then
  (cd "$ROOT/tools/vscode" && npm ci)
fi

step "19/21" "Build and validate VS Code VSIX package"
"$ROOT/scripts/package_vscode_extension.sh" --release

step "20/21" "Run packaging verifier regression checks"
"$ROOT/scripts/packaging_verifier_regression.sh"

step "21/21" "Build host CLI release artifact and checksum metadata"
"$ROOT/scripts/package_cli_artifacts.sh" --release
"$ROOT/scripts/generate_release_checksums.sh"

printf "\nrelease smoke checks passed\n"
