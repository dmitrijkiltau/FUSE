# Release Guide

This guide defines the minimum steps to cut a Fuse release from this repo.

## Scope policy

- Treat `0.1.x` as MVP-stable for currently documented behavior in:
  - `fuse.md`
  - `fls.md`
  - `scope.md`
  - `runtime.md`
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
2. Update `CHANGELOG.md`:
   - Move relevant items from `Unreleased` into the new version section.
3. Run smoke checks:
   - `./scripts/release_smoke.sh`
   - Covers `fusec` + `fuse` test suites, release-mode compile checks, package build cache checks,
     and AST/VM backend smoke runs.
4. Verify package UX manually (optional but recommended):
   - `./scripts/fuse build`
   - `./scripts/fuse run examples/project_demo.fuse`
5. Build distributable binaries:
   - `./scripts/build_dist.sh --release` (outputs `dist/fuse` and `dist/fuse-lsp`)
6. Commit release metadata:
   - `git add CHANGELOG.md RELEASE.md crates/*/Cargo.toml Cargo.lock`
   - `git commit -m "release: vX.Y.Z"`
7. Tag release:
   - `git tag vX.Y.Z`
   - `git push origin main --tags`

## Rollback

- If smoke fails before tagging: fix forward and rerun checklist.
- If a bad tag is pushed: create a patch release (`vX.Y.(Z+1)`); do not rewrite history.
