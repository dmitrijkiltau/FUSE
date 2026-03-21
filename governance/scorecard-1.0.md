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
| 1.1 | Parser round-trip: all golden fixtures pass | 0 failures | ✅ | `--test frontend_canonicalize`: 2 passed; `--test sema_golden`: 6 passed |
| 1.2 | Semantic analysis golden outputs stable | 0 regressions vs. 0.9.9 baseline | ✅ | `--test golden_outputs`: 59 passed |
| 1.3 | String interpolation token highlighting correct | 0 failures | ✅ | `--test lsp_contracts`: 3 passed |
| 1.4 | Refinement type checks enforced at compile time | 0 failures | ✅ | `--test refinement_runtime`: 5 passed |
| 1.5 | `when` expression coverage (including HTML DSL) | 0 failures | ✅ | `--test html_runtime`: 21 passed |
| 1.6 | `fls.md` covers every stable surface introduced through 0.9.x | No stabilized feature missing a normative entry | ⬜ | Manual audit — deferred to M4 |
| 1.7 | No open spec ambiguity with a known test gap | 0 unresolved spec issues tagged `1.0-blocker` | ⬜ | GitHub issue query — deferred to M5 |

---

## 2. Runtime parity (AST interpreter ↔ native JIT)

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 2.1 | Authority/parity suite passes on main | 0 failures | ✅ | `scripts/authority_parity.sh`: passed |
| 2.2 | `parity_ast_native.rs` — all test cases identical output | 0 failures | ✅ | `--test parity_ast_native`: 13 passed |
| 2.3 | `ast_authority_parity.rs` — all test cases identical output | 0 failures | ✅ | `--test ast_authority_parity`: 29 passed |
| 2.4 | DB semantics: pool, typed query, upsert, transaction — both backends | 0 failures | ✅ | pool: 1, typed_query: 3, upsert: 5, transaction: 1, concurrency: 4, migration: 2 — all passed |
| 2.5 | HTTP client parity (native vs AST) | 0 failures | ✅ | HTTP parity covered by `ast_authority_parity` (parity_http_* tests) and `native_http_smoke`: all passed |
| 2.6 | Config, bytes, bool-compare runtime parity | 0 failures | ✅ | config: 3, bytes: 4, bool_compare: 8 — all passed |
| 2.7 | Result decode and error propagation parity | 0 failures | ✅ | `--test result_decode_runtime`: 3 passed |
| 2.8 | No known critical parity gap open | 0 issues tagged `parity` + `1.0-blocker` | ⬜ | GitHub issue query — deferred to M5 |

---

## 3. Native / AOT execution

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 3.1 | All native smoke suites pass | 0 failures across all `native_*_smoke` targets | ✅ | 11 smoke suites, 1 test each — all passed |
| 3.2 | Native perf baseline within SLO | ≤ baseline + 10% | ✅ | `native_perf_check.sh`: cold=3.6ms (limit 800ms), warm=12.8ms (limit 200ms) |
| 3.3 | AOT perf SLO gate | Passes `check_aot_perf_slo.sh` | ✅ | `check_aot_perf_slo.sh`: p50 cold=2.4ms vs JIT 121ms; passed |
| 3.4 | Use-case benchmark regression gate | No regression vs. 0.9.9 | ✅ | `check_use_case_bench_regression.sh`: all 9 metrics PASS |
| 3.5 | AOT artifact verifies cleanly | `verify_aot_artifact.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 3.6 | AOT release contract (`AOT_RELEASE_CONTRACT.md`) honored | Dry-run packaging matches documented contract | ⬜ | Deferred to M3 dress rehearsal |

---

## 4. LSP quality

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 4.1 | Full LSP test suite passes | 0 failures | ✅ | `scripts/lsp_suite.sh`: 10 passed |
| 4.2 | LSP latency SLO gate | Passes `check_lsp_latency_slo.sh` | ✅ | `check_lsp_latency_slo.sh`: 10 passed |
| 4.3 | Incremental workspace update stable | 0 failures in `lsp_workspace_incremental` | ✅ | `--test lsp_workspace_incremental`: 10 passed |
| 4.4 | Completion ranking correct | 0 failures in `lsp_completion_rank` + `lsp_completion_member` | ✅ | rank: 2, member: 4 — all passed |
| 4.5 | Navigation and refactor stable | 0 failures in `lsp_navigation_refactor` | ✅ | `--test lsp_navigation_refactor`: 6 passed |
| 4.6 | Signature help stable | 0 failures in `lsp_signature_help` | ✅ | `--test lsp_signature_help`: 3 passed |
| 4.7 | Code actions stable | 0 failures in `lsp_code_actions` | ✅ | `--test lsp_code_actions`: 10 passed |
| 4.8 | VSCode extension resolves LSP binary | `verify_vscode_lsp_resolution.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 4.9 | VSCode VSIX artifact valid | `verify_vscode_vsix.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 4.10 | Flake rate < 1% over 20 runs | `reliability_repeat.sh` LSP targets: ≤ 1 failure in 100 | ⬜ | Deferred to M3 (time-consuming; run once against release build) |

---

## 5. CLI and packaging

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 5.1 | CLI artifact verifies cleanly | `verify_cli_artifact.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 5.2 | AOT lock parity test passes | 0 failures in `aot_parity_lock` | ✅ | `--test aot_parity_lock`: 1 passed (fixed `find_repo_root` CWD fallback) |
| 5.3 | `project_cli` integration suite passes | 0 failures | ✅ | `cargo test -p fuse`: 100 passed (all suites) |
| 5.4 | Dep resolution stable | 0 failures in `dep_resolution` | ✅ | `--test dep_resolution`: 13 passed |
| 5.5 | dotenv loading correct | 0 failures in `dotenv` | ✅ | `--test dotenv`: 2 passed |
| 5.6 | Asset imports pass | 0 failures in `asset_imports` | ✅ | `--test asset_imports`: 5 passed |
| 5.7 | Packaging verifier regression | `packaging_verifier_regression.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 5.8 | All examples compile and run | `check_examples.sh` exits 0; `run_examples` tests pass | ⬜ | Deferred to M3 dress rehearsal |

---

## 6. Release automation

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 6.1 | Release preflight passes | `release_preflight.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 6.2 | Release smoke passes | `release_smoke.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 6.3 | Release integrity passes | `release_integrity_regression.sh` and `verify_release_integrity.sh` exit 0 | ⬜ | Deferred to M3 dress rehearsal |
| 6.4 | Checksums generated and verifiable | `generate_release_checksums.sh` + spot-verify one artifact | ⬜ | Deferred to M3 dress rehearsal |
| 6.5 | SBOM generated | `generate_release_sboms.sh` exits 0; output is non-empty | ⬜ | Deferred to M3 dress rehearsal |
| 6.6 | Provenance generated | `generate_release_provenance.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 6.7 | Release manifest signs cleanly | `sign_release_manifest.sh` exits 0 | ⬜ | Deferred to M3 dress rehearsal |
| 6.8 | Release notes auto-generated and correct | `generate_release_notes.sh` produces accurate notes for 0.9.10 | ⬜ | Deferred to M4 (notes not final until M4) |
| 6.9 | Version bump script idempotent | `bump_version.sh` dry-run changes only expected files | ⬜ | Deferred to M3 dress rehearsal |
| 6.10 | CI gate workflows pass on main | `pre-release-gate.yml` and `release-artifacts.yml` green | ⬜ | Deferred to M3 dress rehearsal |

---

## 7. Documentation and migration readiness

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 7.1 | `spec/fls.md` complete for stable surface | No stabilized feature missing normative spec coverage | ⬜ | Manual audit — deferred to M4 |
| 7.2 | `spec/runtime.md` complete for stable surface | No stabilized runtime behavior undocumented | ⬜ | Manual audit — deferred to M4 |
| 7.3 | `guides/reference.md` current | No 0.9.x feature omitted | ⬜ | Manual audit against CHANGELOG 0.9.0–0.9.9 — deferred to M4 |
| 7.4 | Migration guide exists for 0.9.x → 1.0.0 | `guides/migrations/0.9-to-1.0.md` present and covers all breaking changes | ⬜ | Deferred to M4 |
| 7.5 | `CHANGELOG.md` entry for 0.9.10 complete | Entry present, accurate, no placeholder text | ⬜ | Deferred to M4 |
| 7.6 | `SECURITY.md` lists 1.0 as supported version | Entry updated before tag | ⬜ | Deferred to M4 |
| 7.7 | `ops/RELEASE.md` reflects actual 1.0 release process | Operator can execute without ad hoc fixes | ⬜ | Dress rehearsal in M3 |
| 7.8 | `ops/DEPLOY.md` accurate for reference service | Deployment steps execute cleanly | ⬜ | Dress rehearsal in M3 |
| 7.9 | `ops/AOT_RELEASE_CONTRACT.md` matches packaged binary | No delta between doc and observed binary behavior | ⬜ | M3 dry run |
| 7.10 | Guide docs regenerated and current | `generate_guide_docs.sh` exits 0; output committed | ⬜ | Deferred to M4 |

---

## 8. Reference service

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 8.1 | Reference service builds cleanly | 0 compile errors | ⬜ | Deferred to M3 dress rehearsal |
| 8.2 | Reference service runs and responds | Health endpoint returns 200 | ⬜ | M3 dress rehearsal |
| 8.3 | Auth flow works end-to-end | Register, login, session flow pass | ⬜ | M3 dress rehearsal |
| 8.4 | OpenAPI artifact in repo matches running service | `openapi_result_schema` test passes | ✅ | `--test openapi_result_schema`: 1 passed |
| 8.5 | Rollback playbook executes | `ops/AOT_ROLLBACK_PLAYBOOK.md` steps complete without error | ⬜ | M3 dress rehearsal |

---

## 9. Stability signals

| # | Check | Threshold | Status | Evidence / notes |
|---|---|---|---|---|
| 9.1 | Overall flake rate | < 1% across all suites over 20 runs | ⬜ | Deferred to M3 (run against release build) |
| 9.2 | No open `1.0-blocker` issues | 0 open issues with that label | ⬜ | GitHub issue query — deferred to M5 |
| 9.3 | No `FIXME` / `TODO` markers in committed Rust source | 0 hits | ✅ | `grep -r 'TODO\|FIXME\|HACK' crates/`: 0 matches |
| 9.4 | `fuse-rt` codec tests pass | 0 failures | ✅ | `cargo test -p fuse-rt`: bytes (2), codec (3) — all passed |
| 9.5 | IR lowering and module-scope regressions stable | 0 failures | ✅ | ir_lower_call_targets: 3, module_function_scope: 12 — all passed |

---

## Section summary

| Section | Pass criteria | Checked | Status |
|---|---|---|---|
| 1. Language semantics | All 7 rows ✅ | 5/7 automated ✅; 2 deferred to M4/M5 | ⚠️ |
| 2. Runtime parity | All 8 rows ✅ | 7/8 ✅; 1 deferred to M5 | ⚠️ |
| 3. Native / AOT | All 6 rows ✅ | 4/6 ✅; 2 deferred to M3 | ⚠️ |
| 4. LSP quality | All 10 rows ✅ | 7/10 ✅; 3 deferred to M3 | ⚠️ |
| 5. CLI and packaging | All 8 rows ✅ | 5/8 ✅; 3 deferred to M3 | ⚠️ |
| 6. Release automation | All 10 rows ✅ | 0/10 ✅; all deferred to M3/M4 | ⬜ |
| 7. Docs and migration | All 10 rows ✅ | 0/10 ✅; all deferred to M4 | ⬜ |
| 8. Reference service | All 5 rows ✅ | 1/5 ✅; 4 deferred to M3 | ⚠️ |
| 9. Stability signals | All 5 rows ✅ | 3/5 ✅; 2 deferred to M3/M5 | ⚠️ |

**Overall verdict: ⚠️ IN PROGRESS** — all automated checks pass; remaining rows are M3/M4 dress-rehearsal and manual audit items.

---

## Go / No-go rule

- **Go**: all 9 sections show ✅ PASS and no `1.0-blocker` issues are open.
- **Marginal go**: at most 2 rows across all sections are ⚠️ MARGINAL with documented mitigations; no section has more than 1 marginal row.
- **No-go**: any ❌ FAIL row, or any section has more than 1 ⚠️ MARGINAL row.

A no-go produces a bounded blocker list (M5 deliverable) and schedules a follow-up 0.9.x patch release.
