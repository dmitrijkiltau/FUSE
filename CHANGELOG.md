# Changelog

All notable changes to this project are documented in this file.

## [0.9.0] - 2026-03-03

### Breaking

- HTML tag/component attribute shorthand is now canonicalized:
  - commas between HTML attrs are rejected
  - map-literal attrs in HTML call position are rejected
  - named attrs stay expression-valued (`div(class=name)`)

### Added

- Native backend performance and throughput upgrades:
  - route segment cache for HTTP matching (fewer per-request path splits)
  - GC allocation-count guard to skip full mark/sweep on non-allocating paths
  - JSON encoding fast paths (`encode_string` ASCII fast path, integer number formatting fast path)
  - tighter perf SLO/regression gates in release scripts and benchmark baselines
- Concurrency runtime throughput and observability upgrades:
  - lower-contention round-robin task scheduling
  - JIT/native `spawn` dispatch now executes asynchronously through task pool
  - runtime concurrency snapshot metrics surfaced in `--diagnostics json` and `FUSE_METRICS_HOOK=stderr`
  - `examples/spawn_bench.fuse` benchmark workload
- Multi-package workflow hardening:
  - shared manifest parser + transitive `dep:`/`root:` dependency expansion
  - cross-package cycle/unknown-dependency diagnostics with actionable hints
  - `fuse check --workspace` with per-package incremental cache correctness
  - LSP manifest-change invalidation + expanded dependency syntax coverage
- LSP scalability improvements for larger workspaces:
  - progressive focus-file indexing for diagnostics
  - persisted workspace index cache across restarts
  - latency SLO coverage for diagnostics/completion/workspace symbols (50-file fixture)
  - dedicated regression gate (`scripts/check_lsp_latency_slo.sh`) integrated into `scripts/lsp_suite.sh`
- Release automation simplification:
  - `scripts/bump_version.sh` for Cargo + VS Code version updates
  - `scripts/release_preflight.sh` for one-command pre-tag checks
  - `scripts/package_release.sh` as unified artifact packaging entry point
  - release workflow dry-run mode for packaging validation without publishing
- HTML DSL enhancements:
  - `component` declaration form with typed implicit `attrs`/`children` contract
  - compile-time `aria-*` attribute validation
  - machine-readable diagnostics for HTML attr migration errors:
  - `FUSE_HTML_ATTR_MAP`
  - `FUSE_HTML_ATTR_COMMA`
- DB layer boundary-model completion:
  - typed query result decoding (`query.all<T>()`, `query.one<T>()`)
  - `query.upsert(struct)` support
  - migration namespace key upgrade from `name` to `(package, name)` with backward-compatible bootstrap

### Changed

- Reference-service UI examples now use no-comma named HTML attrs and expression-valued attrs.
- Diagnostics JSON schema includes optional `code` for compiler diagnostics.
- `scripts/lsp_suite.sh` now runs the LSP latency SLO gate as part of the default suite.

### Migration

- Replace map-literal HTML attrs with named attrs and remove commas between HTML attrs.
- See `guides/migrations/0.8-to-0.9.md`.
- No additional language/runtime migration is required from `0.8.x` to `0.9.0`.

## [0.8.0] - 2026-03-02

### Added

- Runtime capability APIs are now implemented across AST/native:
  - `time.now`, `time.format`, `time.parse`, `time.sleep`
  - `crypto.hash`, `crypto.hmac`, `crypto.random_bytes`, `crypto.constant_time_eq`
- Native lowering support for previously unsupported call-target patterns:
  - index call targets
  - optional-index call targets
  - optional-member call targets
- Example coverage expanded for key language/runtime features:
  - `transaction_demo.fuse`
  - `capability_demo.fuse`
  - `test_demo.fuse`
  - `strict_arch_demo/`
  - `refinement_demo.fuse`
  - `json_codec.fuse`
  - `time_crypto.fuse`
  - `dep_import/`
- DB and config ergonomics improvements:
  - `db.from(...).insert(struct)`
  - `db.from(...).update(column, value).where(...)`
  - `db.from(...).delete().where(...)`
  - query-builder `.count()`
  - config env override support for user-defined struct/enum values
  - config env-name mismatch hints (`APP_DBURL` -> `APP_DB_URL`)
- Developer workflow improvements:
  - incremental `fuse check` cache mode
  - `fuse dev` compile-error overlay via reload websocket
  - `fuse test --filter <pattern>`
  - deterministic AOT build progress stages in `fuse build`
  - `--diagnostics json` structured diagnostic output mode

### Changed

- LSP server implementation was modularized from a monolith into focused handler/state modules;
  dispatch entrypoint surface remains behaviorally equivalent.
- `fuse` CLI internals were split into focused modules (`args`, `manifest`, `run/dev/build`, `deps`, `aot`, `cache`, `output`) with no contract changes.
- Test harnesses now share centralized HTTP/runtime helper logic to reduce duplication and increase parity-test stability.
- `project_cli` integration tests were split into feature-domain files while preserving test names and assertions.
- Documentation delivery moved to a GitHub-first guide surface:
  - `guides/` now contains generated `reference.md`, `onboarding.md`, and `boundary-contracts.md`
  - `docs/` site package was removed
  - `scripts/generate_guide_docs.sh` now generates guides under `guides/`

### Migration

- No language source migration is required from `0.7.x` to `0.8.0`.
- Repository consumers linking to docs-site paths should switch to `guides/` paths.

## [0.7.0] - 2026-02-28

### Added

- AOT runtime contract guarantees were formalized and test-backed:
  - explicit startup order guarantees
  - deterministic config precedence and startup determinism checks
  - stable AOT exit-code and fatal-envelope invariants
  - explicit runtime sealing guarantees (no dynamic backend fallback / runtime compilation)
- Production ergonomics were expanded:
  - canonical `/health` production pattern
  - stable startup log line format contract
  - deterministic graceful shutdown semantics for `SIGTERM`/`SIGINT`
  - explicit runtime plugin system non-goal
- Deployment surface was formalized:
  - single-page deployment guide (`ops/DEPLOY.md`) for VM/Docker/systemd/Kubernetes
  - canonical minimal production Dockerfile (`ops/docker/AOT_MINIMAL.Dockerfile`)
  - release-artifact container image packaging script and workflow wiring
- AST/native/AOT parity gates were tightened:
  - explicit observable-equivalence matrix gate
  - dedicated panic taxonomy parity gate (`exit=101`, `class=panic`)

### Changed

- `scripts/authority_parity.sh` now includes AST/native/AOT parity-lock and AOT panic-taxonomy checks.
- Benchmark regression gate now applies a local WSL2 profile floor for known loopback-latency metrics
  while preserving default/CI thresholds.
- Governance/release tracking docs were refreshed for the finalized AOT hardening scope.

### Migration

- No language source migration is required from `0.6.x` to `0.7.0`.

## [0.6.0] - 2026-02-28

### Breaking

- **Module capability enforcement is now compile-time strict.**
  Calls that require capabilities (`db`, `network`, `time`, `crypto`) now fail to compile unless
  the module declares matching top-level `requires ...` entries; cross-module capability leakage is rejected.
- **Typed error domains are now required on fallible boundaries.**
  Bare `T!` signatures are rejected; `Option ?!` without an explicit error value is rejected.
- **Structured concurrency is compiler-enforced.**
  Detached `spawn` expressions are rejected; spawned task bindings must be awaited before scope exit
  and cannot be reassigned prior to `await`.
- **`transaction:` introduces constrained execution semantics.**
  Transaction blocks reject `spawn`, `await`, early `return`, and loop control jumps;
  non-`db` capability usage inside transaction scope is rejected.

### Added

- Strict architecture compile mode:
  - `--strict-architecture` enforces capability purity
  - cross-layer import-cycle rejection
  - error-domain isolation
- HTTP request/response primitives:
  - `request.header(name)` / `request.cookie(name)`
  - `response.header(name, value)` / `response.cookie(name, value)` / `response.delete_cookie(name)`
- Deterministic transaction runtime behavior:
  - `BEGIN` on transaction entry
  - `COMMIT` on success
  - `ROLLBACK` on failure

### Changed

- Reference service migrated to deterministic architecture patterns:
  - explicit capability declarations and strict-architecture compliance
  - explicit request context flow across API/UI boundaries
  - DB flows wrapped in `transaction:` blocks
  - HTMX UI session persistence moved to HTTP-only `sid` cookie handling
- LSP builtin metadata/completion/signature coverage updated for new request/response primitives.
- Runtime/sema test suites expanded for capability/error/transaction/HTTP primitive contracts.

### Migration

- Add required module-level `requires ...` declarations for capability-gated operations and imports.
- Update fallible return signatures to explicit domains (`T!Domain`, chained where needed).
- Ensure every spawned task is bound and awaited in lexical scope.
- Wrap DB transactional write/read-modify-write flows in `transaction:` and remove disallowed control flow from those blocks.

## [0.5.0] - 2026-02-26

### Breaking

- **VM backend removed** (RFC 0007). The `--backend vm` CLI flag is no longer accepted.
  Users should remove `--backend vm` from scripts/CI; the default backend is already Native.
  Migration: drop `--backend vm` or replace with `--backend native`.
- `program.ir` cache artifact is no longer written by `fuse build`. Only `program.native`
  and `program.meta` are produced.

### Added

- Reference service with CRUD operations, user authentication, note visibility/public
  access, and like functionality.
- Native spawn task implementation (`run_native_spawn_task`) — spawned async tasks now
  execute entirely on the native backend instead of falling back to the VM.
- Note card UI enhancements with checkbox-based edit and visibility actions.

### Changed

- Default backend is Native for all execution paths (was already changed in 0.4.x, now
  the VM fallback is fully removed).
- JIT error handling improvements for native backend.
- Backend rendering improvements for empty strings.
- IR lowering error messages updated from "not supported in VM yet" to
  "not supported in IR yet".
- Documentation, governance, specs, scripts, and issue templates updated to reflect
  two-backend model (AST/native). CHANGELOG historical entries preserved.
- `release_smoke.sh` reduced from 24 to 23 steps (VM smoke step removed).
- `use_case_bench.sh` CLI workload metrics now use `--backend native`.
- Documentation package manifest backend changed from `vm` to `native`.
- Parity tests compare AST vs Native only (VM removed from matrix).
- RFC 0007 status updated to Implemented.

### Removed

- `crates/fusec/src/vm/` module (~3,000 LoC).
- `Backend::Vm` / `RunBackend::Vm` enum variants.
- `run_vm_ir`, `try_load_ir` functions from CLI runner.
- `program.ir` serialization from `write_compiled_artifacts`.
- VM benchmark comparisons from `native_bench_smoke` tests.

### Migration

- Remove `--backend vm` from any CLI invocations or scripts.
- Rebuild cached artifacts (`fuse build --clean`) — `program.ir` is no longer produced.
- No language source migration is required from `0.4.x` to `0.5.0`.

## [0.4.0] - 2026-02-22

### Added

- AOT production backend release surface and contract hardening:
  - `fuse build --aot` and `fuse build --aot --release` promoted as production build path
  - embedded AOT build metadata surfaced via `FUSE_AOT_BUILD_INFO=1`
  - startup operability trace via `FUSE_AOT_STARTUP_TRACE=1`
  - stable fatal envelope fields including `class`, `pid`, and build metadata
- AOT artifact packaging and verification coverage:
  - AOT archive packaging script: `scripts/package_aot_artifact.sh`
  - AOT archive verifier: `scripts/verify_aot_artifact.sh`
  - release packaging regression coverage for CLI/VSIX/AOT payload contracts
- AOT performance and reliability gates:
  - benchmark harness: `scripts/aot_perf_bench.sh`
  - SLO gate: `scripts/check_aot_perf_slo.sh`
  - reliability/release pipelines now include AOT perf gate coverage and artifact uploads
- Rollback/operability release guidance:
  - production rollback playbook: `ops/AOT_ROLLBACK_PLAYBOOK.md`
  - updated release/runtime docs for AOT production posture

### Changed

- Release artifact matrix policy now targets:
  - `linux-x64`
  - `macos-arm64`
  - `windows-x64`
- `scripts/release_smoke.sh` now enforces AOT startup/throughput perf collection and SLO checks.
- `scripts/reliability_repeat.sh` now includes repeated AOT perf SLO validation.
- Packaging/verification now validates AOT `mode` and `profile` metadata fields.
- `README.md`, `spec/runtime.md`, `governance/scope.md`, and `ops/RELEASE.md` now reflect AOT-as-production and rollback expectations.

### Migration

- No language source migration is required from `0.3.x` to `0.4.0`.
- Deployment posture changes:
  - use AOT artifacts as primary production binaries
  - keep CLI artifacts available as rollback fallback per `ops/AOT_ROLLBACK_PLAYBOOK.md`

## [0.3.2] - 2026-02-22

### Added

- Native parity/reliability coverage additions:
  - spawn error propagation parity checks
  - DB query-builder parity checks across AST/VM/native
  - runtime `log(...)` parity checks (text and JSON forms)
  - hardened first-run HTTP parity retry behavior for `parity_http_users_post_ok`
- Multi-file LSP quality coverage additions:
  - mixed `root:`/`dep:` rename/prepare-rename/definition/references coverage
  - incremental dependency rewire/import-shape coverage
  - larger-workspace diagnostics/navigation/references performance budget checks
- Dependency workflow contract hardening:
  - transitive conflict diagnostics include both conflicting specs and origin manifests
  - explicit invalid source/spec diagnostics for dependency entries
  - machine-readable dependency/lockfile diagnostics (`[FUSE_DEP_*]`, `[FUSE_LOCK_*]`)
  - lockfile remediation diagnostics (`unsupported version`, stale lock path guidance)
  - CLI regression coverage for lockfile stability, manifest syntax variants, and cache invalidation
  - CLI regression coverage for Windows-style dependency path separators and diagnostic code assertions
- Release artifact/distribution tooling:
  - CLI bundle packaging script: `scripts/package_cli_artifacts.sh`
  - CLI bundle integrity checker: `scripts/verify_cli_artifact.sh`
  - packaging verifier regression harness: `scripts/packaging_verifier_regression.sh`
  - release checksum/metadata generator: `scripts/generate_release_checksums.sh`
  - cross-platform release artifact workflow: `.github/workflows/release-artifacts.yml`

### Changed

- `scripts/build_dist.sh` now handles Windows `.exe` output names.
- `scripts/verify_vscode_vsix.sh` now ensures `tmp/` exists before temp-file creation.
- `scripts/release_smoke.sh` now includes:
  - host CLI artifact packaging + verification
  - packaging verifier regression checks
  - release checksum metadata generation
  - one retry for benchmark regression check to filter transient host jitter.
- Dependency path resolution now normalizes Windows-style `\` separators for path dependencies on non-Windows hosts.
- Lockfile mismatch diagnostics now include explicit remediation hints (`delete fuse.lock` or `fuse build --clean`).
- Distribution docs now define canonical artifact names:
  - `dist/fuse-cli-<platform>.tar.gz|.zip`
  - `dist/fuse-vscode-<platform>.vsix`
  - `dist/SHA256SUMS`
  - `dist/release-artifacts.json`
- VS Code packaging/release docs now include `.exe` handling and checksum publication guidance.

### Migration

- No source-breaking migration is required from `0.2.x` to `0.3.2`.

## [0.2.0] - 2026-02-22

### Breaking

- Task helper builtins removed: `task.id`, `task.done`, `task.cancel`.
- Concurrency semantics changed: `spawn` now executes on a worker pool (no eager completion model).
- `spawn` blocks now reject:
  - `box` capture/use
  - runtime side-effect builtins (`db.*`, `serve`, `print`, `log`, `env`, `asset`, `svg.inline`)
  - mutation of captured outer state
- Build cache metadata format bumped to `program.meta` v3 (content-hash validation).
- VS Code package artifact switched from `.tgz` payload to installable `.vsix`.

### Added

- Shared task scheduler module used by AST/VM/native spawn execution paths.
- Hash-based cache validation for module graph sources, `fuse.toml`, and `fuse.lock`.
- Cached `fuse run` fast-path now supports CLI program args after `--`.
- VSIX integrity verification script: `scripts/verify_vscode_vsix.sh`.
- Benchmark regression gate script: `scripts/check_use_case_bench_regression.sh`.
- Migration guide: `guides/migrations/0.1-to-0.2.md`.
- CLI `input(prompt: String = "") -> String` builtin across AST/VM/native backends.
- CLI output color policy: `--color auto|always|never` (respects `NO_COLOR`).

### Changed

- Release smoke gate now includes:
  - use-case benchmark collection
  - benchmark regression enforcement vs checked-in baseline
  - VSIX package build and validation
- Benchmark metrics now record millisecond values with sub-ms precision.
- Runtime `log(...)` text level tags now follow CLI color policy while JSON log lines remain plain.
- `fuse check|run|build|test` now emit consistent stderr step markers:
  - `[command] start`
  - `[command] ok|failed|validation failed`
- CLI diagnostics now use normalized `error:` / `warning:` prefixes.

### Migration

- See `guides/migrations/0.1-to-0.2.md` for required source/tooling updates.

## [0.1.0] - 2026-02-21

### Added

- Core language MVP: structs/enums/functions/config/service/app/import/test/migration.
- AST interpreter and VM backend with parity tests.
- Built-in runtime boundaries: config/env binding, JSON encode/decode, validation, HTTP routing.
- DB support with SQLite (`db.exec`, `db.query`, `db.one`) and migrations.
- Package tooling: `fuse.toml`, `fuse run/test/build`, dependency lockfile (`fuse.lock`).
- OpenAPI generation (`fusec --openapi`).
- LSP single-file support: diagnostics, formatting, go-to-definition, hover, rename, workspace symbols.
- Task runtime API: `task.id`, `task.done`, `task.cancel` (AST + VM).
- Semantic typing for task API and DB builtin methods (`db.exec/query/one`).
- Release smoke script and release checklist docs.
- Refinement constraints beyond ranges:
  - `regex("<pattern>")` support on string-like refined bases
  - `predicate(<fn_ident>)` support with signature checks (`fn(<base>) -> Bool`)
  - runtime parity tests across AST/VM/native plus parser/sema coverage
- Tagged JSON decode for `Result<T,E>` request bodies:
  - `{"type":"Ok","data":...}` and `{"type":"Err","data":...}`
  - OpenAPI request-body schema generation as tagged `oneOf`
- SQLite connection pooling and migration transaction pinning:
  - configurable pool size via `FUSE_DB_POOL_SIZE` with `App.dbPoolSize` fallback
  - pool-backed DB execution across AST/VM/native
  - explicit transaction-scoped migration execution (`BEGIN`/`COMMIT`/`ROLLBACK`) on one connection
  - dedicated pool config, concurrency, and migration integrity tests

### Changed

- Error namespace handling is standardized around `std.Error.*`.
- Syntax highlighting updated for qualified names/imports and task/runtime-era syntax.
- Runtime docs/specs now reflect implemented refinement constraints, tagged `Result` body decode,
  and pooled SQLite behavior.

### Notes

- Planned and partial items remain tracked in `governance/scope.md` and `spec/runtime.md`.
