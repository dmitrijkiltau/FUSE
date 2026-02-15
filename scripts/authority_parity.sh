#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/5" "Canonicalization gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test frontend_canonicalize

step "2/5" "AST authority parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test ast_authority_parity

step "3/5" "AST/VM/native parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test parity_vm_ast

step "4/5" "Module scope parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test module_function_scope

step "5/5" "Runtime decode parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test result_decode_runtime

printf "\nauthority parity checks passed\n"
