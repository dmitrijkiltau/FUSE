# RFC 0006: AOT Semantic Contract and SLO Targets

- Status: Implemented
- Authors: FUSE maintainers
- Created: 2026-02-22
- Updated: 2026-02-22
- Related PRs: N/A (landed with v0.4.0 milestone)
- Related issues: v0.4.0 AOT production rollout

## Summary

Define the semantic contract and measurable SLO targets that gate AOT production rollout.

## Motivation

Moving to AOT production mode requires explicit non-negotiable quality targets.
Without numeric thresholds, rollout criteria are subjective and drift-prone.

## Non-goals

- changing language/runtime semantics
- replacing existing semantic authority documents

## Detailed design

1. Semantic authority remains `spec/fls.md` + `spec/runtime.md`.
2. AOT is an execution strategy and must preserve canonical semantics.
3. Contract-facing backend divergence is release-blocking.
4. SLO targets are versioned in `ops/AOT_RELEASE_CONTRACT.md` and tied to release gates.

Initial `v0.4.0` targets:

1. cold-start latency improvement vs JIT-native:
   - >= 30% at p50
   - >= 20% at p95
2. binary size:
   - <= 25 MB stripped for reference service fixture
3. CI build time:
   - <= 10 minutes p95 per target AOT job
4. semantic parity:
   - zero unresolved contract-facing parity failures

## Alternatives considered

1. Defer numeric SLO targets until late implementation.
Rejected because it prevents objective go/no-go decisions.

2. Keep only qualitative goals.
Rejected because it is not enforceable in CI/release gates.

## Compatibility and migration

- no source-level migration
- release process gains explicit objective criteria

## Test plan

- parity suites remain mandatory
- benchmark/reliability jobs include AOT startup measurements
- release checklist asserts SLO pass/fail

## Documentation updates

- `ops/AOT_RELEASE_CONTRACT.md`
- `ops/RELEASE.md`

## Risks

- SLO targets set too aggressively or too loosely

Mitigation:

- threshold changes require explicit maintainer approval and documented rationale

## Rollout plan

1. accept semantic/SLO contract
2. instrument measurement paths
3. enforce gates during `v0.4.0` release completion milestone

## Decision log

- 2026-02-22: Proposed
- 2026-02-22: Accepted
