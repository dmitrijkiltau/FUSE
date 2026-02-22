# AOT Production Backend Plan (JIT -> AOT)

Status: Draft  
Goal: make AOT a production-grade deployment backend without regressing language/runtime semantics.

## Why this exists

FUSE already has:

- canonical AST authority and backend parity intent
- VM + native backend paths
- cache validation/fingerprint infrastructure

FUSE does not yet provide a complete production AOT story:

- standalone service binaries as the primary production output
- cross-platform AOT artifact matrix with CI enforcement
- explicit operational guarantees (startup, reproducibility, deploy model)

This plan focuses on deployment/runtime operational outcomes, not language feature expansion.

## Scope and non-goals

In scope:

- `fuse build --aot` release path
- production-ready AOT artifacts and CI/release integration
- parity and benchmark validation against existing VM/native semantics

Out of scope:

- changing language semantics for AOT
- removing JIT from development workflows
- adding unrelated language features (generics/macros/etc.)

## Success criteria (program-level)

1. AOT artifacts are deterministic and reproducible for supported targets.
2. AOT runtime behavior is semantically equivalent to VM/native on contract suites.
3. Cold start/startup metrics improve measurably vs JIT-native execution.
4. Production release pipelines publish and verify AOT artifacts across supported platforms.

## Milestones

### Milestone 0: Architecture Decisions and Contract

Objective: lock decisions before implementation spread.

Status: Implemented (2026-02-22)

Deliverables:

- ADR/RFC set for:
  - codegen model (IR -> object -> final binary)
  - linker/runtime model (static vs dynamic components)
  - platform ABI and libc policy per target
  - crash/panic/reporting behavior in production binaries
  - debug symbol/profile strategy (`debug`, `release`, `release-with-symbols`)
- AOT semantic contract statement: AOT is an execution strategy, not a semantic authority.
- Production SLO targets:
  - cold start budget
  - binary size budget by platform class
  - build-time budget in CI

Exit criteria:

- all architecture decisions are documented and accepted
- measurable targets are defined and versioned

Implementation artifacts:

- `AOT_CONTRACT.md`
- `rfcs/0001-aot-codegen-pipeline.md`
- `rfcs/0002-aot-link-runtime-model.md`
- `rfcs/0003-aot-platform-abi-policy.md`
- `rfcs/0004-aot-crash-and-panic-policy.md`
- `rfcs/0005-aot-build-profiles-and-symbols.md`
- `rfcs/0006-aot-semantic-contract-and-slos.md`

### Milestone 1: Dual-Mode Build Surface

Objective: add AOT as opt-in production mode while preserving fast dev iteration.

Status: Implemented (2026-02-22)

Deliverables:

- CLI surface:
  - `fuse build --aot`
  - `fuse build --release --aot`
- Mode contract:
  - JIT/native path remains first-class for dev/debug loops
  - AOT is release/deployment path
- Artifact conventions documented (cache artifact vs deployable artifact naming/locations).
- Failure model documented: deterministic failures and diagnostics when AOT build/link cannot complete.

Validation:

- CLI contract tests for flags, output paths, failure behavior.
- Packaging smoke checks include AOT flag path.

Exit criteria:

- AOT builds can be produced locally and in CI for at least one host platform
- JIT workflow remains unchanged for `fuse run`/`fuse dev`

Implementation artifacts:

- CLI flags in `crates/fuse/src/main.rs`:
  - `fuse build --aot`
  - `fuse build --aot --release`
  - validation: `--release` requires `--aot`
- AOT default output contract:
  - `.fuse/build/program.aot` (`.exe` on Windows)
  - `[build].native_bin` remains explicit override
- CLI contract tests:
  - `crates/fuse/tests/project_cli.rs`
- Release smoke coverage of AOT build path:
  - `scripts/release_smoke.sh`

### Milestone 2: AOT Backend Core Implementation

Objective: produce standalone deployable binaries with VM-compatible semantics.

Status: Implemented (2026-02-22)

Deliverables:

- AOT compiler pipeline integrated into `fuse build --aot`.
- Runtime embedding/linking strategy implemented for standalone binaries.
- Build metadata embedded:
  - compiler/runtime version
  - target triple
  - semantic contract version
- Error/panic handling path hardened for production binaries.

Validation:

- unit/integration tests for codegen/link/load execution.
- parity tests between VM/native(AOT disabled) and AOT outputs.

Exit criteria:

- AOT binary runs without requiring Rust toolchain on target host
- semantic parity suite passes for AOT on primary host platform

Implementation artifacts:

- Standalone AOT build path and direct binary execution:
  - `crates/fuse/src/main.rs`
  - `scripts/release_smoke.sh`
- Embedded AOT build metadata:
  - `target`, `rustc`, `cli`, `runtime_cache`, `contract`
  - surfaced via `FUSE_AOT_BUILD_INFO=1`
- Hardened fatal handling:
  - stable fatal envelopes for `runtime_fatal` and `panic`
- Integration tests that run produced AOT binaries directly:
  - `crates/fuse/tests/project_cli.rs`

### Milestone 3: Cross-Platform and Reproducible Artifacts

Objective: make AOT production artifacts publishable across official targets.

Deliverables:

- AOT release matrix support:
  - `linux-x64`
  - `macos-arm64`
  - `windows-x64`
- Reproducibility policy:
  - pinned toolchain components
  - stable build metadata and checksum generation
  - documented non-determinism sources and mitigations
- Optional static binary profile where feasible (documented platform constraints).

Validation:

- CI matrix builds AOT artifacts on all supported targets.
- Release metadata/checksum pipeline includes AOT artifacts.

Exit criteria:

- release workflow emits verified AOT bundles for all supported targets
- reproducibility checks pass within defined tolerances

### Milestone 4: Performance, Reliability, and Operability Hardening

Objective: prove operational value and production safety.

Deliverables:

- benchmark suite extension:
  - JIT-native vs AOT cold start
  - startup latency distribution (p50/p95/p99)
  - steady-state throughput where relevant
- observability hooks for production diagnosis (startup mode, version/build info, crash context).
- rollback-ready deployment guidance (AOT primary, JIT/native fallback in incident playbooks).

Validation:

- benchmark jobs in CI (or scheduled perf pipeline) with trend tracking.
- reliability repeat runs include AOT path.

Exit criteria:

- startup improvement is measurable and documented
- no unresolved severity-1 parity/reliability issues

### Milestone 5: Complete v0.4.0 Release

Objective: ship `v0.4.0` with AOT production backend policy and verified release artifacts.

Deliverables:

- release scope freeze and final go/no-go review for `v0.4.0`
- policy update merged:
  - AOT designated production backend
  - JIT/native designated dev/debug backend
- release documentation complete:
  - `README.md` build/deploy sections
  - `runtime.md` backend behavior notes
  - `scope.md` roadmap updates
  - release/operator notes and upgrade guidance
- release pipeline execution:
  - AOT parity gate required and green
  - AOT artifact matrix required and green
  - checksums + release metadata generated and verified
- tagged and published `v0.4.0` artifacts for supported targets

Exit criteria (go/no-go):

1. `v0.4.0` tag and release artifacts are published
2. no semantic divergence against authoritative suites
3. startup/cold-start improvement meets target thresholds
4. release artifact pipeline supports AOT mode on all supported targets
5. rollback path is documented and tested

## Program risks and mitigations

1. Compile times increase.  
Mitigation: cache strategy, incremental compilation, split debug/release profiles.

2. Build pipeline complexity increases.  
Mitigation: explicit build graph ownership, CI stage isolation, deterministic metadata.

3. Cross-platform burden increases.  
Mitigation: strict target matrix ownership, per-target smoke tests, artifact verification gates.

4. Debuggability regressions.  
Mitigation: symbol strategy, structured crash output, reproducible debug builds.

## Tracking

Suggested status labels per milestone:

- `not started`
- `in progress`
- `blocked`
- `done`

Suggested review cadence:

- weekly milestone review
- release-gate review before changing default backend policy
