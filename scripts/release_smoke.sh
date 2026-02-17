#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/11" "Run AST authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

step "2/11" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/11" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/11" "Run LSP contract/perf suite"
"$ROOT/scripts/lsp_suite.sh"

step "5/11" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "6/11" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "7/11" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "8/11" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "9/11" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "10/11" "Backend smoke run (VM)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend vm

step "11/11" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

printf "\nrelease smoke checks passed\n"
