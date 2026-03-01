# RFC 0001: AOT Codegen Pipeline

- Status: Implemented
- Authors: FUSE maintainers
- Created: 2026-02-22
- Updated: 2026-02-22
- Related PRs: N/A (landed with v0.4.0 milestone)
- Related issues: v0.4.0 AOT production rollout

## Summary

Define the AOT code generation pipeline as a deterministic compilation flow from canonical
frontend artifacts to deployable executables.

## Motivation

FUSE needs a production deployment path that removes runtime compilation while preserving
current semantic authority and backend parity constraints.

## Non-goals

- introducing new language semantics
- removing native-JIT development workflows
- exposing internal IR/object formats as public contracts

## Detailed design

1. AOT consumes canonical frontend outputs already used by native paths.
2. AOT lowers through the existing internal IR/lowering model and emits object code.
3. Object emission must be deterministic for identical source + toolchain + target inputs.
4. Final executable generation is a build-time step; no runtime JIT path is required in deployed binaries.
5. Cache artifacts remain internal and are not a portability/stability contract.

## Alternatives considered

1. Keep JIT-only native execution.
This does not satisfy deployment/startup predictability goals.

2. Introduce a new AOT-only semantic pipeline.
This violates the semantic authority model and raises divergence risk.

## Compatibility and migration

- Backward compatible for language/runtime behavior.
- Tooling changes are additive (`--aot` build mode).
- Existing JIT-native workflows continue to function.

## Test plan

- semantic suite and authority parity suite remain mandatory
- add AOT parity coverage once AOT execution path is implemented
- add deterministic build reproducibility checks in CI

## Documentation updates

- `ops/AOT_RELEASE_CONTRACT.md`
- `README.md` (when CLI surface ships)
- `spec/runtime.md` (when runtime backend surface is updated)

## Risks

- increased build complexity
- object/link determinism drift across toolchains

Mitigation:

- pinned toolchain policy
- deterministic ordering requirements in object emission

## Rollout plan

1. Land decision contracts (this RFC + linked RFCs).
2. Implement dual-mode build surface.
3. Add cross-target CI gates and release integration.

## Decision log

- 2026-02-22: Proposed
- 2026-02-22: Accepted
