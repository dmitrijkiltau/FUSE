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

**Goal:** Measurable latency and throughput improvements for native-backend execution, targeting
the most exercised runtime paths in CLI and HTTP workloads.

Deliverables:

1. Profile and identify top-10 hot paths in native IR execution (using `aot_perf_bench.sh` and
   `use_case_bench.sh` workloads as baseline).
2. Reduce dispatch overhead for high-frequency native call targets (member calls, builtins,
   boundary-validation entry points).
3. Lower allocation pressure in native JSON encode/decode and validation paths.
4. Add cold-start and steady-state throughput regression gates for the changes
   (`check_aot_perf_slo.sh` threshold tightening).
5. Update `benchmarks/use_case_baseline.json` with new post-optimization baselines.

Exit criteria:

- `p50` cold-start improvement ≥ 10% vs `0.8.0` baseline on `project_demo` AOT workload.
- `p50` steady-state CLI throughput improvement ≥ 15% vs `0.8.0` baseline.
- All existing parity gates (`authority_parity.sh`) pass — no semantic divergence introduced.
- Benchmark results documented in `CHANGELOG.md`.

---

### M2 — Concurrency throughput & observability

**Goal:** Improve throughput of the deterministic `spawn`/`await` model and add runtime
observability primitives for concurrent workloads.

Deliverables:

1. Reduce per-task scheduling overhead in the `spawn` runtime (task queue contention,
   wakeup latency).
2. Add structured runtime concurrency metrics:
   - active task count
   - task completion latency histogram
   - spawn queue depth
3. Expose observability surface via `fuse dev` overlay and `--diagnostics json` output.
4. Add concurrency-focused benchmark workload to `use_case_bench.sh` (parallel-spawn CLI
   scenario).
5. Validate structured-concurrency lifetime checks remain sound under higher task throughput.

Exit criteria:

- Parallel-spawn workload throughput improvement ≥ 20% vs `0.8.0` on reference hardware.
- Observability metrics are available in `fuse dev` and `--diagnostics json` modes.
- No new detached/orphaned task regressions in `sema_golden` or integration tests.
- Example coverage: update or add a concurrency-focused example demonstrating observability.

---

### M3 — Dependency and package workflow hardening

**Goal:** Make multi-package repositories reliable and friction-free for larger project layouts.

Deliverables:

1. Harden `dep:` and `root:` import resolution for nested package structures
   (transitive dependency resolution, cycle detection across package boundaries).
2. Improve `fuse check` incremental cache correctness for multi-package workspaces
   (cross-package invalidation on manifest or export-shape changes).
3. Add `fuse check --workspace` mode for checking all packages in a repository root.
4. Improve diagnostic quality for dependency resolution failures (missing dep, version
   mismatch, circular import paths).
5. Extend LSP dependency-root parsing coverage for additional manifest dependency syntaxes
   (per `LSP_ROADMAP.md` planned item).

Exit criteria:

- Multi-package layout with ≥ 3 interdependent packages passes `fuse check --workspace`
  with correct incremental cache behavior.
- LSP provides diagnostics and go-to-definition across package boundaries.
- Integration test coverage for transitive `dep:` resolution and cycle rejection.
- No regressions in single-package workflow (`project_demo`, `reference-service`).

---

### M4 — LSP UX refinement for large workspaces

**Goal:** Keep LSP responsiveness within budget for workspaces significantly larger than
current test fixtures.

Deliverables:

1. Define and enforce latency budgets for core LSP operations:
   - diagnostics publish: ≤ 500 ms for incremental edits in a 50-file workspace.
   - completion response: ≤ 200 ms after keystroke in a 50-file workspace.
   - workspace symbol search: ≤ 300 ms for a 50-file workspace.
2. Implement progressive workspace indexing (index files on demand rather than
   eagerly loading all modules at startup).
3. Add coarse-grained index persistence across LSP restarts (avoid full re-index on
   server restart when workspace has not changed).
4. Validate cancellation handling under sustained editing bursts in large workspaces
   (extend `lsp_perf_reliability` test suite).
5. Add an LSP latency regression harness gated in CI.

Exit criteria:

- All latency budgets met on a synthetic 50-file workspace fixture.
- `lsp_perf_reliability` test suite extended with large-workspace scenarios.
- No regressions in existing LSP behavior (`lsp_suite.sh` green).
- Progressive indexing verified: opening a single file in a large workspace does not
  block on full workspace load.

---

### M5 — Release automation simplification

**Goal:** Reduce manual steps and friction in the release pipeline.

Deliverables:

1. Consolidate per-platform packaging scripts into a single `scripts/package_release.sh`
   entry point that dispatches to CLI, AOT, VSIX, and container image packaging.
2. Add a `scripts/release_preflight.sh` that runs the full pre-tag checklist
   (version bump verification, changelog check, guide regeneration, authority parity,
   smoke, AOT SLO, benchmark regression) in one invocation.
3. Automate version bump across all `Cargo.toml` files and `tools/vscode/package*.json`
   via a `scripts/bump_version.sh <version>` helper.
4. Add dry-run mode to `release-artifacts.yml` workflow for validating packaging without
   publishing.
5. Document simplified release flow in `ops/RELEASE.md`.

Exit criteria:

- `release_preflight.sh` exits 0 on a clean release-ready tree and non-zero with
  actionable diagnostics otherwise.
- `bump_version.sh 0.9.0` correctly updates all version locations.
- `package_release.sh` produces all release artifacts for the host platform.
- Release checklist in `ops/RELEASE.md` references new scripts.

---

### M6 — Pre-tag cleanup, docs, and release

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
                        ├──▶ M3 (package workflow) ──▶ M5 (release automation) ──▶ M6 (release)
M2 (concurrency) ──────┘          │
                                  ▼
                           M4 (LSP scalability)
```

- **M1** and **M2** are independent and can proceed in parallel.
- **M3** depends on M1/M2 stabilizing runtime internals before hardening cross-package workflows.
- **M4** can begin alongside M3 (LSP work is largely independent) but should incorporate
  M3's dependency-parsing changes before closing.
- **M5** can begin as soon as M1–M4 are feature-complete.
- **M6** is the final integration gate.

---

## Risk items

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Native perf changes cause semantic divergence | Release blocker | Parity gates run on every PR; `authority_parity.sh` is CI-enforced |
| Progressive LSP indexing regresses small-workspace UX | User-facing regression | Existing `lsp_suite.sh` + `lsp_ux` tests remain green; latency budgets apply to both small and large workspaces |
| Multi-package cache invalidation edge cases | Incorrect builds | Golden-test coverage for cross-package scenarios; `fuse check --workspace` exercises full graph |
| Release automation scripts mask failures | Bad release | `release_preflight.sh` must exit non-zero on any sub-step failure; dry-run mode validates without publishing |
