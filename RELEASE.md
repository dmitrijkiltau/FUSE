# Release Guide

This guide defines the minimum steps to cut a Fuse release from this repo.

## Scope policy

- Treat the active `0.x` release line as stable for currently documented behavior in:
  - `fuse.md`
  - `fls.md`
  - `scope.md`
  - `runtime.md`
- For AOT production rollout (`v0.4.0` line), enforce the contract and SLO targets in `AOT_CONTRACT.md`.
- Enforce AOT artifact reproducibility/static-profile policy from `AOT_REPRODUCIBILITY.md`.
- Enforce version bump and compatibility rules from `VERSIONING_POLICY.md`.
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
   - Verify `AOT_CONTRACT.md` thresholds for release scope that includes AOT production artifacts.
   - Ensure GitHub Actions `Pre-release Gate` passed on the release PR (`.github/workflows/pre-release-gate.yml`).
   - Covers authority/parity gates, `fusec` + `fuse` test suites, release-mode compile checks,
     package build cache checks, AST/VM/native backend smoke runs, benchmark regression checks,
     VSIX package validation, packaging verifier regression checks, and host release artifact/checksum generation.
4. Verify package UX manually (optional but recommended):
   - `./scripts/fuse build`
   - `./scripts/fuse run examples/project_demo.fuse`
5. Build distributable binaries:
   - `./scripts/build_dist.sh --release` (outputs `dist/fuse[.exe]` and `dist/fuse-lsp[.exe]`)
6. Build host release artifacts and metadata:
   - `./scripts/package_cli_artifacts.sh --release` (emits `dist/fuse-cli-<platform>.tar.gz|.zip`)
   - `./scripts/package_aot_artifact.sh --release --manifest-path .` (emits `dist/fuse-aot-<platform>.tar.gz|.zip`)
   - `./scripts/package_vscode_extension.sh --platform <platform> --release`
   - `SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" ./scripts/generate_release_checksums.sh` (emits `dist/SHA256SUMS` and `dist/release-artifacts.json`)
7. Run the release artifact matrix workflow (`.github/workflows/release-artifacts.yml`):
   - Trigger on tag push (`v*`) or run manually via `workflow_dispatch`.
   - Produces verified per-platform CLI, AOT, and VSIX artifacts for `linux-x64`, `macos-arm64`, `windows-x64`.
   - On tag refs, publishes GitHub release assets automatically and runs post-publish checksum/package verification.
8. Commit release metadata:
   - `git add CHANGELOG.md RELEASE.md VERSIONING_POLICY.md README.md crates/*/Cargo.toml Cargo.lock tools/vscode/package*.json tools/vscode/CHANGELOG.md`
   - `git commit -m "release: vX.Y.Z"`
9. Tag release:
   - `git tag vX.Y.Z`
   - `git push origin main --tags`

## Rollback

- If smoke fails before tagging: fix forward and rerun checklist.
- If a bad tag is pushed: create a patch release (`vX.Y.(Z+1)`); do not rewrite history.
