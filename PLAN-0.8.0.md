# FUSE 0.8.0 Release Plan

Target: Q2 2026

## Theme: Developer ergonomics and runtime depth

v0.7.0 sealed AOT production posture and parity gates. The language contract,
build surface, and deployment story are stable. v0.8.0 shifts focus inward:
deeper runtime capabilities, better developer experience across multi-file
projects, and closing the remaining gaps between what FUSE declares and what it
delivers at runtime.

This release is **non-breaking**. Programs valid on 0.7.0 remain valid on 0.8.x.

---

## Current state summary (as of 0.7.0)

### What is solid

- Two-backend model (AST / native) with strict parity gates
- AOT production binary surface with startup SLOs, fatal envelopes, graceful shutdown
- Structured concurrency with compile-time lifetime enforcement
- Deterministic `transaction:` blocks with static restriction analysis
- Module capability system (`requires db|crypto|network|time`) with cross-module leakage checks
- Typed error domains on all fallible boundaries
- Strict architecture mode (`--strict-architecture`)
- Full-featured LSP (diagnostics, completion, rename, code actions, semantic tokens, incremental relink)
- Dependency resolver with lockfile, `dep:`/`root:` imports, transitive conflict detection
- Release artifact pipeline for Linux/macOS/Windows (CLI + AOT + VSIX + container image)
- All 7 RFCs implemented and closed

### Where gaps remain

- `time` and `crypto` are compile-time capability placeholders with no runtime API
- IR lowering has 3 "not supported in IR yet" code paths for edge-case expression forms
- LSP server is a 6,892-line monolith (`fuse-lsp.rs`)
- No standalone examples for `test`, `transaction:`, `requires`, `--strict-architecture`, or refinement constraints
- `json.encode`/`json.decode` just added to spec but not highlighted in examples
- Config env override does not support user-defined types or `Result<T,E>`
- No `dep:` usage demonstrated outside CI tests

---

## Milestones

### Milestone 1 — `time` and `crypto` runtime APIs

**Goal:** Deliver the two capability-gated runtime subsystems that are currently
compile-time stubs so that `requires time` and `requires crypto` unlock real
functionality.

**Scope:**

Time API:

- `time.now() -> Int` — Unix epoch milliseconds
- `time.format(epoch: Int, fmt: String) -> String` — strftime-style formatting
- `time.parse(text: String, fmt: String) -> Int!Error` — parse to epoch
- `time.sleep(ms: Int)` — blocking pause (rejected inside `spawn` blocks per existing static restrictions)

Crypto API:

- `crypto.hash(algo: String, data: Bytes) -> Bytes` — SHA-256, SHA-512
- `crypto.hmac(algo: String, key: Bytes, data: Bytes) -> Bytes`
- `crypto.random_bytes(n: Int) -> Bytes`
- `crypto.constant_time_eq(a: Bytes, b: Bytes) -> Bool`

**Deliverables:**

- [x] Runtime implementation in AST interpreter
- [x] Runtime implementation in native backend
- [x] AST/native parity tests for all new builtins
- [x] IR lowering for `time.*` and `crypto.*` call targets
- [x] `spec/runtime.md` updated with full API documentation
- [x] `spec/fls.md` capability section updated (remove placeholder notes)
- [x] Reference.md regenerated
- [x] New RFC if scope expands beyond this list (not needed; scope unchanged)

**Progress update (2026-03-01):**

- Implemented: `time.now`, `time.format`, `time.parse`, `time.sleep`
- Implemented: `crypto.hash`, `crypto.hmac`, `crypto.random_bytes`, `crypto.constant_time_eq`
- Added spawn restriction coverage for `time.sleep` in sema golden tests
- Added LSP receiver/completion/signature surface for `time` and `crypto`
- Milestone 1 complete: `spec/fls.md` capability text confirmed, `authority_parity.sh` and `semantic_suite.sh` green

**Exit criteria:** Met on 2026-03-01 (`authority_parity.sh` and `semantic_suite.sh` green with new builtins).

---

### Milestone 2 — Native backend IR lowering completeness

**Goal:** Eliminate the remaining "not supported in IR yet" code paths so that
every valid FUSE program compiles through the native backend without fallback
errors.

**Scope:**

- Audit all `"not supported in IR yet"` paths in `crates/fusec/src/ir/lower.rs`
- Implement IR lowering for the 3 remaining unsupported call expression patterns
- Add targeted test fixtures covering each previously-unsupported form
- Confirm AOT builds succeed for programs exercising these patterns

**Deliverables:**

- [x] Zero "not supported in IR yet" paths remain in `ir/lower.rs`
- [x] Parser fixture tests added for each resolved pattern
- [x] AOT SLO gate passes (`check_aot_perf_slo.sh`)
- [x] No regression in `use_case_bench.sh` metrics

**Progress update (2026-03-01):**

- Replaced the 3 remaining placeholder call-target lowering branches with deterministic IR lowering that preserves evaluation order and emits stable runtime `call target is not callable`
- Added parser fixture coverage for the 3 resolved call-target expression forms (`index`, `optional-index`, `optional-member` call targets)
- Added targeted IR lowering regression tests (`crates/fusec/tests/ir_lower_call_targets.rs`)
- Validation gates run green:
  - `scripts/authority_parity.sh`
  - `cargo test -p fusec --test parser_fixtures`
  - `cargo test -p fusec --test ir_lower_call_targets`
  - `cargo test -p fusec --test parity_ast_native`
  - `scripts/check_aot_perf_slo.sh`
  - `scripts/use_case_bench.sh --median-of-3`
  - `scripts/check_use_case_bench_regression.sh`

**Exit criteria:** Met on 2026-03-01 (`grep -r "not supported in IR yet" crates/` returns zero results).

---

### Milestone 3 — LSP server modularization

**Goal:** Split the 6,892-line `fuse-lsp.rs` monolith into focused modules
without changing any observable LSP behavior.

**Scope:**

- Extract handler groups into separate modules:
  - `lsp/diagnostics.rs` — document diagnostics and publish flow
  - `lsp/navigation.rs` — definition, references, hover, call hierarchy
  - `lsp/completion.rs` — completion and signature help
  - `lsp/refactor.rs` — rename, code actions, organize imports
  - `lsp/tokens.rs` — semantic tokens and inlay hints
  - `lsp/workspace.rs` — workspace symbols, cache management, incremental relink
  - `lsp/server.rs` — main loop, dispatch, lifecycle
- Preserve all existing LSP test coverage without changes to test assertions
- Preserve all incremental relink and caching behavior exactly

**Deliverables:**

- [x] `fuse-lsp.rs` reduced to <500 lines (dispatch + lifecycle)
- [x] All extracted modules compile and pass existing tests
- [x] `lsp_suite.sh` green
- [x] `lsp_perf_reliability.sh` green (no responsiveness regression)
- [x] `lsp_workspace_incremental.sh` green

**Progress update (2026-03-01):**

- Added module scaffold `crates/fusec/src/bin/fuse_lsp/mod.rs`
- Extracted diagnostics publish flow into `crates/fusec/src/bin/fuse_lsp/diagnostics.rs`
- Extracted main loop/dispatch lifecycle into `crates/fusec/src/bin/fuse_lsp/server.rs`
- Extracted navigation handlers into `crates/fusec/src/bin/fuse_lsp/navigation.rs` (`definition`, `hover`, `references`, call hierarchy handlers)
- Extracted completion/signature handlers into `crates/fusec/src/bin/fuse_lsp/completion.rs`
- Extracted workspace-symbol/rename/code-action handlers into `crates/fusec/src/bin/fuse_lsp/refactor.rs`
- Extracted semantic tokens + inlay hints into `crates/fusec/src/bin/fuse_lsp/tokens.rs`
- Extracted workspace snapshot/index/cache internals into `crates/fusec/src/bin/fuse_lsp/workspace.rs`
- Extracted remaining symbol index-builder internals + qualified-ref collector into `crates/fusec/src/bin/fuse_lsp/symbols.rs`
- Extracted shared LSP constants/state/JSON+LSP helpers into `crates/fusec/src/bin/fuse_lsp/core.rs`
- Rewired `textDocument/didOpen`, `textDocument/didChange`, and `textDocument/didClose` through the diagnostics module
- Rewired navigation/call-hierarchy dispatch paths through the navigation module
- Rewired completion/signature and rename/prepareRename/codeAction/workspaceSymbol dispatch paths through the new modules
- Rewired semanticTokens/inlayHint dispatch paths through the tokens module
- Rewired root workspace/cache/index entrypoints through the workspace module (`build_workspace_*`, `workspace_stats_result`, `try_incremental_module_update`, `WorkspaceIndex`/`WorkspaceSnapshot`)
- `fuse-lsp.rs` line count reduced from 6910 to 14 (6896 lines removed; root now only wires module surface + process entrypoint)
- Hardened local cargo test environment in `scripts/cargo_env.sh` for EXDEV stability:
  - default `CARGO_BUILD_PIPELINING=false`
  - default `CARGO_BUILD_JOBS=1` for `cargo test` invocations (overridable via env)
  - automatic bounded retry for cargo commands when `Invalid cross-device link (os error 18)` is detected (`CARGO_ENV_EXDEV_RETRIES`, default `6`)
- Validation status for current extraction:
  - `cargo check -p fusec --bin fuse-lsp`
  - `scripts/lsp_suite.sh`
  - `scripts/release_smoke.sh`

**Exit criteria:** Met on 2026-03-01 (`release_smoke.sh` green). No LSP behavior change observable by users.

---

### Milestone 4 — Example and documentation coverage

**Goal:** Provide standalone examples for every major language feature so that
users can learn FUSE from examples alone, and ensure the reference documentation
covers 100% of the spec surface.

**Scope:**

New examples:

- [x] `examples/transaction_demo.fuse` — `transaction:` block with rollback demonstration
- [x] `examples/capability_demo.fuse` — `requires db` + `requires network` with cross-module calls
- [x] `examples/test_demo.fuse` — `test "..."` blocks with `assert()`
- [x] `examples/strict_arch_demo/` — multi-file package using `--strict-architecture`
- [x] `examples/refinement_demo.fuse` — `regex()`, `predicate()`, range constraints
- [x] `examples/json_codec.fuse` — `json.encode` / `json.decode` round-trip
- [x] `examples/time_crypto.fuse` — `time.*` and `crypto.*` usage (after Milestone 1)
- [x] `examples/dep_import/` — multi-package with `dep:` imports

Documentation:

- [x] `examples/README.md` updated with feature-to-example index
- [x] `scripts/check_examples.sh` extended to cover new examples
- [x] All new examples verified in `release_smoke.sh`

**Progress update (2026-03-01):**

- Added all milestone-targeted example assets:
  - standalone files: `transaction_demo.fuse`, `capability_demo.fuse`, `test_demo.fuse`, `refinement_demo.fuse`, `json_codec.fuse`, `time_crypto.fuse`
  - support module: `capability_demo_data.fuse`
  - package examples: `strict_arch_demo/` and `dep_import/`
- Updated `examples/README.md` with a feature-to-example index covering the new files/directories
- Extended `scripts/check_examples.sh` to validate package examples via manifest-aware checks, including strict-architecture mode for `strict_arch_demo`
- Validation run green: `scripts/check_examples.sh` (outside sandbox to avoid local EXDEV build artifacts)
- Validation run green: `scripts/release_smoke.sh`

**Exit criteria:** Met on 2026-03-01 (examples validated via `check_examples.sh` and `release_smoke.sh`; feature coverage index added in `examples/README.md`).

---

### Milestone 5 — Database and config ergonomics

**Goal:** Improve the day-to-day experience of working with DB operations and
config values, addressing the most common friction points visible in the
reference service.

**Scope:**

Database:

- [x] `db.from(table).insert(struct)` — insert from struct fields
- [x] `db.from(table).update(column, value).where(...)` — update builder method
- [x] `db.from(table).delete().where(...)` — delete builder method
- [x] Query builder `.count()` method
- [x] Error messages for DB operations include the SQL text and parameter summary

Config:

- [x] Support user-defined `type` values in config env overrides (JSON text parsing, consistent with CLI binding)
- [x] Support `enum` values in config env overrides
- [x] Diagnostic hint when config field name doesn't match expected env var name

**Deliverables:**

- [x] AST + native implementation for all new query builder methods
- [x] Parity tests for insert/update/delete/count
- [x] `spec/runtime.md` updated for new query builder surface
- [x] Reference service migrated to use new builder methods where applicable
- [x] Config env override tests for user-defined types

**Progress update (2026-03-01):**

- Query builder expanded across AST/native/IR/sema surfaces with new methods:
  - `insert(struct)`, `update(column, value)`, `delete()`, `count()`
- Database runtime errors now include SQL text + compact parameter summaries
- Parser updated to accept `.delete()` member calls despite `delete` keyword tokenization
- AOT config loader (`load_configs_for_binary` / `ConfigEvaluator`) now supports user-defined
  struct + enum env JSON overrides, including nested type-field defaults
- Added config env-name typo hinting (`APP_DBURL` -> `APP_DB_URL`) in runtime config resolution
- Updated docs:
  - `spec/runtime.md`
  - regenerated `docs/site/specs/reference.md` via `scripts/generate_guide_docs.sh`
- Migrated reference-service DB writes where applicable to query-builder updates/deletes
- Validation runs:
  - `scripts/cargo_env.sh cargo check -p fusec -p fuse`
  - `scripts/cargo_env.sh cargo test -p fusec query_builder_insert_update_delete_count_flow`
  - `scripts/cargo_env.sh cargo test -p fusec db_errors_include_sql_and_params`
  - `scripts/cargo_env.sh cargo test -p fusec --test parity_ast_native parity_db_query_builder`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_runtime_supports_user_defined_config_env_overrides`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_runtime_emits_config_env_name_hint_for_mismatch`
  - `scripts/check_examples.sh`
  - `scripts/semantic_suite.sh`

**Exit criteria:** Met on 2026-03-01 (`authority_parity.sh` and `semantic_suite.sh` green; semantic suite includes authority parity gate).

---

### Milestone 6 — Developer workflow improvements

**Goal:** Reduce iteration friction for the check/run/dev/build cycle.

**Scope:**

- [x] `fuse check` incremental mode — skip re-checking unchanged modules (use module content hashes from incremental check metadata)
- [ ] `fuse dev` diagnostic overlay — show first compilation error in browser (injected via existing `__reload` WebSocket)
- [x] `fuse test --filter "pattern"` — run subset of test blocks matching a name pattern
- [ ] `fuse build` progress indicator for AOT compilation steps
- [ ] Structured JSON diagnostic output mode (`--diagnostics json`) for CI/editor consumption

**Deliverables:**

- [x] Incremental check validated with module-edit-recheck benchmarks
- [ ] `fuse dev` overlay tested with intentional syntax errors
- [x] `fuse test --filter` documented in README and reference
- [ ] JSON diagnostics schema documented
- [ ] `use_case_bench.sh` updated with incremental check metric

**Progress update (2026-03-01):**

- Stabilized local runtime integration harnessing for faster dev-loop reruns:
  - `crates/fusec/tests/html_runtime.rs` now allocates app ports only inside the HTTP test lock
  - lock poisoning no longer cascades (`PoisonError` recovery via `into_inner()`)
  - upstream one-shot bind in vite proxy tests is coordinated by the same lock to prevent cross-test socket collisions
  - `crates/fusec/tests/parity_ast_native.rs` HTTP harness now uses the same serialized lock strategy, retries bind collisions, and retries transient connect/read/write reset paths before failing
  - `crates/fusec/tests/result_decode_runtime.rs` now serializes HTTP harness setup, retries bind collisions, and treats transient connect/write/read reset paths as retryable until timeout
- Added test filtering for project test runs:
  - `fuse test --filter <pattern>` implemented in CLI argument plumbing (`fuse` -> `fusec`)
  - `fusec --test --filter <pattern>` filters test jobs by case-sensitive substring match on test name
  - docs updated in `README.md` and `spec/runtime.md`; regenerated `docs/site/specs/reference.md`
- Implemented project-level incremental `fuse check` caching:
  - warm-cache fast path returns immediately when `.fuse/build/check.meta` (or `check.strict.meta`) remains valid
  - cache invalidation keys include module content hashes plus manifest/lock/build fingerprints
  - incremental re-check computes changed modules and transitive importers, then sema-checks only affected roots
  - check cache metadata is refreshed on successful runs
- Added CLI regression coverage for incremental behavior in `crates/fuse/tests/project_cli.rs`:
  - `check_incremental_cache_hit_skips_unchanged_modules`
  - `check_incremental_rechecks_importers_when_dependency_changes`
- Cold vs warm reference-service measurement confirms expected speedup:
  - cold: `0.18s`
  - warm: `0.12s`
- Validation runs green:
  - `scripts/cargo_env.sh cargo test -p fusec --test html_runtime`
  - `scripts/cargo_env.sh cargo test -p fusec --test result_decode_runtime`
  - `scripts/cargo_env.sh cargo test -p fusec --test parity_ast_native`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli test_filter_runs_matching_tests_only`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli filter_option_is_rejected_for_non_test_commands`
  - `scripts/cargo_env.sh cargo check -p fuse -p fusec`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_incremental_cache_hit_skips_unchanged_modules`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_incremental_rechecks_importers_when_dependency_changes`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_`

**Exit criteria:** Warm `fuse check` on reference-service measurably faster than cold check.

---

### Milestone 7 — Release hardening and gating

**Goal:** All milestones integrated, documented, and passing full release gate.

**Scope:**

- [ ] `CHANGELOG.md` updated with all 0.8.0 additions/changes
- [ ] `governance/scope.md` roadmap refreshed (mark completed items, add next priorities)
- [ ] `governance/VERSIONING_POLICY.md` updated with 0.8.0 compatibility line
- [ ] `spec/fls.md` and `spec/runtime.md` fully reflect new behavior
- [ ] `docs/site/specs/reference.md` regenerated and verified
- [ ] `benchmarks/use_case_baseline.json` refreshed if metrics changed
- [ ] `ops/RELEASE.md` checklist executed

**Gate commands (all must pass):**

```bash
scripts/semantic_suite.sh
scripts/authority_parity.sh
scripts/lsp_suite.sh
scripts/lsp_perf_reliability.sh
scripts/lsp_workspace_incremental.sh
scripts/use_case_bench.sh
scripts/check_use_case_bench_regression.sh
scripts/aot_perf_bench.sh
scripts/check_aot_perf_slo.sh
scripts/packaging_verifier_regression.sh
scripts/release_smoke.sh
scripts/check_examples.sh
```

**Exit criteria:** `release_smoke.sh` green. No known regressions. All docs regenerated.

---

## Milestone dependency graph

```
M1 (time/crypto APIs)
  └──→ M4 (examples — time_crypto.fuse depends on M1)

M2 (IR lowering completeness)  ──→ M7 (release)
M3 (LSP modularization)        ──→ M7
M4 (examples + docs)           ──→ M7
M5 (DB/config ergonomics)      ──→ M7
M6 (workflow improvements)     ──→ M7

M1, M2, M3, M5, M6 are independent of each other and can be parallelized.
M7 is the integration/release gate and depends on all others.
```

---

## Out of scope for 0.8.0

These items are deferred to future releases:

- User-defined generics / parametric polymorphism (identity charter: will not do)
- Macro / metaprogramming system (identity charter: will not do)
- Non-SQLite database backends (scope.md: not committed)
- WASM / embedded / mobile targets (scope.md: not in MVP)
- Interface / trait-style abstraction mechanisms (scope.md: likely future, not 0.8.0)
- Package registry / remote dependency hosting
- `dep:` git dependency authentication (SSH keys, tokens)
- Runtime plugin extension system (explicit non-goal per runtime.md)

---

## Risk assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `time`/`crypto` API design disagreements | Medium | Delays M1 | Keep surface minimal; follow existing builtin patterns |
| LSP modularization introduces subtle regressions | Medium | Blocks M3 | Existing test coverage is extensive; refactor is structural only |
| IR lowering edge cases harder than expected | Low | Delays M2 | Scope is only 3 known paths; fallback is documenting limitations |
| Incremental check hashing adds complexity | Medium | Delays M6 | Can ship `fuse test --filter` independently |
| DB insert/update/delete builder changes SQL generation | Low | Delays M5 | Query builder is well-tested; new methods are additive |

---

## Success criteria

v0.8.0 is ready to ship when:

1. `requires time` and `requires crypto` unlock working runtime APIs
2. Every valid FUSE program can compile through the native backend
3. LSP source is modular and maintainable
4. Every language feature has a standalone example
5. DB query builder covers CRUD operations
6. `fuse check` is measurably faster for unchanged modules
7. `release_smoke.sh` passes without exceptions
