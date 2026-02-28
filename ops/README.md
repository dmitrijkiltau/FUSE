# Ops Tier

This directory is the documentation-tier index for release and incident operations contracts.

Canonical operational contract documents for the active `v0.4.x` line:

- [`AOT_RELEASE_CONTRACT.md`](AOT_RELEASE_CONTRACT.md) - AOT production + reproducibility release contract
- [`AOT_ROLLBACK_PLAYBOOK.md`](AOT_ROLLBACK_PLAYBOOK.md) - AOT rollback and recovery steps
- [`DEPLOY.md`](DEPLOY.md) - canonical deployment patterns (VM, Docker, systemd, Kubernetes)
- [`RELEASE.md`](RELEASE.md) - release workflow and gate checklist
- [`FLAKE_TRIAGE.md`](FLAKE_TRIAGE.md) - intermittent failure triage workflow
- [`BENCHMARKS.md`](BENCHMARKS.md) - benchmark matrix and metric definitions

Conflict rule: release policy and incident handling defer to the document that owns that area.
