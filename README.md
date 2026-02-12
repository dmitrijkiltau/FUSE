# FUSE

FUSE is an experimental language and toolchain for small services and CLIs with
first‑class HTTP routing, config, validation, and OpenAPI generation.

Status: the language and VM are usable; the native backend is experimental and
fails on unsupported instructions.

## Requirements

- Rust toolchain (stable)
- SQLite dev libs (via `rusqlite`)

## Quick start

Run a single file:

```
./scripts/fuse run examples/project_demo.fuse
```

Run a package (uses `fuse.toml`):

```
./scripts/fuse run --manifest-path examples/notes-api
```

Run package in watch mode with live reload:

```
./scripts/fuse dev --manifest-path examples/notes-api
```

OpenAPI UI is auto-exposed in dev at `/docs` (configurable via `[serve].openapi_path`).

Start the LSP:

```
./scripts/fuse lsp
```

## Build and test

Run the compiler test suite:

```
./scripts/cargo_env.sh cargo test -p fusec
```

Release smoke checks:

```
./scripts/release_smoke.sh
```

Build distributable binaries:

```
./scripts/build_dist.sh
```

## Package tooling

`fuse` reads `fuse.toml` (current directory or `--manifest-path`) and uses
`package.entry` for `fuse dev` / `fuse run` / `fuse test`.
Set `[serve].openapi_ui = true` to expose the OpenAPI UI for normal `fuse run`.

Build artifacts and caches:

- `.fuse/build/program.ir`
- `.fuse/build/program.native` (native backend image)

Use `fuse build --clean` to clear `.fuse/build`.

## Config loading

Config resolution order:

1. Environment variables
2. `config.toml` (default; override with `FUSE_CONFIG`)
3. Default expressions in `config` blocks

The CLI loads a `.env` file from the package directory (if present) and injects
any missing environment variables before config resolution.

## Native backend

Enable with `--backend native` or `backend = "native"` in `fuse.toml`.
The native backend uses a Cranelift JIT fast‑path for some functions and fails
on unsupported instructions (no fallback yet).

## Repo structure

- `crates/fusec` — compiler, VM, native backend
- `crates/fuse` — CLI
- `examples/` — sample programs and packages
- `tools/vscode` — VS Code extension assets

## Specs

- `docs/fuse.md` — product overview
- `docs/fls.md` — formal language spec
- `docs/runtime.md` — runtime semantics
- `docs/scope.md` — scope/roadmap
