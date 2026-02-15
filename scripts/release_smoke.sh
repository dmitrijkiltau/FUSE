#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/10" "Run AST authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

step "2/10" "Run fusec test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec

step "3/10" "Run fuse CLI test suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse

step "4/10" "Release-mode compile check (fuse CLI)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse --release

step "5/10" "Release-mode compile check (fusec binaries)"
"$ROOT/scripts/cargo_env.sh" cargo build -p fusec --release --bins

step "6/10" "Build package from clean state"
"$ROOT/scripts/fuse" build --clean

step "7/10" "Build package with warm cache"
"$ROOT/scripts/fuse" build

step "8/10" "Backend smoke run (AST)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend ast

step "9/10" "Backend smoke run (VM)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend vm

step "10/10" "Backend smoke run (native)"
"$ROOT/scripts/cargo_env.sh" cargo run -p fusec -- --run "$ROOT/examples/task_api.fuse" --backend native

printf "\nrelease smoke checks passed\n"
