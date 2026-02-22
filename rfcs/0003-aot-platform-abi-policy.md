# RFC 0003: AOT Platform ABI and libc Policy

- Status: Accepted
- Authors: FUSE maintainers
- Created: 2026-02-22
- Updated: 2026-02-22
- Related PRs: TBD
- Related issues: v0.4.0 AOT production rollout

## Summary

Define supported AOT targets and ABI/libc policy for production artifacts.

## Motivation

AOT production support is only credible with explicit target policy, ABI expectations,
and release-matrix ownership.

## Non-goals

- supporting every Rust target triple
- preserving compatibility for internal cache artifacts across versions

## Detailed design

Supported AOT release targets:

1. `linux-x64`
2. `macos-arm64`
3. `windows-x64`

Policy:

1. The target matrix above is the release contract for AOT production artifacts.
2. Target-specific ABI/libc decisions must be documented in release metadata.
3. Any target addition/removal requires RFC update and release-policy update.
4. Cross-target binary reproducibility is measured per target class, not across unlike targets.

## Alternatives considered

1. Keep host-only AOT builds.
Rejected because it does not satisfy production release distribution goals.

2. Allow unbounded target list.
Rejected due to unowned reliability burden.

## Compatibility and migration

- additive for existing users
- release artifacts are explicitly scoped to supported target list

## Test plan

- CI build + smoke checks for each supported target
- packaging verifier coverage for each produced bundle

## Documentation updates

- `AOT_CONTRACT.md`
- `RELEASE.md`
- `README.md` distribution section (when AOT artifacts ship)

## Risks

- target-specific linker/toolchain breakage

Mitigation:

- per-target CI ownership
- explicit release gate for matrix completeness

## Rollout plan

1. lock target matrix (this RFC)
2. implement CI matrix for AOT artifacts
3. make matrix green-release mandatory for `v0.4.0`

## Decision log

- 2026-02-22: Proposed
- 2026-02-22: Accepted
