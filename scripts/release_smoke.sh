#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/15" "Run AST authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

step "2/15" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/15" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/15" "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

step "5/15" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "6/15" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "7/15" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "8/15" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "9/15" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "10/15" "Backend smoke run (VM)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend vm

step "11/15" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

step "12/15" "Collect use-case benchmark metrics"
"$ROOT/scripts/use_case_bench.sh"

step "13/15" "Enforce benchmark regression thresholds"
"$ROOT/scripts/check_use_case_bench_regression.sh"

step "14/15" "Ensure VS Code extension dependencies are installed"
if [[ ! -d "$ROOT/tools/vscode/node_modules" ]]; then
  (cd "$ROOT/tools/vscode" && npm ci)
fi

step "15/15" "Build and validate VS Code VSIX package"
"$ROOT/scripts/package_vscode_extension.sh" --release

printf "\nrelease smoke checks passed\n"
