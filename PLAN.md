# FUSE 1.0.0 Stability Plan

Status: Active (`M2` in progress, `M3` next)  
Source contract: `spec/1.0.0_STABILITY_CONTRACT.md`

This plan defines milestones to satisfy all `v1.0.0` entry criteria and keep docs clean during
execution and at release cut.

## Execution rules

1. Contract-facing code changes and doc updates ship in the same PR.
2. Each milestone must leave the repo in a green gate state for affected suites.
3. Non-goals in the stability contract remain enforced unless maintainers explicitly revise scope.

## Milestone status (as of 2026-02-23)

| Milestone | Status | Notes |
|---|---|---|
| `M0` | Pending | Tracking artifacts still need to be formalized as deliverables. |
| `M1` | Pending | Full contract-test expansion not started. |
| `M2` | In progress | Canonical reference service migration is mostly complete; remaining replacement/operational parity items still open. |
| `M3` | Next | Start with runtime/spec contract for request IDs, structured logs, panic classes, metrics hook after `M2` closure. |
| `M4` | Pending | Depends on `M3` contract clarity for operational defaults rollout. |
| `M5` | Ongoing | Continuous sync has been applied during `M2`; final sweep still pending. |
| `M6` | Pending | Release-candidate gate run and acceptance update are pending. |

`M2` gate evidence captured on 2026-02-23:

1. `./scripts/semantic_suite.sh` (`PASS`)
2. `./scripts/authority_parity.sh` (`PASS`)
3. `./scripts/lsp_suite.sh` (`PASS`)
4. `./scripts/reliability_repeat.sh --iterations 2` (`PASS`; local sandbox run failed only due host bind restrictions, rerun outside sandbox passed)
5. `./scripts/check_aot_perf_slo.sh` (`PASS`)
6. `./scripts/packaging_verifier_regression.sh` (`PASS`)

Remaining `M2` closure items:

1. Complete full package replacement parity in docs/workflows (including explicit run/test/build/AOT usage docs for `examples/reference-service`).
2. Ensure build/AOT validation path is runnable in release environments with SCSS tooling (`sass`) available and documented.
3. Finalize and verify replacement cleanup details against all `M2` deliverables and exit criteria before marking `M2` complete.

## Milestones

### M0: Baseline audit and tracking

Deliverables:

1. Gap matrix mapping every contract criterion to current implementation status.
2. Ownership map for each criterion (compiler/runtime/tooling/docs).
3. Tracking checklist issue with links to milestone PRs.

Exit criteria:

1. All contract clauses are mapped to a concrete implementation task.
2. Blocking unknowns are resolved or explicitly deferred with owner/date.

### M1: Freeze semantics with contract tests

Deliverables:

1. Expand parser fixtures (`crates/fusec/tests/parser_fixtures.rs`) for frozen syntax cases.
2. Expand semantic golden tests (`crates/fusec/tests/sema_golden.rs`) for frozen static/runtime
   boundary behavior.
3. Add/update runtime tests for error envelope, status mapping, config precedence, JSON behavior,
   and dependency/lockfile determinism.

Exit criteria:

1. `./scripts/semantic_suite.sh` passes.
2. `./scripts/authority_parity.sh` passes.
3. No known AST/VM/native/AOT contract-facing divergence remains open.

### M2: Canonical reference service

Deliverables:

1. Create `examples/reference-service/` with:
   - auth
   - CRUD
   - validation failure paths
   - DB migrations
   - deterministic `spawn` usage
   - structured logging mode
   - OpenAPI enabled route
2. Add service validation scripts/docs for run/test/build/AOT paths.
3. Produce deployable AOT artifact for the reference service.
4. Migrate all package-level example workflows from legacy canonical examples to
   `examples/reference-service/`.
5. Remove replaced canonical package examples after migration.
6. Remove redundant examples that duplicate covered behavior without adding unique contract value.

Exit criteria:

1. Reference service is used in CI/regression gates where appropriate.
2. Service passes semantic/parity/LSP/reliability/AOT/packaging checks.
3. No user-facing workflow points to removed canonical examples.
4. Remaining examples each have a unique scope statement in `examples/README.md`.

### M3: Observability baseline

Kickoff sequence (next step):

1. Define observability contract in `spec/runtime.md`:
   - request ID propagation source/precedence and response emission rules
   - structured request logging mode and stable field set
   - deterministic panic classification taxonomy for fatal envelopes
   - metrics hook shape, guarantees, and non-goals
2. Add runtime tests for all contract clauses before implementation broadening.
3. Implement minimal runtime plumbing to satisfy contract and tests.
4. Update docs in the same PR (`README.md`, `guides/fuse.md`, `spec/runtime.md`).

Deliverables:

1. Request ID propagation for HTTP request lifecycle.
2. Optional structured request logging mode.
3. Deterministic panic classification aligned with fatal envelope contract.
4. Minimal metrics hook extension point (non-semantic).

Exit criteria:

1. Observability behavior is tested and documented.
2. No behavioral regression in existing logging/error paths.

### M4: AOT release-default behavior

Deliverables:

1. Implement `fuse build --release` => AOT by default.
2. Preserve non-AOT local development path and document explicit usage.
3. Update build/test/release tooling to reflect new default behavior.

Exit criteria:

1. CLI tests cover both default AOT and explicit non-AOT paths.
2. AOT metadata/fatal envelope contract remains unchanged.

### M5: Documentation sync (continuous + final sweep)

Deliverables during each milestone PR:

1. Update impacted docs immediately, not post hoc.
2. Keep command examples and behavior statements aligned with actual CLI/runtime behavior.

Final sweep deliverables before `v1.0.0` tag:

1. Reconcile and update:
   - `README.md`
   - `guides/fuse.md`
   - `spec/fls.md`
   - `spec/runtime.md`
   - `governance/scope.md`
2. Regenerate docs site reference:
   - `./scripts/generate_guide_docs.sh`
   - verify `docs/site/specs/reference.md` is current
3. Run a repository trace check for removed canonical examples and clear all remaining references
   in docs/scripts/tests/benchmarks.

Exit criteria:

1. No contradictory contract statements across docs.
2. Release-facing docs reflect the accepted behavior and gates.
3. Removed canonical examples have no remaining references in active documentation/workflows.

### M6: Release candidate and acceptance

Deliverables:

1. Run full release gates:
   - `./scripts/semantic_suite.sh`
   - `./scripts/authority_parity.sh`
   - `./scripts/lsp_suite.sh`
   - `./scripts/reliability_repeat.sh --iterations 2`
   - `./scripts/check_aot_perf_slo.sh`
   - `./scripts/packaging_verifier_regression.sh`
   - `./scripts/release_smoke.sh`
2. Resolve or explicitly waive remaining issues with maintainer sign-off.
3. Update contract status from `Proposed` to `Accepted` at `v1.0.0`.

Exit criteria:

1. All gates pass on release candidate commit.
2. `spec/1.0.0_STABILITY_CONTRACT.md` is accepted and published with release notes.
