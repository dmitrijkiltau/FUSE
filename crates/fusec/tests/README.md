# fusec test suite structure

This directory contains both semantic-contract tests and implementation smoke tests.

## Semantic-contract tests

Use `scripts/semantic_suite.sh` to run the canonical semantic suite.
These tests define language/runtime behavior expected to remain stable:

- `parser_fixtures.rs` - parser + canonical AST shape fixtures
- `frontend_canonicalize.rs` - frontend canonicalization guarantees
- `sema_golden.rs` - type-checking and semantic diagnostics
- `ast_authority_parity.rs` - AST authority and backend parity rules
- `parity_vm_ast.rs` - AST/VM/native behavior parity scenarios
- `module_function_scope.rs` - module-scoped symbol resolution/dispatch semantics
- `config_runtime.rs` - config/env/CLI boundary semantics
- `bytes_runtime.rs` - bytes encode/decode boundary semantics
- `refinement_runtime.rs` - refinement validation semantics
- `result_decode_runtime.rs` - tagged `Result<T,E>` decode semantics
- `db_pool_runtime.rs` - DB pool config/validation semantics
- `openapi_result_schema.rs` - OpenAPI schema semantics for tagged `Result`

## Feature-to-test matrix

| Semantic area | Coverage tests | Behavior locked by tests |
| --- | --- | --- |
| Type rules | `parser_fixtures.rs`, `sema_golden.rs`, `frontend_canonicalize.rs`, `refinement_runtime.rs` | Canonical AST shape, type checking diagnostics, refined type constraints, HTML canonicalization requirements |
| Boundary contracts | `config_runtime.rs`, `bytes_runtime.rs`, `result_decode_runtime.rs`, `db_pool_runtime.rs`, `openapi_result_schema.rs` | Config/env/CLI binding, bytes/base64 behavior, tagged `Result<T,E>` JSON decode, DB pool config rules, OpenAPI schema mapping |
| Error mapping | `sema_golden.rs`, `ast_authority_parity.rs`, `parity_vm_ast.rs`, `golden_outputs.rs`, `native_error_smoke.rs` | Compile-time semantic diagnostics, runtime error JSON/status mapping, backend parity for failure behavior |
| Dispatch and symbol resolution | `module_function_scope.rs`, `parity_vm_ast.rs`, `ast_authority_parity.rs` | Module-scoped symbol resolution, local/import shadowing rules, cross-backend call dispatch equivalence |

Command gates:

- `scripts/semantic_suite.sh` is the semantic contract gate.
- `scripts/authority_parity.sh` is the backend semantic-authority parity gate.
- `scripts/release_smoke.sh` includes authority parity inside release checks.

## Smoke and implementation tests

The remaining files are backend smoke, performance-smoke, UX, ABI, and integration checks.
They are valuable for regressions but are not the primary semantic contract gate.

LSP-specific gates:

- `scripts/lsp_suite.sh` runs:
  - focused LSP contract tests (`lsp_contracts.rs`)
  - navigation/refactor safety coverage (`lsp_navigation_refactor.rs`)
  - signature help coverage (`lsp_signature_help.rs`)
  - completion ranking coverage (`lsp_completion_rank.rs`)
  - member-chain/import-aware completion coverage (`lsp_completion_member.rs`)
  - code-action quickfix/organize coverage (`lsp_code_actions.rs`)
  - VS Code extension LSP binary path-resolution checks (`scripts/verify_vscode_lsp_resolution.sh`)
  - cancellation burst + large-workspace responsiveness budgets (`lsp_perf_reliability.rs`)
  - end-to-end UX smoke (`lsp_ux.rs`)
