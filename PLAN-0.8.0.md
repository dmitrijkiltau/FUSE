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
  - regenerated `guides/reference.md` via `scripts/generate_guide_docs.sh`
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
- [x] `fuse dev` diagnostic overlay — show first compilation error in browser (injected via existing `__reload` WebSocket)
- [x] `fuse test --filter "pattern"` — run subset of test blocks matching a name pattern
- [x] `fuse build` progress indicator for AOT compilation steps
- [x] Structured JSON diagnostic output mode (`--diagnostics json`) for CI/editor consumption

**Deliverables:**

- [x] Incremental check validated with module-edit-recheck benchmarks
- [x] `fuse dev` overlay tested with intentional syntax errors
- [x] `fuse test --filter` documented in README and reference
- [x] JSON diagnostics schema documented
- [x] `use_case_bench.sh` updated with incremental check metric

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
  - docs updated in `README.md` and `spec/runtime.md`; regenerated `guides/reference.md`
- Implemented project-level incremental `fuse check` caching:
  - warm-cache fast path returns immediately when `.fuse/build/check.meta` (or `check.strict.meta`) remains valid
  - cache invalidation keys include module content hashes plus manifest/lock/build fingerprints
  - incremental re-check computes changed modules and transitive importers, then sema-checks only affected roots
  - check cache metadata is refreshed on successful runs
- Implemented `fuse dev` browser diagnostic overlay via the existing reload websocket:
  - `fuse dev` now performs a compile gate (parse + sema) before each restart and emits the first compile error as a websocket event payload
  - runtime live-reload script (AST/native) now handles websocket event types (`reload`, `clear_error`, `compile_error`) and renders an in-browser overlay for compile failures
  - successful restarts clear overlays and trigger the existing reload flow
- Implemented deterministic AOT build progress stages in `fuse build`:
  - AOT-emitting builds now print `[build] aot [n/6] ...` checkpoints across compile/cache/object/runner/link stages
  - non-AOT `fuse build` output remains unchanged except standard start/ok|failed step markers
- Implemented structured JSON diagnostics mode:
  - new `--diagnostics json` option for `fuse` commands switches stderr diagnostic output to JSON Lines
  - wrapper emits machine-readable command-step events (`kind=command_step`) and structured compiler diagnostics (`kind=diagnostic`)
  - delegated `fusec` execution paths (`fuse check|run|test` fallback) now honor the same mode for parse/sema diagnostics via shared diagnostics formatting
- Documented JSON diagnostics schema in `spec/runtime.md` and propagated to `guides/reference.md`
- Added JSON diagnostics regression coverage:
  - `crates/fuse/tests/project_cli.rs::check_diagnostics_json_emits_structured_output_for_project_mode`
  - `crates/fuse/tests/project_cli.rs::check_diagnostics_json_emits_structured_output_for_delegated_mode`
  - `crates/fuse/tests/project_cli.rs::run_validation_errors_emit_json_step_events_when_diagnostics_json_enabled`
- Added AOT progress regression coverage:
  - `crates/fuse/tests/project_cli.rs::build_aot_emits_progress_indicator`
- Added `fuse dev` overlay regression coverage:
  - `crates/fuse/tests/project_cli.rs::dev_emits_compile_error_overlay_event_for_syntax_error`
  - `crates/fusec/tests/html_runtime.rs::html_http_injects_live_reload_script_when_enabled` now asserts overlay marker/event wiring
- Added CLI regression coverage for incremental behavior in `crates/fuse/tests/project_cli.rs`:
  - `check_incremental_cache_hit_skips_unchanged_modules`
  - `check_incremental_rechecks_importers_when_dependency_changes`
- Updated `scripts/use_case_bench.sh` for incremental-check benchmarking:
  - benchmark runs against a per-iteration temp copy of `examples/reference-service` to safely apply synthetic module edits
  - new metric `reference_service.check_incremental_edit_ms` measures `fuse check --manifest-path ...` immediately after editing `src/errors.fuse`
  - benchmark outputs now include the new metric in both Markdown and JSON (`schema_version: 3`)
- Cold/warm/incremental check measurement confirms expected incremental behavior:
  - cold: `113.380 ms`
  - warm: `112.120 ms`
  - incremental edit re-check: `115.257 ms`
- Validation runs green:
  - `scripts/cargo_env.sh cargo test -p fusec --test html_runtime`
  - `scripts/cargo_env.sh cargo test -p fusec --test result_decode_runtime`
  - `scripts/cargo_env.sh cargo test -p fusec --test parity_ast_native`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli test_filter_runs_matching_tests_only`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli filter_option_is_rejected_for_non_test_commands`
  - `scripts/cargo_env.sh cargo check -p fuse -p fusec`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_incremental_cache_hit_skips_unchanged_modules`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_incremental_rechecks_importers_when_dependency_changes`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli dev_emits_compile_error_overlay_event_for_syntax_error`
  - `scripts/cargo_env.sh cargo test -p fusec --test html_runtime html_http_injects_live_reload_script_when_enabled`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_emits_progress_indicator`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_diagnostics_json_emits_structured_output_for_project_mode`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli check_diagnostics_json_emits_structured_output_for_delegated_mode`
  - `scripts/cargo_env.sh cargo test -p fuse --test project_cli run_validation_errors_emit_json_step_events_when_diagnostics_json_enabled`
  - `./scripts/use_case_bench.sh`
  - `./scripts/check_use_case_bench_regression.sh`

**Exit criteria:** Warm `fuse check` on reference-service measurably faster than cold check.

---

### Milestone 7 — Release hardening and gating

**Goal:** All milestones integrated, documented, and passing full release gate.

**Scope:**

- [ ] `CHANGELOG.md` updated with all 0.8.0 additions/changes
- [ ] `governance/scope.md` roadmap refreshed (mark completed items, add next priorities)
- [ ] `governance/VERSIONING_POLICY.md` updated with 0.8.0 compatibility line
- [ ] `spec/fls.md` and `spec/runtime.md` fully reflect new behavior
- [ ] GitHub-facing guide/reference markdown regenerated and verified
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

### Milestone 7A — Code cleanup and redundancy removal (pre-tag)

**Goal:** Reduce maintenance cost and duplicated logic before the 0.8.0 tag
without changing language/runtime behavior.

**Scope:**

- [x] Split `crates/fuse/src/main.rs` into focused `crates/fuse/src/` modules (args, diagnostics, run/dev/build, deps/lock, AOT helpers)
- [x] Remove diagnostics formatting duplication between `fuse` and `fusec` (`--diagnostics json|text` surface stays identical)
- [x] Extract shared HTTP/runtime integration harness helpers for `fusec` tests
- [x] Split `crates/fuse/tests/project_cli.rs` into domain-focused files with shared fixture/process helpers
- [x] Remove redundant script/report glue where equivalent helpers already exist

**Deliverables:**

- [x] `crates/fuse/src/main.rs` reduced to dispatch/lifecycle oriented surface (<1500 lines target)
- [x] One canonical diagnostics JSON/text renderer path used by both CLIs
- [x] No duplicated `send_http_request_with_retry` helper implementations across runtime parity tests
- [x] `project_cli` coverage preserved after file split (`build`, `run/dev`, `deps/lock`, `diagnostics/format`)
- [x] Release gates still pass after cleanup refactors (`release_smoke.sh`, `use_case_bench.sh`, `check_use_case_bench_regression.sh`)

**Execution slices (recommended order):**

1. `fuse` CLI modularization pass:
   - extract `main.rs` command-specific blocks into dedicated modules with zero behavior changes
   - keep root `main.rs` as argument entrypoint + dispatch only
2. Diagnostics dedup pass:
   - move JSON/text diagnostic rendering helpers into shared code consumed by both `fuse` and `fusec`
   - keep existing JSON schema contract unchanged
3. Runtime test harness dedup pass:
   - add shared helpers under `crates/fusec/tests/support/` for port allocation, server readiness, and retryable HTTP requests
   - migrate `html_runtime.rs`, `parity_ast_native.rs`, and `result_decode_runtime.rs`
4. `project_cli` split pass:
   - create `crates/fuse/tests/support/` helpers
   - split monolithic test file into feature-grouped test files while preserving test names/coverage
5. Script cleanup pass:
   - remove redundant local helper logic in scripts where already centralized
   - re-run bench/regression scripts and update baseline only if metrics changed

**Progress update (2026-03-01):**

- Slice 1 started with low-risk CLI decomposition:
  - extracted argument discovery/parsing into `crates/fuse/src/cli_args.rs` (`discover_color_choice`, `discover_diagnostics_format`, `parse_common_args`)
  - extracted manifest loading/entry resolution into `crates/fuse/src/manifest.rs` (`load_manifest`, `find_manifest`, `resolve_entry`)
  - rewired `run()` in `crates/fuse/src/main.rs` to call the new modules without behavior changes
- continued Slice 1 command-surface extraction:
  - extracted project command operations into `crates/fuse/src/command_ops.rs` (`run_build`, `run_project_check`, `run_project_fmt` + local OpenAPI/project-file helpers)
  - rewired command dispatch in `run()` to call `command_ops` module functions
- continued Slice 1 dev-loop extraction:
  - extracted dev watch/restart workflow into `crates/fuse/src/dev.rs` (`run_dev`, compile-error probe, reload websocket hub, watch snapshot helpers)
  - rewired `Command::Dev` dispatch in `run()` to call `dev::run_dev`
- continued Slice 1 dependency/lock extraction:
  - extracted dependency resolver + lockfile/git internals into `crates/fuse/src/deps.rs` (`resolve_dependencies`, lockfile read/write, git checkout/reference helpers, dependency normalization)
  - rewired dependency loading in `run()` to call `deps::resolve_dependencies`
- continued Slice 1 asset-pipeline extraction:
  - extracted asset/static helper internals into `crates/fuse/src/assets.rs` (`collect_files_by_extension`, `resolve_manifest_relative_path`, asset hook + hashing/manifest helpers, `run_asset_pipeline`, `apply_asset_manifest_env`)
  - rewired build/dev paths to call `assets::*` (`command_ops::run_build`, `dev::run_dev`) and kept serve env integration via `apply_asset_manifest_env`
- continued Slice 1 runtime-env/bootstrap extraction:
  - extracted run/dev serve environment + OpenAPI UI + dotenv/bootstrap helpers into `crates/fuse/src/runtime_env.rs` (`configure_openapi_ui_env`, `apply_serve_env`, `apply_dotenv`, `apply_default_config_path`)
  - rewired `run()` in `crates/fuse/src/main.rs` to call `runtime_env::*`
- continued Slice 1 CLI output/diagnostics extraction:
  - extracted CLI color/output/step + diagnostic emit helpers into `crates/fuse/src/cli_output.rs` (`apply_color_choice`, `apply_diagnostics_format`, `emit_cli_error`, `emit_command_step`, `emit_diags_with_fallback`, `line_info`, etc.)
  - rewired root/child-module access via `main.rs` re-export surface without behavior changes
- continued Slice 1 AOT/native compile+link extraction:
  - extracted AOT/native compile-link/cache/runtime helpers into `crates/fuse/src/aot.rs` (`compile_artifacts`, `write_compiled_artifacts`, `write_native_binary`, native runner generation + link helpers, `try_load_native`, `run_native_program`)
  - rewired `run()` + `command_ops::run_build` to call `aot::*`
- continued Slice 1 build-cache/meta extraction:
  - extracted SHA1 + build dir + IR meta/check metadata helpers into `crates/fuse/src/cache.rs` (`sha1_digest`, `build_dir`, `clean_build_dir`, `build_ir_meta`, `load/write_ir_meta`, incremental-check delta helpers, `file_stamp`)
  - rewired shared usage through `main.rs` re-export surface consumed by `aot`, `assets`, `command_ops`, and `dev`
- continued Slice 1 model/type extraction:
  - extracted manifest/dependency + IR metadata structs into `crates/fuse/src/model.rs` (`Manifest`, `DependencySpec`, `DependencyDetail`, `IrMeta`, `IrFileMeta`, serve/build/assets/vite config structs)
  - rewired shared usage through `main.rs` re-export surface consumed by `manifest`, `deps`, `assets`, `runtime_env`, `command_ops`, `cache`, and `aot`
- continued Slice 2 diagnostics dedup extraction:
  - added shared diagnostics renderer in `crates/fusec/src/diag_render.rs` (`line_info`, JSON diagnostic value builder, configurable text diagnostic emitter)
  - rewired `crates/fuse/src/cli_output.rs` and `crates/fusec/src/cli.rs` to use the shared renderer while preserving existing CLI-specific style behavior (`fuse` colored labels/caret and fallback path, `fusec` plain text style)
  - exported shared renderer via `crates/fusec/src/lib.rs`
- continued Slice 3 runtime test harness dedup extraction:
  - added shared HTTP runtime test helpers in `crates/fusec/tests/support/http.rs` (`HttpResponse`, retryable HTTP request + status/body helper) and exported via `crates/fusec/tests/support/mod.rs`
  - migrated duplicated HTTP request helpers out of `html_runtime.rs`, `parity_ast_native.rs`, `result_decode_runtime.rs`, `golden_outputs.rs`, and `native_http_smoke.rs`
  - upgraded shared `crates/fusec/tests/support/net.rs::find_free_port` to deterministic process-local port cycling with bind checks (fallback keeps `127.0.0.1:0`)
- continued Slice 4 `project_cli` split extraction:
  - reduced `crates/fuse/tests/project_cli.rs` to shared fixtures/helpers + module wiring
  - moved test coverage into domain-focused modules under `crates/fuse/tests/project_cli/`:
    - `run_dev_build.rs`
    - `deps_lock.rs`
    - `output_aot.rs`
    - `test_diag.rs`
  - preserved all existing test names/assertions while reusing shared helper surface from root module
- continued Slice 5 script glue cleanup:
  - added shared shell helper `scripts/lib/common.sh` (`fuse_repo_root`, `step`)
  - removed duplicated root/step boilerplate from:
    - `scripts/authority_parity.sh`
    - `scripts/semantic_suite.sh`
    - `scripts/lsp_suite.sh`
    - `scripts/release_smoke.sh`
- post-extraction stabilization:
  - hardened `project_cli` HTTP service test harness port selection to avoid cross-test port reuse races (`reserve_local_port` now uses deterministic process-local port cycling with bind checks)
  - hardened HTTP request retries for AOT service tests to wait for successful 2xx responses instead of accepting transient non-success responses
  - optimized AOT link dependency resolution in `crates/fuse/src/aot.rs` to reuse existing `fusec`/`bincode` rlibs when already present (fallback still runs `cargo build -p fusec` when needed)
- `crates/fuse/src/main.rs` line count reduced from `4254` to `418` across the first ten cuts
- Validation runs green:
  - `./scripts/cargo_env.sh cargo fmt`
  - `./scripts/cargo_env.sh cargo check -p fuse`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_manifest_path_reports_cross_file_location`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_diagnostics_json_emits_structured_output_for_project_mode`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli unknown_command_uses_error_prefix`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli filter_option_is_rejected_for_non_test_commands`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli dev_emits_compile_error_overlay_event_for_syntax_error`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_reports_transitive_dependency_conflicts_with_origin_paths`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_rejects_dependency_with_multiple_git_reference_fields`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_reports_lock_entry_unknown_source_code`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_reports_lock_entry_subdir_not_found_code`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli run_with_program_args_uses_cached_ir_when_valid`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli build_runs_before_build_hook_when_configured`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli run_validation_errors_emit_json_step_events_when_diagnostics_json_enabled`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_emits_progress_indicator`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli run_invalidates_cached_ir_when_meta_target_fingerprint_changes`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_build_info_env_prints_embedded_metadata`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_startup_trace_env_emits_operability_header`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_service_request_id_and_structured_logs_are_consistent`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli build_aot_` (`16 passed`, runtime dropped from ~`65s` to ~`12s`)
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli` (`78 passed`)
  - `./scripts/cargo_env.sh cargo check -p fuse -p fusec`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_diagnostics_json_emits_structured_output_for_project_mode`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli check_diagnostics_json_emits_structured_output_for_delegated_mode`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli run_validation_errors_emit_json_step_events_when_diagnostics_json_enabled`
  - `./scripts/cargo_env.sh cargo check -p fusec --tests`
  - `./scripts/cargo_env.sh cargo test -p fusec --test golden_outputs --test html_runtime --test native_http_smoke --test parity_ast_native --test result_decode_runtime`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli --no-run`
  - `./scripts/cargo_env.sh cargo test -p fuse --test project_cli` (`78 passed`)
  - `bash -n scripts/lib/common.sh scripts/authority_parity.sh scripts/semantic_suite.sh scripts/lsp_suite.sh scripts/release_smoke.sh`
  - `./scripts/authority_parity.sh`
  - `./scripts/release_smoke.sh`
  - `./scripts/use_case_bench.sh`
  - `./scripts/check_use_case_bench_regression.sh`

**Exit criteria:** Refactor-only changes merged with no observable CLI/runtime behavior regression and full release gates green.

---

### Milestone 7B — GitHub-first guide surface (remove `/docs`)

**Goal:** Replace the current `docs/` site package with a repository-native markdown
guide surface that is directly readable on GitHub, while preserving reference
coverage and generation ergonomics.

**Scope:**

- [x] Remove the `docs/` folder from the repository
- [x] Introduce a root-level guide surface (for example `guides/`) with separate markdown files (`onboarding`, `boundary-contracts`, `reference`, etc.)
- [x] Update `scripts/generate_guide_docs.sh` to generate into the new GitHub-facing location instead of `docs/site/specs/`
- [x] Update all references/build scripts/tests that currently point at `docs/site/specs/reference.md`
- [x] Keep generated guide links valid in GitHub markdown navigation (`README.md` and guide cross-links)

**Deliverables:**

- [x] `docs/` removed with no broken references in repository tooling/docs
- [x] Generated guide files exist as first-class markdown docs in-repo (GitHub-renderable)
- [x] `scripts/generate_guide_docs.sh` succeeds and is the canonical generation path
- [x] `README.md` links to the new guide index/reference locations
- [x] Release gates still pass after docs migration (`semantic_suite.sh`, `authority_parity.sh`, `release_smoke.sh`)

**Execution slices (recommended order):**

1. Layout migration pass:
   - introduce new root guide directory and move generated outputs there
   - keep filenames/link slugs stable where possible
2. Generator migration pass:
   - retarget `generate_guide_docs.sh` inputs/outputs to new layout
   - regenerate guide/reference markdown and verify diff quality
3. Reference rewiring pass:
   - update README/spec/governance/script references to new paths
   - remove `docs/` package and dead docs-only assets
4. Gate pass:
   - run release-critical scripts and ensure no regressions

**Progress update (2026-03-02):**

- Migrated guide source + outputs to root `guides/`:
  - source moved to `guides/src/` (`onboarding.fuse`, `boundary-contracts.fuse`)
  - generated outputs now in `guides/` (`onboarding.md`, `boundary-contracts.md`, `reference.md`)
  - added `guides/README.md` index and moved migration guide to `guides/migrations/0.1-to-0.2.md`
- Retargeted generator:
  - `scripts/generate_guide_docs.sh` now reads from `guides/src` and writes to `guides/`
  - generation run completed: `./scripts/generate_guide_docs.sh`
- Repository reference rewiring completed:
  - updated paths in `README.md`, `AGENTS.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`, `governance/VERSIONING_POLICY.md`, and `CHANGELOG.md`
- Removed `docs/` package and assets completely from repository tree
- Post-migration validation runs green:
  - `./scripts/semantic_suite.sh`
  - `./scripts/authority_parity.sh`
  - `./scripts/release_smoke.sh`

**Exit criteria:** `docs/` removed, GitHub-first guide markdown generated by script, and release gates remain green.

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
M7A (cleanup + dedupe)         ──→ M7
M7B (docs to github guides)    ──→ M7

M1, M2, M3, M5, M6 are independent of each other and can be parallelized.
M7A and M7B run as pre-tag hardening before the release gate.
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
