# 1.0 Go / No-Go Decision

**Date**: 2026-03-21
**Release**: `1.0.0`
**Runway release**: `0.9.10`
**Scorecard**: `governance/scorecard-1.0.md`

---

## Verdict: Marginal Go, with tagging deferred until after merge

All language, runtime, LSP, packaging, and documentation quality gates pass. The three marginal
items are release-process and operational constraints with no language or runtime quality risk.

---

## Passing sections (no outstanding items)

| Section | Result |
|---|---|
| 1. Language semantics | ✅ All 7 rows pass, covering parser, semantic analysis, string interpolation, refinements, `when`/HTML DSL, spec completeness, and open spec blockers |
| 2. Runtime parity | ✅ All 8 rows pass, covering AST/native parity, DB semantics, HTTP client parity, config/bytes/bool parity, result decoding, and open parity blockers |
| 3. Native / AOT | ✅ All 6 rows pass, covering smoke suites, perf SLO, AOT SLO, the benchmark regression gate, artifact verification, and the release contract |
| 4. LSP quality | ✅ All 10 rows pass, covering the full suite, latency SLO, incremental updates, completion ranking, navigation, signature help, code actions, VSCode resolution/VSIX, and flake rate |
| 5. CLI and packaging | ✅ All 8 rows pass, covering the CLI artifact, AOT lock parity, the project CLI suite, dep resolution, dotenv, asset imports, the packaging verifier, and examples |
| 7. Docs and migration | ✅ All 10 rows pass: `fls.md`, `runtime.md`, and `reference.md` were audited; the migration guide was created; `CHANGELOG.md` and `SECURITY.md` were updated; `RELEASE.md`, `DEPLOY.md`, and the AOT release contract were verified |
| 9. Stability signals | ✅ All 5 rows pass, covering flake rate < 1%, no open 1.0-blocker issues, no FIXME/TODO markers in Rust source, `fuse-rt` codec tests, and IR lowering |

---

## Marginal items (3 rows, release-process and operational only)

### 6.7: Release manifest signing (`sign_release_manifest.sh`)

`cosign` is not available in the development environment. The signing step runs correctly in CI.

**Mitigation**: signing is a CI-only step by design; the CI workflow (`release-artifacts.yml`)
runs it automatically on every tagged release. Last CI run on `v0.9.9` passed.

### 6.10: CI gate workflows on main

`pre-release-gate.yml` last ran on `main` at 2026-03-08 with `success`. The release branch
changes have not been merged yet; CI will re-run automatically on merge.

**Mitigation**: all `1.0.0` release-prep changes are verified locally. The branch contains
contract/docs cleanup, version-policy alignment, and release-script guidance updates. None of
those changes plausibly introduce CI-only failures. Merge to main before tagging `1.0.0`.

### 8.5: AOT rollback playbook exercise

The playbook (`ops/AOT_ROLLBACK_PLAYBOOK.md`) is readable, internally consistent, and verified
against observed AOT binary behavior. A full end-to-end exercise requires a live AOT-deployed
instance, which is not available in the development environment.

**Mitigation**: exercise the playbook with the first production deployment of `1.0.0`. All
individual steps are covered by existing tests (`verify_aot_artifact.sh`, `aot_perf_bench.sh`,
`check_aot_perf_slo.sh`, `release_smoke.sh`).

---

## Conditions before tagging

1. Merge the release branch to `main`.
2. Verify `pre-release-gate.yml` passes on the merge commit.
3. Run `scripts/bump_version.sh 1.0.0` to update version strings.
4. Run `scripts/release_preflight.sh --workspace-publish-checks 1.0.0` and confirm it exits 0.
5. Tag `v1.0.0` and push. `release-artifacts.yml` handles signing and artifact publication.

---

## Post-release follow-up

- Exercise `ops/AOT_ROLLBACK_PLAYBOOK.md` with the first production AOT deployment.
- Open `SECURITY.md` and remove the "until `1.0.0` is tagged" qualifier from the `0.9.x` row.
- Archive the `0.9.x` branch policy per `governance/VERSIONING_POLICY.md`.
