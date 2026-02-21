# Contributing to FUSE

This document defines contribution standards for language, runtime, tooling, and docs changes.

## Project priorities

Contributions must preserve:

- semantic authority: parser/frontend canonicalization define behavior
- backend parity: AST/VM/native must stay equivalent for observable semantics
- boundary determinism: typed contracts and runtime mapping remain explicit

See:

- `IDENTITY_CHARTER.md`
- `EXTENSIBILITY_BOUNDARIES.md`
- `VERSIONING_POLICY.md`
- `GOVERNANCE.md`
- `CODE_OF_CONDUCT.md`

## Development setup

Required commands and conventions:

- run cargo commands via `scripts/cargo_env.sh`
- use `scripts/fuse` for CLI/package workflows
- default compiler test command:
  - `scripts/cargo_env.sh cargo test -p fusec`

Recommended baseline before opening a PR:

1. `scripts/semantic_suite.sh`
2. `scripts/authority_parity.sh`
3. `scripts/release_smoke.sh`
4. `scripts/fuse check --manifest-path docs` (if docs site changed)

## Change types and required updates

| Change type | Required updates |
| --- | --- |
| Syntax, type rules, canonicalization, parser/sema behavior | update `fls.md`; add/adjust semantic tests |
| Runtime behavior, boundary mapping, builtins, status/error semantics | update `runtime.md`; add/adjust runtime/parity tests |
| Scope or non-goal boundaries | update `scope.md` and/or `IDENTITY_CHARTER.md` |
| Versioning/deprecation behavior | update `VERSIONING_POLICY.md` and `CHANGELOG.md` |
| Docs site behavior/content pipeline | update docs source + generated outputs under `docs/site/` |
| Language/runtime behavior visible in reference docs | update `docs/site/specs/reference.md` (manually maintained, must stay aligned with `fls.md` and `runtime.md`) |

If semantics change and docs are not updated in the same PR, the PR is incomplete.

## Pull request standards

PRs must include:

1. Clear problem statement and proposed behavior.
2. Test evidence (commands run and outcomes).
3. Spec/doc updates when behavior changed.
4. Backward-compatibility note:
   - compatible, deprecated, or breaking.
5. RFC link when required (see next section).

Reviewers should reject PRs that:

- introduce backend-specific semantics
- bypass semantic authority/parity gates
- change contract-facing behavior without spec changes
- mix unrelated refactors with semantic changes

## RFC process for language/runtime decisions

An RFC is required for:

- new syntax or grammar changes
- type-system behavior changes
- runtime boundary behavior changes (decode/validation/error mapping/status rules)
- compatibility/deprecation policy changes
- new extension surface proposals

An RFC is not required for:

- implementation-only refactors with no contract change
- test-only and docs-only clarifications
- bug fixes that restore already documented behavior

RFC workflow:

1. Copy `rfcs/0000-template.md` to `rfcs/NNNN-short-title.md`.
2. Open RFC PR with status `Proposed`.
3. Gather review and update the decision section.
4. Mark status `Accepted` or `Rejected`.
5. Land implementation PR(s) that reference the accepted RFC.
6. Update RFC status to `Implemented` when complete.

## Issue intake

Use the issue templates to keep reports actionable:

- `.github/ISSUE_TEMPLATE/bug_report.md`
- `.github/ISSUE_TEMPLATE/language_change_rfc.md`

Security vulnerabilities should not be reported through public issues. Use the private reporting
path documented in `SECURITY.md`.

## Commit and review hygiene

- Keep PRs focused and small enough to review.
- Include generated artifacts when they are part of the checked-in workflow.
- Do not force-push over reviewed history unless requested.
- Do not merge with failing required checks.

## Decision authority

Merge authority and escalation policy are defined in `GOVERNANCE.md`. Contract-facing changes
remain maintainer-gated and require explicit approval on the PR.
