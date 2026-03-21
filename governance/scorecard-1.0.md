# Fuse 1.0 Readiness Scorecard

> Artifact produced by M1 of the 0.9.10 runway. Update each row as M2–M4 work lands.
> A section passes when every row in it is ✅. The 1.0 go/no-go decision (M5) requires all sections to pass.

---

## How to read this document

Each row has:
- **Check** — what is being evaluated
- **Threshold** — the minimum bar for a PASS verdict
- **Status** — `✅ PASS` · `⚠️ MARGINAL` · `❌ FAIL` · `⬜ NOT CHECKED`
- **Evidence / notes** — pointer to the test run, artifact, or commit that establishes the verdict

Update status and evidence in place. Do not delete rows; add a note if a check becomes irrelevant.

---

## 1. Language semantics

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 1.1 | Parser round-trip: all golden fixtures pass | 0 failures | ⬜ | `cargo test -p fusec frontend_canonicalize sema_golden` |
| 1.2 | Semantic analysis golden outputs stable | 0 regressions vs. 0.9.9 baseline | ⬜ | `cargo test -p fusec sema_golden golden_outputs` |
| 1.3 | String interpolation token highlighting correct | 0 failures | ⬜ | `cargo test -p fusec lsp_contracts` (token tests) |
| 1.4 | Refinement type checks enforced at compile time | 0 failures | ⬜ | `cargo test -p fusec refinement_runtime` |
| 1.5 | `when` expression coverage (including HTML DSL) | 0 failures | ⬜ | `cargo test -p fusec html_runtime` |
| 1.6 | `fls.md` covers every stable surface introduced through 0.9.x | No stabilized feature missing a normative entry | ⬜ | Manual audit against CHANGELOG 0.9.0–0.9.9 |
| 1.7 | No open spec ambiguity with a known test gap | 0 unresolved spec issues tagged `1.0-blocker` | ⬜ | GitHub issue query |

---

## 2. Runtime parity (AST interpreter ↔ native JIT)

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 2.1 | Authority/parity suite passes on main | 0 failures | ⬜ | `scripts/authority_parity.sh` |
| 2.2 | `parity_ast_native.rs` — all test cases identical output | 0 failures | ⬜ | `cargo test -p fusec parity_ast_native` |
| 2.3 | `ast_authority_parity.rs` — all test cases identical output | 0 failures | ⬜ | `cargo test -p fusec ast_authority_parity` |
| 2.4 | DB semantics: pool, typed query, upsert, transaction — both backends | 0 failures | ⬜ | `cargo test -p fusec db_pool_runtime db_typed_query_runtime db_upsert_runtime transaction_runtime` |
| 2.5 | HTTP client parity (native vs AST) | 0 failures | ⬜ | `cargo test -p fusec http` |
| 2.6 | Config, bytes, bool-compare runtime parity | 0 failures | ⬜ | `cargo test -p fusec config_runtime bytes_runtime bool_compare_runtime` |
| 2.7 | Result decode and error propagation parity | 0 failures | ⬜ | `cargo test -p fusec result_decode_runtime` |
| 2.8 | No known critical parity gap open | 0 issues tagged `parity` + `1.0-blocker` | ⬜ | GitHub issue query |

---

## 3. Native / AOT execution

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 3.1 | All native smoke suites pass | 0 failures across all `native_*_smoke` targets | ⬜ | `cargo test -p fusec native_` |
| 3.2 | Native perf baseline within SLO | ≤ baseline + 10% | ⬜ | `scripts/native_perf_check.sh` |
| 3.3 | AOT perf SLO gate | Passes `check_aot_perf_slo.sh` | ⬜ | `scripts/check_aot_perf_slo.sh` |
| 3.4 | Use-case benchmark regression gate | No regression vs. 0.9.9 | ⬜ | `scripts/check_use_case_bench_regression.sh` |
| 3.5 | AOT artifact verifies cleanly | `verify_aot_artifact.sh` exits 0 | ⬜ | `scripts/verify_aot_artifact.sh` |
| 3.6 | AOT release contract (`AOT_RELEASE_CONTRACT.md`) honored | Dry-run packaging matches documented contract | ⬜ | `scripts/package_aot_artifact.sh` (dry run) |

---

## 4. LSP quality

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 4.1 | Full LSP test suite passes | 0 failures | ⬜ | `scripts/lsp_suite.sh` |
| 4.2 | LSP latency SLO gate | Passes `check_lsp_latency_slo.sh` | ⬜ | `scripts/check_lsp_latency_slo.sh` |
| 4.3 | Incremental workspace update stable | 0 failures in `lsp_workspace_incremental` | ⬜ | `cargo test -p fusec lsp_workspace_incremental` |
| 4.4 | Completion ranking correct | 0 failures in `lsp_completion_rank` + `lsp_completion_member` | ⬜ | `cargo test -p fusec lsp_completion` |
| 4.5 | Navigation and refactor stable | 0 failures in `lsp_navigation_refactor` | ⬜ | `cargo test -p fusec lsp_navigation_refactor` |
| 4.6 | Signature help stable | 0 failures in `lsp_signature_help` | ⬜ | `cargo test -p fusec lsp_signature_help` |
| 4.7 | Code actions stable | 0 failures in `lsp_code_actions` | ⬜ | `cargo test -p fusec lsp_code_actions` |
| 4.8 | VSCode extension resolves LSP binary | `verify_vscode_lsp_resolution.sh` exits 0 | ⬜ | `scripts/verify_vscode_lsp_resolution.sh` |
| 4.9 | VSCode VSIX artifact valid | `verify_vscode_vsix.sh` exits 0 | ⬜ | `scripts/verify_vscode_vsix.sh` |
| 4.10 | Flake rate < 1% over 20 runs | `reliability_repeat.sh` LSP targets: ≤ 1 failure in 100 | ⬜ | `scripts/reliability_repeat.sh` |

---

## 5. CLI and packaging

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 5.1 | CLI artifact verifies cleanly | `verify_cli_artifact.sh` exits 0 | ⬜ | `scripts/verify_cli_artifact.sh` |
| 5.2 | AOT lock parity test passes | 0 failures in `aot_parity_lock` | ⬜ | `cargo test -p fuse aot_parity_lock` |
| 5.3 | `project_cli` integration suite passes | 0 failures | ⬜ | `cargo test -p fuse project_cli` |
| 5.4 | Dep resolution stable | 0 failures in `dep_resolution` | ⬜ | `cargo test -p fusec dep_resolution` |
| 5.5 | dotenv loading correct | 0 failures in `dotenv` | ⬜ | `cargo test -p fuse dotenv` |
| 5.6 | Asset imports pass | 0 failures in `asset_imports` | ⬜ | `cargo test -p fusec asset_imports` |
| 5.7 | Packaging verifier regression | `packaging_verifier_regression.sh` exits 0 | ⬜ | `scripts/packaging_verifier_regression.sh` |
| 5.8 | All examples compile and run | `check_examples.sh` exits 0; `run_examples` tests pass | ⬜ | `scripts/check_examples.sh` |

---

## 6. Release automation

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 6.1 | Release preflight passes | `release_preflight.sh` exits 0 | ⬜ | `scripts/release_preflight.sh` |
| 6.2 | Release smoke passes | `release_smoke.sh` exits 0 | ⬜ | `scripts/release_smoke.sh` |
| 6.3 | Release integrity passes | `release_integrity_regression.sh` and `verify_release_integrity.sh` exit 0 | ⬜ | Both scripts |
| 6.4 | Checksums generated and verifiable | `generate_release_checksums.sh` + spot-verify one artifact | ⬜ | `scripts/generate_release_checksums.sh` |
| 6.5 | SBOM generated | `generate_release_sboms.sh` exits 0; output is non-empty | ⬜ | `scripts/generate_release_sboms.sh` |
| 6.6 | Provenance generated | `generate_release_provenance.sh` exits 0 | ⬜ | `scripts/generate_release_provenance.sh` |
| 6.7 | Release manifest signs cleanly | `sign_release_manifest.sh` exits 0 | ⬜ | `scripts/sign_release_manifest.sh` |
| 6.8 | Release notes auto-generated and correct | `generate_release_notes.sh` produces accurate notes for 0.9.10 | ⬜ | `scripts/generate_release_notes.sh` |
| 6.9 | Version bump script idempotent | `bump_version.sh` dry-run changes only expected files | ⬜ | `scripts/bump_version.sh --dry-run` |
| 6.10 | CI gate workflows pass on main | `pre-release-gate.yml` and `release-artifacts.yml` green | ⬜ | GitHub Actions |

---

## 7. Documentation and migration readiness

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 7.1 | `spec/fls.md` complete for stable surface | No stabilized feature missing normative spec coverage | ⬜ | Manual audit |
| 7.2 | `spec/runtime.md` complete for stable surface | No stabilized runtime behavior undocumented | ⬜ | Manual audit |
| 7.3 | `guides/reference.md` current | No 0.9.x feature omitted | ⬜ | Manual audit against CHANGELOG 0.9.0–0.9.9 |
| 7.4 | Migration guide exists for 0.9.x → 1.0.0 | `guides/migrations/0.9-to-1.0.md` present and covers all breaking changes | ⬜ | File presence + content review |
| 7.5 | `CHANGELOG.md` entry for 0.9.10 complete | Entry present, accurate, no placeholder text | ⬜ | Manual review |
| 7.6 | `SECURITY.md` lists 1.0 as supported version | Entry updated before tag | ⬜ | File review |
| 7.7 | `ops/RELEASE.md` reflects actual 1.0 release process | Operator can execute without ad hoc fixes | ⬜ | Dress rehearsal in M3 |
| 7.8 | `ops/DEPLOY.md` accurate for reference service | Deployment steps execute cleanly | ⬜ | Dress rehearsal in M3 |
| 7.9 | `ops/AOT_RELEASE_CONTRACT.md` matches packaged binary | No delta between doc and observed binary behavior | ⬜ | M3 dry run |
| 7.10 | Guide docs regenerated and current | `generate_guide_docs.sh` exits 0; output committed | ⬜ | `scripts/generate_guide_docs.sh` |

---

## 8. Reference service

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 8.1 | Reference service builds cleanly | 0 compile errors | ⬜ | `fuse build` inside `examples/reference-service/` |
| 8.2 | Reference service runs and responds | Health endpoint returns 200 | ⬜ | M3 dress rehearsal |
| 8.3 | Auth flow works end-to-end | Register, login, session flow pass | ⬜ | M3 dress rehearsal |
| 8.4 | OpenAPI artifact in repo matches running service | `openapi_result_schema` test passes | ⬜ | `cargo test -p fusec openapi_result_schema` |
| 8.5 | Rollback playbook executes | `ops/AOT_ROLLBACK_PLAYBOOK.md` steps complete without error | ⬜ | M3 dress rehearsal |

---

## 9. Stability signals

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 9.1 | Overall flake rate | < 1% across all suites over 20 runs | ⬜ | `scripts/reliability_repeat.sh` |
| 9.2 | No open `1.0-blocker` issues | 0 open issues with that label | ⬜ | GitHub issue query |
| 9.3 | No `FIXME` / `TODO` markers in committed Rust source | 0 hits | ⬜ | `grep -r 'TODO\|FIXME\|HACK' crates/` |
| 9.4 | `fuse-rt` codec tests pass | 0 failures | ⬜ | `cargo test -p fuse-rt` |
| 9.5 | IR lowering and module-scope regressions stable | 0 failures | ⬜ | `cargo test -p fusec ir_lower_call_targets module_function_scope` |

---

## Section summary

| Section | Pass criteria | Status |
|---|---|---|
| 1. Language semantics | All 7 rows ✅ | ⬜ |
| 2. Runtime parity | All 8 rows ✅ | ⬜ |
| 3. Native / AOT | All 6 rows ✅ | ⬜ |
| 4. LSP quality | All 10 rows ✅ | ⬜ |
| 5. CLI and packaging | All 8 rows ✅ | ⬜ |
| 6. Release automation | All 10 rows ✅ | ⬜ |
| 7. Docs and migration | All 10 rows ✅ | ⬜ |
| 8. Reference service | All 5 rows ✅ | ⬜ |
| 9. Stability signals | All 5 rows ✅ | ⬜ |

**Overall verdict: ⬜ NOT READY** *(update when all sections pass)*

---

## Go / No-go rule

- **Go**: all 9 sections show ✅ PASS and no `1.0-blocker` issues are open.
- **Marginal go**: at most 2 rows across all sections are ⚠️ MARGINAL with documented mitigations; no section has more than 1 marginal row.
- **No-go**: any ❌ FAIL row, or any section has more than 1 ⚠️ MARGINAL row.

A no-go produces a bounded blocker list (M5 deliverable) and schedules a follow-up 0.9.x patch release.
