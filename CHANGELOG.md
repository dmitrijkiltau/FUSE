# Changelog

All notable changes to this project are documented in this file.

## [0.1.0] - 2026-02-04

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

### Changed

- Error namespace handling is standardized around `std.Error.*`.
- Syntax highlighting updated for qualified names/imports and task/runtime-era syntax.

### Notes

- Planned and partial items remain tracked in `scope.md` and `runtime.md`.
