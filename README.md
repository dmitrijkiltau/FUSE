# FUSE

FUSE is a small, strict language + toolchain for small CLIs and HTTP services with built-in
config loading, validation, JSON binding, and OpenAPI generation.

Status:

- parser + semantic analysis + AST/VM backends are usable
- native backend is available (`--backend native`) with VM-compatible runtime semantics
- function symbols are module-scoped (duplicate function names across modules are supported)

## v0.1 stability contract

For `0.1.x`, compatibility is defined by currently documented supported behavior in:

- `fls.md` (syntax + static semantics)
- `runtime.md` (runtime semantics + boundary behavior)
- `scope.md` (project constraints and non-goals)

## Requirements

- Rust toolchain (stable)
- SQLite dev libs (via `rusqlite`)

## Quick start

Run a single file:

```
./scripts/fuse run examples/project_demo.fuse
```

Run a package (directory with `fuse.toml`):

```
./scripts/fuse run examples/notes-api
```

Run package in watch mode (`fuse dev`) with live reload:

```
./scripts/fuse dev examples/notes-api
```

OpenAPI UI is auto-exposed in dev at `/docs` (configurable via `[serve].openapi_path`).

Start the LSP:

```
./scripts/fuse lsp
```

## Package tooling

`fuse` reads `fuse.toml` (current directory or `--manifest-path`) and uses `package.entry` for
`fuse run` / `fuse dev` / `fuse test`.

Common package features:

- `[serve].openapi_ui` / `openapi_path` for OpenAPI UI serving
- `[assets]` (`scss`, `css`, `watch`, `hash`) for external `sass` orchestration
- `[assets.hooks].before_build` for pre-build external hooks
- `[vite]` (`dev_url`, `dist_dir`) for dev proxy fallback and production static defaults

Use `asset("css/app.css")` to resolve logical asset paths to hashed URLs, and
`svg.inline("icons/name")` to inline SVG from `assets/svg` (or `FUSE_SVG_DIR`) as `Html`.
Html tag builtins are available directly (`div`, `section`, `meta`, ...), with block DSL sugar
(`div(): ...`), string-literal child lowering (`"x"` -> `html.text("x")` in Html blocks), and
attribute shorthand (`div(class="hero", type="button")` -> attrs map). Named args can also be
written one-per-line without commas. Underscores in shorthand names are normalized to dashes
(`aria_label` -> `aria-label`, `data_view` -> `data-view`).

Build artifacts and caches:

- `.fuse/build/program.ir`
- `.fuse/build/program.native`

Use `fuse build --clean` to clear `.fuse/build`.

## Config loading

Resolution order:

1. environment variables
2. `config.toml` (default; override via `FUSE_CONFIG`)
3. default expressions in `config` blocks

The CLI loads `.env` from the package directory and only sets missing environment variables.

## Build and test

Default test command:

```
./scripts/cargo_env.sh cargo test -p fusec
```

Run all `fuse` CLI tests:

```
./scripts/cargo_env.sh cargo test -p fuse
```

Release smoke checks:

```
./scripts/release_smoke.sh
```

Build distributable binaries:

```
./scripts/build_dist.sh
```

## Repo structure

- `crates/fusec` - compiler, parser/sema, VM, native runtime/JIT, LSP
- `crates/fuse` - package-oriented CLI wrapper around `fusec`
- `examples/` - sample programs/packages
- `docs/` - docs site package (UI, assets, and docs app)
- `tools/vscode` - VS Code extension assets

## License

Apache-2.0. See `LICENSE`.

## Specs

- `fuse.md` - project overview + package tooling
- `fls.md` - formal language specification
- `runtime.md` - runtime semantics and builtin behavior
- `scope.md` - project scope and non-goals
