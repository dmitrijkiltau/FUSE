#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/7" "Canonicalization gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test frontend_canonicalize

step "2/7" "AST authority parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test ast_authority_parity

step "3/7" "AST/native parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test parity_ast_native

step "4/7" "Module scope parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test module_function_scope

step "5/7" "Runtime decode parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test result_decode_runtime

step "6/7" "AST/native/AOT parity lock matrix gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse --test aot_parity_lock

step "7/7" "AOT panic taxonomy parity gate"
"$ROOT/scripts/cargo_env.sh" cargo test -p fuse --test project_cli \
  build_aot_runtime_panic_uses_exit_101_and_panic_fatal_envelope

printf "\nauthority parity checks passed\n"
