# FUSE Governance

This document defines how project decisions are made and how maintainership works.

## Roles

- Maintainers: approve and merge contract-affecting changes.
- Contributors: propose fixes/features via issues and pull requests.
- RFC authors: drive language/runtime/tooling design proposals.

## Decision model

- Day-to-day implementation changes: handled in normal pull-request review.
- Contract-facing changes (language semantics, runtime boundary behavior, compatibility policy):
  - require RFC acceptance (`rfcs/README.md`)
  - require explicit maintainer approval on implementation PRs
  - must pass semantic authority/parity/release gates

## Required quality gates for contract changes

1. `scripts/semantic_suite.sh`
2. `scripts/authority_parity.sh`
3. `scripts/release_smoke.sh`

If docs site behavior changed, also run:

4. `scripts/fuse check --manifest-path docs`

## Disagreements

- Resolve disagreements in the relevant issue/PR first.
- If unresolved, escalated maintainer review decides for the current release.
- Decision rationale should be documented in the PR and, when applicable, RFC decision log.

## Governance updates

Changes to this document are governance changes and require:

- explicit maintainer approval
- updates to `CONTRIBUTING.md` and/or `README.md` if contributor workflow changes
