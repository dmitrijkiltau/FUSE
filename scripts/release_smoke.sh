#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/17" "Run AST authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

step "2/17" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/17" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/17" "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

step "5/17" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "6/17" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "7/17" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "8/17" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "9/17" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "10/17" "Backend smoke run (VM)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend vm

step "11/17" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

step "12/17" "DB query-builder backend smoke run"
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend ast
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend vm
FUSE_DB_URL=sqlite::memory: "$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/db_query_builder.fuse" --backend native

step "13/17" "Native spawn error propagation smoke"
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

step "14/17" "Collect use-case benchmark metrics"
"$ROOT/scripts/use_case_bench.sh"

step "15/17" "Enforce benchmark regression thresholds"
"$ROOT/scripts/check_use_case_bench_regression.sh"

step "16/17" "Ensure VS Code extension dependencies are installed"
if [[ ! -d "$ROOT/tools/vscode/node_modules" ]]; then
  (cd "$ROOT/tools/vscode" && npm ci)
fi

step "17/17" "Build and validate VS Code VSIX package"
"$ROOT/scripts/package_vscode_extension.sh" --release

printf "\nrelease smoke checks passed\n"
