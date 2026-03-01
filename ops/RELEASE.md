# Release Guide

This guide defines the minimum steps to cut a Fuse release from this repo.

## Scope policy

- Treat the active `0.x` release line as stable for currently documented behavior in:
  - `README.md`
  - `spec/fls.md`
  - `governance/scope.md`
  - `spec/runtime.md`
  - `DEPLOY.md` (deployment patterns and official image path)
- For AOT production rollout (`v0.7.0` line), enforce the contract and SLO targets in `AOT_RELEASE_CONTRACT.md`.
- Enforce rollback preparedness from `AOT_ROLLBACK_PLAYBOOK.md`.
- Enforce version bump and compatibility rules from `governance/VERSIONING_POLICY.md`.
- Features marked planned/unsupported stay out of release criteria.

## Prerequisites

- Rust toolchain installed.
- Clean working tree (or deliberate release-only diff).
- Version/changelog updates prepared.

## Release checklist

1. Update versions:
   - `crates/fuse/Cargo.toml`
   - `crates/fusec/Cargo.toml`
   - `crates/fuse-rt/Cargo.toml` (if changed API/runtime)
   - `tools/vscode/package.json`
   - `tools/vscode/package-lock.json`
2. Update `CHANGELOG.md`:
   - Move relevant items from `Unreleased` into the new version section.
3. Run smoke checks:
   - `./scripts/authority_parity.sh` (explicit semantic-authority gate)
   - `./scripts/release_smoke.sh`
   - Verify `AOT_RELEASE_CONTRACT.md` thresholds for release scope that includes AOT production artifacts.
   - Verify latest `.fuse/bench/aot_perf_metrics.json` passes `./scripts/check_aot_perf_slo.sh`.
   - Ensure GitHub Actions `Pre-release Gate` passed on the release PR (`.github/workflows/pre-release-gate.yml`).
   - Covers authority/parity gates, `fusec` + `fuse` test suites, release-mode compile checks,
     package build cache checks, AST/native backend smoke runs, benchmark regression checks,
     VSIX package validation, packaging verifier regression checks, and host release artifact/checksum generation.
4. Verify package UX manually (optional but recommended):
   - `./scripts/fuse build`
   - `./scripts/fuse run examples/project_demo.fuse`
5. Build distributable binaries:
   - `./scripts/build_dist.sh --release` (outputs `dist/fuse[.exe]` and `dist/fuse-lsp[.exe]`)
6. Build host release artifacts and metadata:
   - `./scripts/package_cli_artifacts.sh --release` (emits `dist/fuse-cli-<platform>.tar.gz|.zip`)
   - `./scripts/package_aot_artifact.sh --release --manifest-path .` (emits `dist/fuse-aot-<platform>.tar.gz|.zip`)
   - `./scripts/package_aot_container_image.sh --archive dist/fuse-aot-linux-x64.tar.gz --image ghcr.io/dmitrijkiltau/fuse-aot-demo --tag vX.Y.Z --tag <git-sha>` (builds container image from release artifact)
   - `./scripts/package_vscode_extension.sh --platform <platform> --release`
   - `SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" ./scripts/generate_release_checksums.sh` (emits `dist/SHA256SUMS` and `dist/release-artifacts.json`)
7. Validate rollback posture:
   - confirm `AOT_ROLLBACK_PLAYBOOK.md` steps were reviewed for this release
   - confirm fallback `fuse-cli-<platform>.*` artifacts are published alongside `fuse-aot-<platform>.*`
8. Run the release artifact matrix workflow (`.github/workflows/release-artifacts.yml`):
   - Trigger on tag push (`v*`) or run manually via `workflow_dispatch`.
   - Produces verified per-platform CLI, AOT, and VSIX artifacts for `linux-x64`, `macos-arm64`, `windows-x64`.
   - On tagged releases, also publishes the official reference container image:
     `ghcr.io/dmitrijkiltau/fuse-aot-demo:<tag>` built from `fuse-aot-linux-x64.tar.gz`.
   - On tag refs, publishes GitHub release assets automatically and runs post-publish checksum/package verification.
9. Commit release metadata:
   - `git add CHANGELOG.md ops/RELEASE.md governance/VERSIONING_POLICY.md README.md crates/*/Cargo.toml Cargo.lock tools/vscode/package*.json tools/vscode/CHANGELOG.md`
   - `git commit -m "release: vX.Y.Z"`
10. Tag release:
   - `git tag vX.Y.Z`
   - `git push origin main --tags`

## Rollback

- If smoke fails before tagging: fix forward and rerun checklist.
- If a bad tag is pushed: create a patch release (`vX.Y.(Z+1)`); do not rewrite history.
