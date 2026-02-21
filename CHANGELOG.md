# Changelog

All notable changes to this project are documented in this file.

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
