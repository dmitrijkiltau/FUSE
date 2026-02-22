# FUSE

FUSE is a small, strict language for building CLI apps and HTTP services with
built-in config loading, validation, JSON binding, and OpenAPI generation.

```fuse
config App:
  port: Int = env("PORT") ?? 3000

type UserCreate:
  email: Email
  name: String(1..80)

service Users at "/api":
  post "/users" body UserCreate -> UserCreate:
    return body

app "users":
  serve(App.port)
```

## Status

FUSE v0.1.0 is released. This branch is the v0.2.0 line and includes intentional breaking
changes for concurrency (`spawn`/`await` contract reset), build cache metadata, and VS Code
distribution packaging.

Compatibility is defined by documented behavior in `fls.md`, `runtime.md`, `scope.md`, and
`VERSIONING_POLICY.md`.
Upgrade guidance for this breaking minor is in `docs/migrations/0.1-to-0.2.md`.

## Requirements

- Rust toolchain (stable)
- SQLite development libraries (`libsqlite3-dev` / `sqlite-devel`)

## Quick start

```bash
# Run a single file
./scripts/fuse run examples/project_demo.fuse

# Run a package
./scripts/fuse run examples/notes-api

# Watch mode with live reload
./scripts/fuse dev examples/notes-api

# Start the language server
./scripts/fuse lsp
```

## Package commands

| Command | Description |
|---|---|
| `fuse check` | Type-check and validate a project |
| `fuse run` | Run a file or package |
| `fuse dev` | Run with file watching and live reload |
| `fuse test` | Run in-language test blocks |
| `fuse build` | Produce build artifacts |
| `fuse migrate` | Run database migrations |
| `fuse lsp` | Start the language server |

Global CLI output option:

- `--color auto|always|never` controls ANSI colors for diagnostics/status output and runtime
  `log(...)` level tags.
  `auto` is default and respects `NO_COLOR`.
- `fuse check|run|build|test` emit consistent stderr step markers:
  `[command] start`, `[command] ok|failed|validation failed`.

Packages use a `fuse.toml` manifest. Minimal example:

```toml
[package]
entry = "src/main.fuse"
app = "Api"
backend = "native"
```

### Manifest sections

- `[package]` — entry point, app name, backend selection
- `[serve]` — `openapi_ui`, `openapi_path` for built-in OpenAPI UI
- `[assets]` — SCSS/CSS compilation, file watching, content hashing
- `[assets.hooks]` — `before_build` for external pre-build hooks
- `[vite]` — `dev_url` for dev proxy fallback, `dist_dir` for production statics
- `[dependencies]` — package dependencies

### Dependency contract

Accepted `[dependencies]` forms:

```toml
[dependencies]
# local path (inline table)
LocalA = { path = "./deps/local-a" }

# local path (string shorthand)
LocalB = "./deps/local-b"

# git source pinned by revision/tag/branch/version
AuthRev = { git = "https://example.com/auth.git", rev = "a1b2c3d4" }
AuthTag = { git = "https://example.com/auth.git", tag = "v1.2.0" }
AuthBranch = { git = "https://example.com/auth.git", branch = "main" }
AuthVersion = { git = "https://example.com/auth.git", version = "1.2.0" }

# optional subdir inside git checkout
AuthSubdir = { git = "https://example.com/mono.git", tag = "v1.2.0", subdir = "packages/auth" }
```

Rules:

- Exactly one source must be set: `path` or `git`.
- For git dependencies, at most one selector may be set: `rev`, `tag`, `branch`, or `version`.
- `subdir` is valid only for git dependencies.
- Bare version strings are not a supported source form (`Dep = "1.2.3"` is invalid).
- Transitive conflicts are rejected by dependency name when specs differ.

Lockfile semantics (`fuse.lock`):

- Resolver writes lockfile `version = 1`.
- Entries store resolved source (`path` or `git+rev`) and requested spec fingerprint.
- If requested fingerprint matches, lock entry is reused; if it differs, entry is refreshed.
- Unchanged dependency graphs keep stable lockfile content.

### Build artifacts

Build outputs are stored in `.fuse/build/` (`program.ir`, `program.native`).
Cache validity uses content hashes (module graph + `fuse.toml` + `fuse.lock`) in `program.meta` v3.
Native/IR cache reuse also requires matching build fingerprints (target triple, Rust toolchain, CLI version).
Use `fuse build --clean` to clear the cache.

## Config loading

Config values are resolved in order:

1. Environment variables
2. Config file (`config.toml` by default; override with `FUSE_CONFIG`)
3. Default expressions in `config` blocks

The CLI loads `.env` from the package directory and sets only missing variables.

## Development

### Build and test

```bash
# Compiler tests (default)
./scripts/cargo_env.sh cargo test -p fusec

# CLI tests
./scripts/cargo_env.sh cargo test -p fuse
```

Always run Cargo through `scripts/cargo_env.sh` to avoid cross-device link errors.

### Quality gates

| Gate | Command | Purpose |
|---|---|---|
| Semantic suite | `./scripts/semantic_suite.sh` | Parser, type system, and boundary contract tests |
| Authority parity | `./scripts/authority_parity.sh` | AST/VM/native semantic equivalence |
| LSP suite | `./scripts/lsp_suite.sh` | LSP contracts, navigation, completions, code actions |
| LSP performance | `./scripts/lsp_perf_reliability.sh` | Cancellation handling and responsiveness budgets |
| LSP incremental | `./scripts/lsp_workspace_incremental.sh` | Workspace cache correctness |
| Benchmarks | `./scripts/use_case_bench.sh` | Real-world workload metrics |
| Release smoke | `./scripts/release_smoke.sh` | Full pre-release gate (includes all above) |

CI enforces the release smoke gate via `.github/workflows/pre-release-gate.yml`.

### Distribution

Canonical artifact names:

| Artifact | Output name |
|---|---|
| CLI bundle (Linux/macOS) | `dist/fuse-cli-<platform>.tar.gz` |
| CLI bundle (Windows) | `dist/fuse-cli-<platform>.zip` |
| VS Code extension | `dist/fuse-vscode-<platform>.vsix` |
| Release checksums | `dist/SHA256SUMS` |
| Release metadata | `dist/release-artifacts.json` |

Supported release matrix platforms:
`linux-x64`, `macos-x64`, `macos-arm64`, `windows-x64`.

```bash
# Build release binaries
./scripts/build_dist.sh --release

# Package host CLI bundle (archive + integrity check)
./scripts/package_cli_artifacts.sh --release

# Package VS Code extension with bundled LSP (.vsix + integrity check)
./scripts/package_vscode_extension.sh --release

# Generate checksums and JSON metadata for release publication
./scripts/generate_release_checksums.sh

# Install a packaged VSIX example
code --install-extension dist/fuse-vscode-linux-x64.vsix

# Regenerate docs site guides
./scripts/generate_guide_docs.sh
```

## Repo structure

| Path | Contents |
|---|---|
| `crates/fusec` | Compiler: parser, semantic analysis, VM, native runtime/JIT, LSP |
| `crates/fuse` | Package-oriented CLI wrapper |
| `crates/fuse-rt` | Shared runtime library |
| `examples/` | Sample programs and packages |
| `docs/` | Documentation site (source, assets, generated specs) |
| `tools/vscode/` | VS Code extension (syntax highlighting + LSP client) |

## Documentation

### Language and runtime specs

| Document | Scope |
|---|---|
| `fuse.md` | Product overview and doc navigation |
| `fls.md` | Formal language specification (syntax, grammar, AST, type system) |
| `runtime.md` | Runtime semantics (validation, JSON, config, HTTP, builtins, DB) |
| `scope.md` | Project constraints, roadmap, and non-goals |

### Project governance

| Document | Scope |
|---|---|
| `IDENTITY_CHARTER.md` | Language identity boundaries and "will not do" list |
| `EXTENSIBILITY_BOUNDARIES.md` | Allowed extension surfaces and stability tiers |
| `VERSIONING_POLICY.md` | Compatibility guarantees and deprecation rules |
| `BENCHMARKS.md` | Workload matrix and benchmark definitions |
| `LSP_ROADMAP.md` | Editor capability baseline and planned improvements |

### Contributing

| Document | Scope |
|---|---|
| `CONTRIBUTING.md` | Contribution standards, required checks, and RFC criteria |
| `GOVERNANCE.md` | Maintainer roles, decision model, and escalation |
| `CODE_OF_CONDUCT.md` | Contributor behavior expectations |
| `SECURITY.md` | Vulnerability disclosure and response policy |
| `rfcs/` | RFC process, template, and index |

## License

Apache-2.0. See `LICENSE`.
