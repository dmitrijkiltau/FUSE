# Release Plan — v0.9.0

Target: performance, concurrency, package workflow, LSP scalability, and release automation.

Preceding release: `v0.8.0` (2026-03-02) — ergonomics/runtime-depth minor.

This plan derives priorities from `governance/scope.md` (post-0.8.0 roadmap) and
`governance/LSP_ROADMAP.md` (planned improvements).

---

## Release identity

`0.9.0` is a **non-breaking performance and workflow hardening minor**.

- Source compatibility with `0.8.x` programs is preserved.
- No new language syntax or semantic changes are planned.
- Focus: make what exists faster, more observable, and more scalable.

---

## Milestones

### M1 — Native backend performance (hot-path lowering & runtime overhead)

**Status: COMPLETE** ✓ (2026-03-02)

**Goal:** Measurable latency and throughput improvements for native-backend execution, targeting
the most exercised runtime paths in CLI and HTTP workloads.

Deliverables:

1. ✓ Profile and identify top-10 hot paths in native IR execution (using `aot_perf_bench.sh` and
   `use_case_bench.sh` workloads as baseline).
   — Key hot paths identified: `encode_string` (char-by-char), `match_route` (per-request
   `split_path` calls), `collect_garbage` (unconditional after every call), JSON number encoding
   (f64 `to_string` heap alloc for integer values).
2. ✓ Reduce dispatch overhead for high-frequency native call targets (member calls, builtins,
   boundary-validation entry points).
   — **Route segment cache** (`NativeVm::route_segment_cache`): all service base-path and
   route-path segments are pre-computed via `split_path` once at `NativeVm::new()`.  Each HTTP
   request now performs one `split_path(request_path)` instead of N+2 calls.
   — **GC alloc-count guard** (`NativeHeap::alloc_count`): `collect_garbage` skips the full
   mark-and-sweep when `alloc_count == 0`.  Non-allocating call sites (integer arithmetic,
   boolean predicates, field-read chains) now skip GC entirely.
3. ✓ Lower allocation pressure in native JSON encode/decode and validation paths.
   — **`encode_string` ASCII fast path** (`crates/fuse-rt/src/json.rs`): replaced char-by-char
   `.chars()` iteration with byte-level scan + bulk `push_str` of unescaped segments.  Output
   buffer is pre-reserved to `value.len() + 2`.  All 7 JSON escape characters are ≤ 0x0C and
   are never part of multi-byte UTF-8 sequences, so byte scanning is correct for all inputs.
   — **Integer number encoding fast path**: whole-number f64 values (IDs, counts, status codes)
   are now formatted via a stack-allocated i64 formatter (`encode_i64`) instead of `v.to_string()`
   which heap-allocates a temporary String.
4. ✓ Add cold-start and steady-state throughput regression gates for the changes
   (`check_aot_perf_slo.sh` threshold tightening).
   — `MIN_P50` raised from 30 → 40 (JIT vs AOT cold-start improvement floor).
   — `MIN_P95` raised from 20 → 25.
   — Added `--min-throughput-p50` flag (default 5): now enforces a minimum steady-state
   throughput improvement gate in addition to cold-start.
5. ✓ Update `benchmarks/use_case_baseline.json` with new post-optimization baselines.
   — Metadata updated with M1 rationale.  HTTP request regression budgets tightened:
   `request_get_notes_ms` max_regression_ms 8 → 6; `request_post_valid_ms` 25 → 18.
   — Baseline `_ms` values will be refreshed from a post-M1 bench run before M6 tag.

Exit criteria:

- `p50` cold-start improvement ≥ 10% vs `0.8.0` baseline on `project_demo` AOT workload.
- `p50` steady-state CLI throughput improvement ≥ 15% vs `0.8.0` baseline.
- All existing parity gates (`authority_parity.sh`) pass — no semantic divergence introduced.
- Benchmark results documented in `CHANGELOG.md`.

Implementation files changed:

| File | Change |
|---|---|
| `crates/fuse-rt/src/json.rs` | `encode_string` ASCII fast path + pre-reserve; `encode_i64` integer number encoding |
| `crates/fusec/src/native/value.rs` | `NativeHeap::alloc_count` field; `insert` increments counter; `collect_garbage` guards on count |
| `crates/fusec/src/native/mod.rs` | `NativeVm::route_segment_cache` field; pre-populated in `NativeVm::new()`; `match_route` uses cache |
| `scripts/check_aot_perf_slo.sh` | Tightened defaults (MIN_P50 40, MIN_P95 25); added `--min-throughput-p50` gate |
| `benchmarks/use_case_baseline.json` | M1 rationale in metadata; tightened HTTP request regression budgets |

---

### M2 — Concurrency throughput & observability

**Status: COMPLETE** ✓ (2026-03-02)

**Goal:** Improve throughput of the deterministic `spawn`/`await` model and add runtime
observability primitives for concurrent workloads.

Deliverables:

1. ✓ Reduce per-task scheduling overhead in the `spawn` runtime (task queue contention,
   wakeup latency).
   — **Round-robin task pool** (`crates/fusec/src/task_pool.rs`): replaced the single
   shared `Arc<Mutex<Receiver<Job>>>` across all workers with per-worker `mpsc` channels
   and an `AtomicUsize` round-robin counter.  Worker threads receive from their own
   private channel with zero mutex contention on the receive side.
   — **JIT async spawn** (`crates/fusec/src/native/jit.rs`): the native backend's
   `Instr::Spawn` codegen previously compiled and called the callee inline
   (synchronous).  `fuse_native_spawn_async` hostcall now dispatches callee execution
   to the task pool via `Task::spawn_async`, making JIT-backed `spawn` genuinely
   parallel.  `fuse_native_task_await` blocks on the pending task result.
2. ✓ Add structured runtime concurrency metrics:
   - active task count, total spawned, total completed
   - task completion latency histogram (5 buckets: <1 ms, 1–10 ms, 10–100 ms, 100 ms–1 s, ≥1 s)
   - spawn queue depth, mean task latency (µs), worker count
   — `crates/fusec/src/concurrency_metrics.rs` (new): all metrics as process-global
   atomics with `Ordering::Relaxed`.  `record_task_enqueued/started/completed` called
   from task pool job wrappers.  `snapshot()` returns a `ConcurrencySnapshot` struct.
3. ✓ Expose observability surface via `--diagnostics json` output and `FUSE_METRICS_HOOK=stderr`.
   — `emit_concurrency_metrics` in `crates/fusec/src/observability.rs`: emits a
   `{"event":"concurrency.snapshot",...}` NDJSON line when `--diagnostics json` is set,
   and a `metrics: {"metric":"concurrency.snapshot",...}` line when
   `FUSE_METRICS_HOOK=stderr`.  Only emits when `total_spawned > 0` to suppress noise
   for non-concurrent programs.  Called from `crates/fusec/src/cli.rs` on successful
   `--run` completion.
4. ✓ Add concurrency-focused benchmark workload to `use_case_bench.sh` (parallel-spawn CLI
   scenario).
   — `examples/spawn_bench.fuse` (new): spawns 8 parallel tasks each accumulating
   10 000 integers via `spawn`/`await`; serves as the M2 concurrency throughput
   regression baseline.
   — `scripts/use_case_bench.sh`: new `cli_spawn_bench` section measures
   `spawn_bench.fuse` with `--backend native`; output appears in both JSON metrics
   and the markdown summary table.
5. ✓ Validate structured-concurrency lifetime checks remain sound under higher task throughput.
   — All 12 `sema_golden` spawn/structured-concurrency tests pass unchanged.
   — `transaction_commits_and_rolls_back_in_native_backend` and
   `transaction_commits_and_rolls_back_in_ast_backend` integration tests pass.
   — No detached/orphaned task regressions detected.

Exit criteria:

- Parallel-spawn workload throughput improvement ≥ 20% vs `0.8.0` on reference hardware.
- Observability metrics are available in `--diagnostics json` and `FUSE_METRICS_HOOK=stderr` modes.
- No new detached/orphaned task regressions in `sema_golden` or integration tests.
- Example coverage: `examples/spawn_bench.fuse` demonstrates parallel spawn with observability.

Implementation files changed:

| File | Change |
|---|---|
| `crates/fusec/src/task_pool.rs` | Round-robin per-worker channels; job wrappers call `concurrency_metrics::record_*`; `worker_count()` helper |
| `crates/fusec/src/concurrency_metrics.rs` | New: lock-free atomic metrics, `ConcurrencySnapshot`, `snapshot()` |
| `crates/fusec/src/lib.rs` | Added `pub mod concurrency_metrics` |
| `crates/fusec/src/native/value.rs` | `TaskValue::pending: Option<Task>` field for JIT async spawn |
| `crates/fusec/src/native/jit.rs` | `fuse_native_spawn_async` hostcall; `Instr::Spawn` codegen dispatches to pool; `fuse_native_task_await` blocks on pending |
| `crates/fusec/src/native/mod.rs` | `run_native_spawn_task` made `pub(super)` |
| `crates/fusec/src/observability.rs` | `emit_concurrency_metrics` for `FUSE_METRICS_HOOK` + `--diagnostics json` |
| `crates/fusec/src/cli.rs` | Calls `emit_concurrency_metrics` after successful `--run` |
| `examples/spawn_bench.fuse` | New: 8-task parallel spawn benchmark |
| `scripts/use_case_bench.sh` | New `cli_spawn_bench` workload section |

---

### M3 — Dependency and package workflow hardening

**Status: COMPLETE** ✓ (2026-03-03)

**Goal:** Make multi-package repositories reliable and friction-free for larger project layouts.

Deliverables:

1. ✓ Harden `dep:` and `root:` import resolution for nested package structures
   (transitive dependency resolution, cycle detection across package boundaries).
   — `crates/fusec/src/manifest.rs` (new): `parse_manifest` reads all three `fuse.toml`
   dependency syntaxes; `build_transitive_deps` performs BFS expansion of each dep's own
   manifest; direct deps always shadow same-named sub-deps; cycles detected by tracking
   the active resolution chain.
   — Cycle diagnostic: `circular import: A → B → A` (full chain with `→` separators).
   — Unknown-dep diagnostic: `unknown dependency 'Foo' — available: Auth, Math`.
2. ✓ Improve `fuse check` incremental cache correctness for multi-package workspaces.
   — `CheckCache` in `crates/fusec/src/cli.rs`: per-entry-point TSV fingerprint file at
   `.fuse-cache/check-<hash>.tsv`; stores nanosecond-precision mtime for every loaded
   source file; cache hit prints `check: ok (cached, no changes)`; cache invalidated
   on any diagnostic error.
3. ✓ Add `fuse check --workspace` mode for checking all packages in a repository root.
   — `find_workspace_manifests` in `manifest.rs` walks the directory tree (skipping
   `target/`, `.git/`, hidden dirs, `node_modules/`), discovers all `fuse.toml` with a
   `[package].entry`; `run_workspace_check` checks each with per-package caching; final
   summary line with total pass/fail count.
4. ✓ Improve diagnostic quality for dependency resolution failures.
   — `unknown dependency` error now lists all declared dep names as a hint.
   — Cycle error shows the full `A → B → A` path.
5. ✓ Extend LSP dependency-root parsing coverage for additional manifest dependency syntaxes
   and manifest-change invalidation.
   — `workspace.rs` replaced 8 inline TOML helpers + ~130 lines with calls to
   `fusec::manifest::{parse_manifest, build_transitive_deps}`.
   — `WorkspaceSnapshot.manifest_mtimes`: nanosecond-precision mtime per tracked
   `fuse.toml`; `any_manifest_changed` checked at the top of every incremental update;
   detected change clears the workspace cache and triggers a full rebuild on next request.

Exit criteria:

- Multi-package layout with ≥ 3 interdependent packages passes `fuse check --workspace`
  with correct incremental cache behavior.
- LSP provides diagnostics and go-to-definition across package boundaries.
- Integration test coverage for transitive `dep:` resolution and cycle rejection.
- No regressions in single-package workflow (`project_demo`, `reference-service`).

Implementation files changed:

| File | Change |
|---|---|
| `crates/fusec/src/manifest.rs` | New: `parse_manifest`, `parse_manifest_contents`, `build_transitive_deps`, `find_workspace_manifests`, `find_workspace_root_for_entry` |
| `crates/fusec/src/lib.rs` | Added `pub mod manifest;` |
| `crates/fusec/src/loader.rs` | Both `load_program_with_modules_and_deps*` call `build_transitive_deps` first; improved diagnostic messages for unknown-dep and cycle |
| `crates/fusec/src/cli.rs` | `--workspace` flag + `run_workspace_check`; `CheckCache` with nanosecond-mtime TSV fingerprint file; cache hit/miss/invalidate paths |
| `crates/fusec/src/bin/fuse_lsp/workspace.rs` | `WorkspaceSnapshot.manifest_mtimes`; `any_manifest_changed` + `manifest_mtime` (nanosecond); replaces inline TOML helpers with `parse_manifest`; `try_incremental_module_update` checks manifest change before incremental path |
| `crates/fusec/tests/dep_resolution.rs` | New: 12 integration tests covering transitive resolution, cycle detection, CLI cache, `--workspace` mode, `find_workspace_manifests` |
| `crates/fusec/tests/lsp_workspace_incremental.rs` | Added `lsp_full_rebuild_triggered_by_manifest_change` test |

---

### M4 — LSP UX refinement for large workspaces

**Status: COMPLETE** ✓ (2026-03-02)

**Goal:** Keep LSP responsiveness within budget for workspaces significantly larger than
current test fixtures.

Deliverables:

1. ✓ Define and enforce latency budgets for core LSP operations:
   - diagnostics publish: ≤ 500 ms for incremental edits in a 50-file workspace.
   - completion response: ≤ 200 ms after keystroke in a 50-file workspace.
   - workspace symbol search: ≤ 300 ms for a 50-file workspace.
   — Budget constants added to `lsp_perf_reliability.rs` (`M4_DIAG_INCREMENTAL_BUDGET_MS`,
   `M4_COMPLETION_WARM_BUDGET_MS`, `M4_SYMBOL_SEARCH_BUDGET_MS`).
2. ✓ Implement progressive workspace indexing (index files on demand rather than
   eagerly loading all modules at startup).
   — `build_progressive_snapshot_cached()` in `workspace.rs`: builds a focus-file-only
   snapshot (focus file + its transitive imports) keyed by `(docs_revision, focus_uri)`.
   Cache invalidates automatically on every document change.
   — `workspace_diags_for_uri()` in `diagnostics.rs` now checks whether the full
   workspace cache is already warm; if not, it falls back to the progressive snapshot
   instead of triggering a full workspace build.  The full workspace is only built
   lazily when cross-file features (completion, references, workspace/symbol) are first
   requested.
   — `LspState.progressive_cache` / `progressive_builds` track the progressive snapshot.
   — `fuse/internalWorkspaceStats` now exposes `progressiveBuilds` and
   `progressiveCachePresent`.
3. ✓ Add coarse-grained index persistence across LSP restarts (avoid full re-index on
   server restart when workspace has not changed).
   — `workspace_fingerprint()`: computes a coarse fingerprint from all loaded source
   file paths + nanosecond mtimes.
   — `persist_workspace_index()` serialises the `WorkspaceIndex` to
   `.fuse-cache/lsp-index-<hash>.json` after each fresh build.
   — `load_persisted_workspace_index()` checks for a matching fingerprint on the next
   build and deserialises the cached index if valid, skipping `build_workspace_from_registry`.
   — Full JSON round-trip serialisation implemented for all `WorkspaceIndex` fields
   (`files`, `defs`, `refs`, `calls`, `module_alias_exports`, `redirects`).
   — `SymbolKind::to_u8()` / `from_u8()` added to `symbols.rs` for lossless kind
   serialisation (unlike `lsp_kind()` which is not injective).
4. ✓ Validate cancellation handling under sustained editing bursts in large workspaces
   (extend `lsp_perf_reliability` test suite).
   — `lsp_large_workspace_edit_burst_does_not_hang`: 20 rapid edit+cancellation pairs
   in a 50-file workspace; server must drain within 5 s and remain responsive.
5. ✓ Add an LSP latency regression harness gated in CI.
   — `scripts/check_lsp_latency_slo.sh`: runs `lsp_perf_reliability` with `--nocapture`;
   exits non-zero if any budget assertion fails.
   — `scripts/lsp_suite.sh` updated to 11 steps; step 11 invokes the SLO harness.

Exit criteria:

- All latency budgets met on a synthetic 50-file workspace fixture.
- `lsp_perf_reliability` test suite extended with large-workspace scenarios.
- No regressions in existing LSP behavior (`lsp_suite.sh` green).
- Progressive indexing verified: opening a single file in a large workspace does not
  block on full workspace load.

Implementation files changed:

| File | Change |
|---|---|
| `crates/fusec/src/bin/fuse_lsp/symbols.rs` | `SymbolKind::to_u8()` / `from_u8()` for lossless serialisation |
| `crates/fusec/src/bin/fuse_lsp/core.rs` | `LspState.progressive_cache`, `progressive_builds`; `invalidate_workspace_cache` clears progressive cache |
| `crates/fusec/src/bin/fuse_lsp/workspace.rs` | `build_progressive_snapshot_cached`; `build_workspace_index_cached` now loads/saves persisted index; `workspace_fingerprint`, `fingerprint_hash`, `persist_workspace_index`, `load_persisted_workspace_index`, `serialize_workspace_index`, `deserialize_workspace_index`; `workspace_stats_result` exposes new counters |
| `crates/fusec/src/bin/fuse_lsp/diagnostics.rs` | `workspace_diags_for_uri` uses progressive snapshot when full workspace cache is cold |
| `crates/fusec/tests/lsp_perf_reliability.rs` | M4 budget constants; 5 new tests: `lsp_50_file_workspace_incremental_diagnostics_within_budget`, `lsp_50_file_workspace_completion_warm_within_budget`, `lsp_50_file_workspace_symbol_search_within_budget`, `lsp_progressive_indexing_does_not_block_on_full_workspace_load`, `lsp_large_workspace_edit_burst_does_not_hang` |
| `scripts/check_lsp_latency_slo.sh` | New: CI latency SLO regression gate |
| `scripts/lsp_suite.sh` | Extended to 11 steps; step 11 runs latency SLO gate |

---

### M5 — Release automation simplification

**Status: COMPLETE** ✓ (2026-03-02)

**Goal:** Reduce manual steps and friction in the release pipeline.

Deliverables:

1. ✓ Consolidate per-platform packaging scripts into a single `scripts/package_release.sh`
   entry point that dispatches to CLI, AOT, VSIX, and container image packaging.
   — `package_release.sh` accepts `--release`, `--skip-build`, `--skip-container`,
   `--push-container`, `--platform`, `--image`, `--tag`, and `--manifest-path` flags.
   — Auto-detects host platform; silently skips container packaging on non-linux hosts.
   — Runs steps 1–5: CLI artifact → AOT artifact → VSIX → checksums/metadata → container image.
   — Derives `SOURCE_DATE_EPOCH` from git HEAD for deterministic checksum metadata when
   not set by the caller.
2. ✓ Add a `scripts/release_preflight.sh` that runs the full pre-tag checklist
   (version bump verification, changelog check, guide regeneration, authority parity,
   smoke, AOT SLO, benchmark regression) in one invocation.
   — Collects per-step pass/fail results; always prints a full summary table.
   — Exits non-zero and lists every failing gate by name for actionable diagnostics.
   — Supports `--skip-bench` (when perf artifacts unavailable) and `--skip-guide-regen`.
3. ✓ Automate version bump across all `Cargo.toml` files and `tools/vscode/package*.json`
   via a `scripts/bump_version.sh <version>` helper.
   — Uses `perl -i` for portable in-place regex (GNU and BSD `sed -i` differ in semantics).
   — Verifies `x.y.z` format before patching; rejects unexpected arguments.
   — Supports `--dry-run` to preview what would change without writing files.
   — Regenerates `Cargo.lock` via `cargo generate-lockfile` after patching crate manifests.
4. ✓ Add dry-run mode to `release-artifacts.yml` workflow for validating packaging without
   publishing.
   — `workflow_dispatch` gains a `dry_run: boolean` input (default `false`).
   — `publish`, `publish-container`, and `verify-published` jobs skip when `inputs.dry_run` is set.
   — `aggregate` job emits a Dry Run notice to the step summary explaining what was skipped.
5. ✓ Document simplified release flow in `ops/RELEASE.md`.
   — Replaced the 11-step manual checklist with a 3-script flow table (`bump_version.sh`,
   `release_preflight.sh`, `package_release.sh`) plus a 9-step step-by-step guide.
   — Documents dry-run workflow dispatch usage for validating packaging pre-publish.

Exit criteria:

- `release_preflight.sh` exits 0 on a clean release-ready tree and non-zero with
  actionable diagnostics otherwise. ✓
- `bump_version.sh 0.9.0` correctly updates all version locations. ✓
- `package_release.sh` produces all release artifacts for the host platform. ✓
- Release checklist in `ops/RELEASE.md` references new scripts. ✓

Implementation files changed:

| File | Change |
|---|---|
| `scripts/bump_version.sh` | New: version bump helper for all Cargo.toml and package*.json; `--dry-run` mode |
| `scripts/package_release.sh` | New: single packaging dispatch entry point (cli → aot → vsix → checksums → container) |
| `scripts/release_preflight.sh` | New: full pre-tag checklist runner with per-step pass/fail summary |
| `.github/workflows/release-artifacts.yml` | `dry_run` boolean input; publish/container/verify jobs gated on `!inputs.dry_run`; dry-run step summary notice |
| `ops/RELEASE.md` | Simplified to 3-script flow table + 9-step guide; documents dry-run workflow dispatch |

---

### M6 — HTML DSL ergonomics

**Status: COMPLETE** ✓ (2026-03-02)

**Goal:** Extend HTML rendering expressiveness while preserving the closed, boundary-first model.

Deliverables:

1. `component` declaration — a typed `fn`-like form with an enforced signature of
   `(attrs: Map<String, String>, children: List<Html>) -> Html`. The compiler verifies
   the signature at the declaration site and at every call site. Components are resolved
   through the existing module system; no new abstraction layer is introduced.
2. Typed attribute constraints — compile-time validation of `aria-*` attribute names and
   values on HTML elements. Unknown or misused attributes are flagged as diagnostics.
   No new constraint mechanism is introduced; `if`/`for` remain the idiomatic control
   flow inside HTML blocks.

Exit criteria:

- `component` declarations are type-checked at both declaration and call sites; invalid
  signatures are rejected with clear diagnostics.
- At least 5 `aria-*` misuse patterns produce compile-time diagnostics.
- `spec/fls.md` updated to document both additions.
- Example coverage: `examples/component_demo.fuse` demonstrates component declarations,
  typed attribute constraints, and `if`/`for` control flow inside HTML blocks.

Implementation files changed:

| File | Change |
|---|---|
| `crates/fusec/src/parser.rs` | `component` declaration parsing |
| `crates/fusec/src/ast.rs` | `ComponentDecl` AST node |
| `crates/fusec/src/sema/check.rs` | Component signature verification at declaration and call sites; `aria-*` attribute constraint checks |
| `crates/fusec/src/ir/lower.rs` | Lower component calls to typed function dispatch |
| `crates/fusec/src/interp/mod.rs` | Component eval |
| `crates/fusec/src/native/jit.rs` | JIT codegen for component dispatch |
| `spec/fls.md` | Document `component` declaration syntax and attribute constraints |
| `examples/component_demo.fuse` | New: component declarations and typed attribute constraints |

---

### M7 — DB layer enhancements

**Status: COMPLETE** ✓ (2026-03-03)

**Goal:** Apply the boundary model inward to DB outputs and complete the CRUD surface
before multi-package workloads scale.

Deliverables:

1. Typed row results — `db.from(table).select(...).all<T>()` and `.one<T>()` forms
   that apply the same struct-decode/validation pipeline used at HTTP boundaries to DB
   output rows. The type parameter `T` must be a declared `type` whose field names match
   the selected columns; mismatches are caught at compile time. Architecturally consistent
   with the principle of pushing boundary validation inward.
2. `db.from(...).upsert(struct)` — companion to `.insert()` using INSERT OR REPLACE
   semantics. Accepts the same struct argument shape as `.insert()`. Fills the last
   significant gap in the CRUD surface that currently requires falling back to raw
   `db.exec()` with hand-written SQL.
3. Migration namespacing — `__fuse_migrations` table extended from a single `name TEXT`
   primary key to a composite `(package TEXT, name TEXT)` primary key. Package name is
   sourced from `fuse.toml`; defaults to an empty string for single-package projects,
   preserving full backward compatibility with existing migration records. Required before
   multi-package usage scales beyond the `reference-service` pattern.

Exit criteria:

- `db.from(...).all<T>()` produces correctly typed values for all matching field types;
  column/field mismatches produce compile-time diagnostics.
- `upsert` passes parity tests against both interpreter and native backends.
- Migration namespacing is backward-compatible: existing single-package projects run
  without re-running migrations.
- Integration test coverage for typed queries, upsert, and multi-package migration
  namespace isolation.
- `spec/fls.md` and `spec/runtime.md` updated to document all three additions.

Implementation files changed:

| File | Change |
|---|---|
| `crates/fusec/src/db.rs` | `upsert_struct` method; typed row extraction helpers; `Query::all_typed` / `one_typed` |
| `crates/fusec/src/sema/check.rs` | Type-parameterised `.all<T>()` / `.one<T>()` checking; `upsert` method signature in `lookup_query_member` |
| `crates/fusec/src/interp/mod.rs` | `query.upsert`, `query.all_typed`, and `query.one_typed` builtin dispatch |
| `crates/fusec/src/native/jit.rs` | `fuse_native_query_upsert`, `fuse_native_query_all_typed`, `fuse_native_query_one_typed` hostcalls |
| `crates/fusec/src/loader.rs` | Migration schema bootstrap checks for composite `(package, name)` primary key; migration runner receives and passes package name |
| `spec/fls.md` | Document typed query result forms and `upsert` |
| `spec/runtime.md` | Document migration namespace schema change and backward-compat guarantee |
| `examples/native_db.fuse` | Updated: demonstrate typed query results and `upsert` |

---

### M8 — Pre-tag cleanup, docs, and release

**Goal:** Final integration, documentation, and release cut.

Deliverables:

1. Update `governance/scope.md` roadmap section (move 0.9.0 items to "Completed",
   update next priorities).
2. Update `governance/LSP_ROADMAP.md` with any newly landed LSP capabilities.
3. Update `governance/VERSIONING_POLICY.md` with `0.9.0` release-line notes.
4. Write `CHANGELOG.md` entries for all milestone deliverables.
5. Regenerate guide docs (`scripts/generate_guide_docs.sh`).
6. Run full release checklist per `ops/RELEASE.md`.
7. Tag `v0.9.0`.

Exit criteria:

- All milestone exit criteria met.
- `release_smoke.sh` and `authority_parity.sh` pass.
- `CHANGELOG.md`, `scope.md`, `VERSIONING_POLICY.md`, and `LSP_ROADMAP.md` updated.
- Tag `v0.9.0` pushed; release artifacts published.

---

## Milestone sequencing

```
M1 (native perf) ──────┐
                        ├──▶ M3 (package workflow) ──▶ M5 (release automation) ──▶ M6 (HTML DSL) ──┐
M2 (concurrency) ──────┘          │                                                                  ├──▶ M8 (release)
                                  ▼                                               M7 (DB layer) ─────┘
                           M4 (LSP scalability)
```

- **M1** and **M2** are independent and can proceed in parallel.
- **M3** depends on M1/M2 stabilizing runtime internals before hardening cross-package workflows.
- **M4** can begin alongside M3 (LSP work is largely independent) but should incorporate
  M3's dependency-parsing changes before closing.
- **M5** can begin as soon as M1–M4 are feature-complete.
- **M6** (HTML DSL) and **M7** (DB layer) are independent of each other and can proceed in
  parallel after M5. M7 should incorporate M3's multi-package plumbing before closing
  (migration namespacing depends on the package-name plumbing from M3).
- **M8** is the final integration gate; requires M6 and M7 to be feature-complete.

---

## Risk items

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Native perf changes cause semantic divergence | Release blocker | Parity gates run on every PR; `authority_parity.sh` is CI-enforced |
| Progressive LSP indexing regresses small-workspace UX | User-facing regression | Existing `lsp_suite.sh` + `lsp_ux` tests remain green; latency budgets apply to both small and large workspaces |
| Multi-package cache invalidation edge cases | Incorrect builds | Golden-test coverage for cross-package scenarios; `fuse check --workspace` exercises full graph |
| Release automation scripts mask failures | Bad release | `release_preflight.sh` must exit non-zero on any sub-step failure; dry-run mode validates without publishing |
| `component` signature enforcement breaks existing `fn`-returning-`Html` patterns | Source-level breakage | `component` is a new keyword; existing `fn` declarations are unaffected; no coercion between `fn` and `component` call sites |
| Typed DB query column/field mismatch diagnostics are overly strict at runtime | Developer friction | Mismatch is a compile-time error only when type parameter is explicit; unparameterised `.all()` retains current `List<Map<String, String>>` behaviour |
| Migration namespace schema change breaks existing deployments | Data loss / re-run migrations | Migration runner detects old single-key schema and runs a non-destructive `ALTER TABLE` migration before any user migrations execute; covered by integration tests |
