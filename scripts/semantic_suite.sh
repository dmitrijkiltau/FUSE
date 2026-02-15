#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() {
  printf "\n[%s] %s\n" "$1" "$2"
}

step "1/10" "Parser fixture semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test parser_fixtures

step "2/10" "Frontend canonicalization semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test frontend_canonicalize

step "3/10" "Semantic analysis golden suite"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test sema_golden

step "4/10" "Config boundary semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test config_runtime

step "5/10" "Bytes boundary semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test bytes_runtime

step "6/10" "Refinement semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test refinement_runtime

step "7/10" "Tagged Result decode semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test result_decode_runtime

step "8/10" "DB pool boundary semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test db_pool_runtime

step "9/10" "OpenAPI result schema semantics"
"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test openapi_result_schema

step "10/10" "Cross-backend semantic authority/parity gates"
"$ROOT/scripts/authority_parity.sh"

printf "\nsemantic suite checks passed\n"
