# FUSE Versioning Policy

This document defines non-negotiable versioning and compatibility rules for FUSE language,
runtime, and tooling evolution.

Companion references:

- `../spec/fls.md` for syntax and static semantics
- `../spec/runtime.md` for runtime/boundary semantics
- `../README.md` for release-facing stability contract
- `../ops/RELEASE.md` for release execution steps
- `../ops/AOT_RELEASE_CONTRACT.md` for AOT production contract and SLO targets (`v0.4.0` rollout)

## Versioned surfaces

### 1) Language contract (`STABLE`)

Owned by:

- `../spec/fls.md`
- `../spec/runtime.md`

This is what users write and rely on (syntax, type behavior, boundary semantics, error mapping).

### 2) Tooling contract (`STABLE`)

Owned by:

- `fuse` CLI behavior and flags
- `fuse.toml` user-facing manifest fields
- documented release scripts in `../README.md` / `../ops/RELEASE.md`

### 3) Compiler/runtime internals (`INTERNAL`)

Not a compatibility surface:

- `fusec` Rust internal modules and APIs
- IR/bytecode/native internal representation details
- `.fuse/build/*` cache artifact formats
- internal JIT/runtime ABI details

Internal surfaces may change between releases without compatibility guarantees.

## Version scheme

FUSE uses SemVer tags (`MAJOR.MINOR.PATCH`) with explicit pre-1.0 policy:

- `PATCH` (for the active `0.x` line, e.g. `0.3.x`): no breaking changes to language or tooling contracts.
- `MINOR` (pre-1.0, e.g. `0.3.0`): breaking changes allowed only with migration notes.
- `MAJOR` (1.0+): required for breaking changes.

Recent release-line notes:

- `0.2.0` is an explicitly breaking minor that reset parts of the pre-1.0 contract:
  - task helper API removal (`task.id/done/cancel`)
  - `spawn` execution and restriction semantics
  - build cache metadata schema bump (`program.meta` v3)
  - VS Code packaging artifact change (`.tgz` -> `.vsix`)
- `0.3.0` is a quality/stability minor that keeps `0.2.x` source compatibility while expanding
  parity/tooling/release artifact reliability coverage.

- semantic regressions across AST/VM/native are release blockers on all active release lines.

## Compatibility guarantees

### Source compatibility

- Programs valid on `0.1.0` must remain valid and equivalent on `0.1.x`.
- Programs valid on `0.2.0` must remain valid and equivalent on `0.2.x`.
- Programs valid on `0.3.0` must remain valid and equivalent on `0.3.x`.
- If behavior must change incompatibly, release must bump at least `MINOR` and include migration guidance.

### Runtime behavior compatibility

- Error/status/JSON/boundary behavior documented in `../spec/runtime.md` is part of the stable contract.
- Backend divergence (AST vs VM vs native) is a correctness bug.

### Cache/binary compatibility

- `.fuse/build/program.ir` and `.fuse/build/program.native` are cache artifacts, not portability contracts.
- Cross-version cache compatibility is not guaranteed; rebuild on version changes is expected.
- `0.2.0` intentionally invalidates `0.1.x` build cache metadata.
- `0.3.0` continues the `program.meta` v3 cache contract.

## Deprecation policy

A contract-facing change must follow all steps:

1. Introduce deprecation:
   - document in `CHANGELOG.md` and the relevant spec doc (`../spec/fls.md` or `../spec/runtime.md`)
   - provide replacement path
2. Deprecation window:
   - keep old behavior for at least one `MAJOR` cycle (1.0+)
   - pre-1.0 (`0.x`) breaking minor releases may remove behavior without a full deprecation window,
     but must include:
     - explicit breaking notes in `CHANGELOG.md`
     - compiler/runtime diagnostics with migration hints where possible
     - a concrete migration guide under `docs/migrations/`
3. Removal:
   - allowed only on the next breaking release boundary
   - include migration notes and before/after examples

No silent removals of user-facing language/runtime/tooling behavior are allowed, including pre-1.0.

## Release gate requirements

Any release that changes contract-facing behavior must include:

1. Spec updates:
   - `../spec/fls.md` and/or `../spec/runtime.md`
   - `README.md` stability notes if scope changed
2. Changelog entries:
   - explicit `Added`, `Changed`, and `Breaking` (when applicable)
3. Semantic and parity gates:
   - `./scripts/semantic_suite.sh`
   - `./scripts/authority_parity.sh`
   - `./scripts/release_smoke.sh`
4. Migration notes for breaking releases:
   - what changed
   - who is affected
   - exact migration steps

## Non-negotiable rules

1. No breaking language/runtime/tooling changes in patch releases.
2. No undocumented contract-facing behavior changes.
3. No backend-specific semantic divergence.
4. No removal without deprecation window and migration guidance.
