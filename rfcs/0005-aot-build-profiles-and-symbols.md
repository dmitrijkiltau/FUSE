# RFC 0005: AOT Build Profiles and Debug Symbol Strategy

- Status: Implemented
- Authors: FUSE maintainers
- Created: 2026-02-22
- Updated: 2026-02-22
- Related PRs: N/A (landed with v0.4.0 milestone)
- Related issues: v0.4.0 AOT production rollout

## Summary

Define the supported build profile set for AOT outputs and symbol-handling policy.

## Motivation

AOT production use requires predictable tradeoffs between debuggability, startup performance,
binary size, and CI build time.

## Non-goals

- exposing compiler internals as stable user APIs
- guaranteeing identical binary size across all toolchains

## Detailed design

Required profile set:

1. `debug`: maximal diagnostics, not production-grade.
2. `release`: production default, stripped/minimized where appropriate.
3. `release-with-symbols`: production-equivalent optimization with external symbol retention
   for crash forensics.

Policy:

1. `release` is the default profile for published AOT artifacts.
2. `release-with-symbols` artifacts are generated for internal debugging and incident response.
3. Symbol files must be traceable to published binary checksums/build metadata.
4. Profile behavior and flags must be documented and versioned.

## Alternatives considered

1. Single release profile only.
Rejected because it weakens postmortem diagnosis.

2. Debug profile for production.
Rejected due to startup/size/perf unpredictability.

## Compatibility and migration

- additive to existing tooling contract
- no source migration required

## Test plan

- profile-specific build checks in CI
- verify `release` and `release-with-symbols` outputs are functionally equivalent
- verify symbol artifact generation and mapping metadata

## Documentation updates

- `ops/AOT_RELEASE_CONTRACT.md`
- `README.md` build/distribution section
- `ops/RELEASE.md` release checklist

## Risks

- profile drift causing hard-to-debug production failures

Mitigation:

- explicit profile contract and CI assertions

## Rollout plan

1. define profile contract
2. implement profile outputs in AOT pipeline
3. enforce profile checks in release workflows

## Decision log

- 2026-02-22: Proposed
- 2026-02-22: Accepted
