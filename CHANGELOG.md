# Changelog

All notable changes to this project are documented in this file.

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
- v1.0.0 stability contract and execution plan (`spec/1.0.0_STABILITY_CONTRACT.md`).
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
- `docs/fuse.toml` backend changed from `vm` to `native`.
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
- Migration guide: `docs/migrations/0.1-to-0.2.md`.
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

- See `docs/migrations/0.1-to-0.2.md` for required source/tooling updates.

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
