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

FUSE v0.3.2 is released. This branch tracks the v0.3.x quality/stability line with
native/backend parity hardening, multi-file tooling reliability, dependency workflow
contract coverage, and release artifact matrix automation.

Compatibility is defined by documented behavior in `fls.md`, `runtime.md`, `scope.md`, and
`VERSIONING_POLICY.md`.
Historical upgrade guidance for the `0.1.x -> 0.2.0` breaking minor is in
`docs/migrations/0.1-to-0.2.md`.

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
| `fuse build` | Produce build artifacts and optional AOT output |
| `fuse migrate` | Run database migrations |
| `fuse lsp` | Start the language server |

Global CLI output option:

- `--color auto|always|never` controls ANSI colors for diagnostics/status output and runtime
  `log(...)` level tags.
  `auto` is default and respects `NO_COLOR`.
- `fuse check|run|build|test` emit consistent stderr step markers:
  `[command] start`, `[command] ok|failed|validation failed`.

Build-specific options:

- `fuse build --aot` emits a deployable AOT binary using the default output path
  `.fuse/build/program.aot` (`.fuse/build/program.aot.exe` on Windows) unless
  `[build].native_bin` is configured.
- `fuse build --aot --release` uses the release profile for AOT binary generation.
- `fuse build --release` without `--aot` is rejected.

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
- Path dependencies accept `/` and `\` separators in manifest values for cross-platform repos.
- Bare version strings are not a supported source form (`Dep = "1.2.3"` is invalid).
- Transitive conflicts are rejected by dependency name when specs differ.
- Dependency and lockfile diagnostics include machine-readable codes
  (`[FUSE_DEP_*]`, `[FUSE_LOCK_*]`) for CI/tooling parsing.

Lockfile semantics (`fuse.lock`):

- Resolver writes lockfile `version = 1`.
- Entries store resolved source (`path` or `git+rev`) and requested spec fingerprint.
- If requested fingerprint matches, lock entry is reused; if it differs, entry is refreshed.
- Unchanged dependency graphs keep stable lockfile content.
- Lockfile format/load errors include remediation guidance to regenerate `fuse.lock`.

### Build artifacts

Cache outputs are stored in `.fuse/build/` (`program.ir`, `program.native`).
Cache validity uses content hashes (module graph + `fuse.toml` + `fuse.lock`) in `program.meta` v3.
Native/IR cache reuse also requires matching build fingerprints (target triple, Rust toolchain, CLI version).

Deployable AOT output:

- `fuse build --aot` emits `.fuse/build/program.aot` (`.exe` on Windows) by default.
- `[build].native_bin` overrides the AOT output path and remains supported.
- AOT binaries embed build metadata:
  `target`, `rustc`, `cli`, `runtime_cache`, and `contract`.
  Use `FUSE_AOT_BUILD_INFO=1 <aot-binary>` to print this metadata and exit.
- AOT build/link failures are deterministic command failures with `error:` diagnostics and
  `[build] failed` step footer.
- Runtime failures in AOT binaries emit a stable fatal envelope:
  `fatal: class=<runtime_fatal|panic> message=<...> <build-info>`.

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
| Benchmarks | `./scripts/use_case_bench.sh` | Real-world workload metrics (`--median-of-3` available for reliability runs) |
| Reliability repeat | `./scripts/reliability_repeat.sh --iterations 2` | Repeat-run stability checks for parity/LSP/benchmark-sensitive paths |
| Packaging verifier regression | `./scripts/packaging_verifier_regression.sh` | Cross-platform CLI+AOT archive and VSIX verifier coverage (including Windows `.exe` naming) |
| Release smoke | `./scripts/release_smoke.sh` | Full pre-release gate (includes all above) |

CI enforces the release smoke gate via `.github/workflows/pre-release-gate.yml`.

### Distribution

Canonical artifact names:

| Artifact | Output name |
|---|---|
| CLI bundle (Linux/macOS) | `dist/fuse-cli-<platform>.tar.gz` |
| CLI bundle (Windows) | `dist/fuse-cli-<platform>.zip` |
| AOT reference bundle (Linux/macOS) | `dist/fuse-aot-<platform>.tar.gz` |
| AOT reference bundle (Windows) | `dist/fuse-aot-<platform>.zip` |
| VS Code extension | `dist/fuse-vscode-<platform>.vsix` |
| Release checksums | `dist/SHA256SUMS` |
| Release metadata | `dist/release-artifacts.json` |

Supported release matrix platforms:
`linux-x64`, `macos-arm64`, `windows-x64`.

Reproducibility + static profile policy: `AOT_REPRODUCIBILITY.md`.

```bash
# Build release binaries
./scripts/build_dist.sh --release

# Package host CLI bundle (archive + integrity check)
./scripts/package_cli_artifacts.sh --release

# Package host AOT reference bundle (archive + integrity check)
./scripts/package_aot_artifact.sh --release --manifest-path .

# Package VS Code extension with bundled LSP (.vsix + integrity check)
./scripts/package_vscode_extension.sh --release

# Generate checksums and JSON metadata for release publication
./scripts/generate_release_checksums.sh

# Reproducible metadata timestamp (optional)
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" ./scripts/generate_release_checksums.sh

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
| `AOT_REPRODUCIBILITY.md` | AOT release reproducibility and static-profile constraints |
| `FLAKE_TRIAGE.md` | Checklist for diagnosing and closing intermittent CI/test failures |
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
