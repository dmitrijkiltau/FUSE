# Changelog

All notable changes to this project are documented in this file.

## [0.3.0] - 2026-02-22

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
  - CLI regression coverage for lockfile stability, manifest syntax variants, and cache invalidation
- Release artifact/distribution tooling:
  - CLI bundle packaging script: `scripts/package_cli_artifacts.sh`
  - CLI bundle integrity checker: `scripts/verify_cli_artifact.sh`
  - release checksum/metadata generator: `scripts/generate_release_checksums.sh`
  - cross-platform release artifact workflow: `.github/workflows/release-artifacts.yml`

### Changed

- `scripts/build_dist.sh` now handles Windows `.exe` output names.
- `scripts/verify_vscode_vsix.sh` now ensures `tmp/` exists before temp-file creation.
- `scripts/release_smoke.sh` now includes:
  - host CLI artifact packaging + verification
  - release checksum metadata generation
  - one retry for benchmark regression check to filter transient host jitter.
- Distribution docs now define canonical artifact names:
  - `dist/fuse-cli-<platform>.tar.gz|.zip`
  - `dist/fuse-vscode-<platform>.vsix`
  - `dist/SHA256SUMS`
  - `dist/release-artifacts.json`
- VS Code packaging/release docs now include `.exe` handling and checksum publication guidance.

### Migration

- No source-breaking migration is required from `0.2.x` to `0.3.0`.

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

- Planned and partial items remain tracked in `scope.md` and `runtime.md`.
