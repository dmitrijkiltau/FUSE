# RFC 0002: AOT Link and Runtime Composition Model

- Status: Implemented
- Authors: FUSE maintainers
- Created: 2026-02-22
- Updated: 2026-02-22
- Related PRs: N/A (landed with v0.4.0 milestone)
- Related issues: v0.4.0 AOT production rollout

## Summary

Define how AOT executables are linked and how runtime support is composed into production binaries.

## Motivation

AOT requires explicit ownership of link/runtime composition decisions that are currently implicit
in JIT-native development flow.

## Non-goals

- exposing internal runtime ABI as public stable API
- mandating one static-link strategy for every target

## Detailed design

1. AOT output is a standalone executable that embeds required FUSE runtime support.
2. Default link mode uses target-appropriate system linkage for compatibility and predictable CI.
3. Optional fully static profiles may be offered only where platform/toolchain constraints allow.
4. Runtime data required for execution (config/type metadata and entry symbols) is embedded at build time.
5. Deployed executables must not depend on runtime JIT compilation.

## Alternatives considered

1. Dynamic plugin/runtime loading model.
Rejected due to deploy complexity and weaker portability guarantees.

2. Force full static linking on all targets.
Rejected because it is not uniformly feasible across supported targets.

## Compatibility and migration

- additive tooling change
- no source-language migration required
- deployment workflows may opt in incrementally

## Test plan

- executable smoke tests per target
- startup and runtime conformance tests on produced binaries
- packaging verification checks include AOT artifacts

## Documentation updates

- `ops/AOT_RELEASE_CONTRACT.md`
- `ops/RELEASE.md` (release artifact expectations)

## Risks

- platform linker variance
- runtime composition regressions

Mitigation:

- explicit per-target policy (RFC 0003)
- release matrix verification gates

## Rollout plan

1. finalize link/runtime policy
2. implement `--aot` build path
3. enforce artifact verification in release workflow

## Decision log

- 2026-02-22: Proposed
- 2026-02-22: Accepted
