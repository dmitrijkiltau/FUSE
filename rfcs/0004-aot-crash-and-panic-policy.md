# RFC 0004: AOT Crash and Panic Reporting Policy

- Status: Accepted
- Authors: FUSE maintainers
- Created: 2026-02-22
- Updated: 2026-02-22
- Related PRs: TBD
- Related issues: v0.4.0 AOT production rollout

## Summary

Define production crash/panic handling behavior for AOT executables.

## Motivation

AOT production binaries require explicit operational behavior for fatal failures.
Without this, incident handling and supportability are inconsistent.

## Non-goals

- guaranteeing crash-free execution
- adding runtime reflection/introspection facilities

## Detailed design

1. AOT binaries must emit a stable, user-facing fatal-error envelope on unrecoverable failures.
2. Fatal output must include:
   - error class (`panic`, `runtime_fatal`, or `internal_error`)
   - short message
   - build metadata handle (version/target/build id)
3. Debug-only details (for example backtrace addresses) are controlled by build profile and env knobs.
4. Process exit codes must be deterministic for the same fatal class.
5. Panic behavior is part of operational contract and must be tested in release profile.

## Alternatives considered

1. Keep default toolchain panic output untouched.
Rejected because output stability and operator guidance are insufficient.

2. Suppress all fatal details.
Rejected because it harms diagnosability.

## Compatibility and migration

- additive operational contract for AOT mode
- no source migration required

## Test plan

- panic-path integration tests in AOT release profile
- verify fatal output envelope shape and exit code mapping
- include fatal-path checks in release smoke coverage

## Documentation updates

- `AOT_CONTRACT.md`
- `RELEASE.md`
- operator-facing release notes

## Risks

- too little detail hurts diagnosis
- too much detail leaks internal state

Mitigation:

- profile-based verbosity policy
- stable external envelope + optional debug internals

## Rollout plan

1. define envelope and exit-code mapping
2. add fatal-path tests
3. include in release criteria for `v0.4.0`

## Decision log

- 2026-02-22: Proposed
- 2026-02-22: Accepted
