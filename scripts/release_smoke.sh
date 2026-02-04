#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/5" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "2/5" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "3/5" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "4/5" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "5/5" "Backend smoke run (VM)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend vm

printf "\nrelease smoke checks passed\n"
