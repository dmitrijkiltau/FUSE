# FUSE

FUSE is a small, strict language for building CLI apps and HTTP services with
built-in config loading, validation, JSON binding, and OpenAPI generation.

```fuse
fn main(name: String = "world"):
  print("Hello, ${name}!")

app "hello":
  main()
```

```fuse
requires network

config App:
  port: Int = env_int("PORT") ?? 3000

type UserCreate:
  email: Email
  name: String(1..80)

service Users at "/api":
  post "/users" body UserCreate -> UserCreate:
    return body

app "users":
  serve(App.port)
```

## Guiding idea

FUSE is not trying to invent new syntax. The differentiator is a consistent contract at
boundaries: types, validation, and transport behavior are aligned by default.

### What FUSE optimizes for

**Small and strict.** The language intentionally keeps a narrow core: indentation-based blocks,
explicit declarations (`fn`, `type`, `enum`, `config`, `service`, `app`), and strong types with
minimal ceremony.

**Boundaries as first-class language concerns.** Runtime surfaces are built in and consistent
across backends: config loading, JSON encoding/decoding, validation, and HTTP request/response
binding.

**One source of truth per concern.** You describe contracts in FUSE types and route signatures.
The runtime applies those contracts at boundaries instead of requiring repeated glue code.

## Document contract

- `Normative`: No.
- `Front door`: Yes. This is the single onboarding entry point for the repository.
- `Owned concerns`: installation prerequisites, day-1 commands, build/test/release workflows,
  artifact matrix, and documentation routing.
- `Conflict policy`: if this file conflicts with `spec/fls.md`, `spec/runtime.md`, `governance/scope.md`, or
  `governance/VERSIONING_POLICY.md`, defer to the document that owns that concern.

## Status

FUSE `v0.9.x` is the current stable line. `v0.9.0` shipped with HTML attribute syntax
simplifications, workspace incremental-check optimizations, and full LSP large-workspace support.
Patch releases (`0.9.1`, …) stabilize this line with non-breaking improvements.

Compatibility is defined by documented behavior in `spec/fls.md`, `spec/runtime.md`, `governance/scope.md`, and
`governance/VERSIONING_POLICY.md`.
Historical upgrade guidance is in:
`guides/migrations/0.8-to-0.9.md` and `guides/migrations/0.1-to-0.2.md`.

## Requirements

- Rust toolchain (stable)
- SQLite development libraries (`libsqlite3-dev` / `sqlite-devel`)

## Quick start

```bash
# Run a single file
./scripts/fuse run examples/project_demo.fuse

# Run a package
./scripts/fuse run examples/reference-service

# Watch mode with live reload
./scripts/fuse dev examples/reference-service

# Start the language server
./scripts/fuse lsp
```

## Strings

FUSE supports both single-line and multiline string literals:

- `"..."` for standard strings
- `"""..."""` for multiline text (useful for SQL/query text)
- `${expr}` interpolation works in both forms

```fuse
db.exec("""create table if not exists users (
  id int primary key,
  name text
)""")
```

## Module capabilities

Capability boundaries are declared at module top-level and enforced at compile-time:

```fuse
requires db
requires network
```

Current capability checks:

- `db.exec/query/one/from` and `db.from(...).{select,where,order_by,limit,insert,upsert,update,delete,count,one,all,exec}` require `requires db`
- typed query forms `db.from(...).select([...]).one<T>()` / `.all<T>()` validate rows into declared `type` values
- `serve(...)` requires `requires network`
- `http.request/get/post` require `requires network`
- `time(...)` / `time.*` require `requires time`
- `crypto.*` requires `requires crypto`
- calling imported module functions requires declaring the callee module's capabilities
- `transaction:` blocks require `requires db` and reject non-`db` capability usage inside the block

## Typed error domains

Fallible boundaries require explicit error domains:

- use `T!Domain` (and chained forms like `T!AuthError!DbError`) on function/service return types
- bare `T!` is rejected at compile-time
- `expr ?!` without an explicit error value is allowed only for `Result` propagation; `Option ?!` requires `err`

## Structured concurrency

`spawn`/`await` is compile-time constrained for deterministic task lifetimes:

- detached task expressions are rejected
- spawned task bindings must be awaited before leaving scope
- spawned task bindings cannot be reassigned before `await`

## Deterministic transactions

`transaction:` introduces a constrained DB transaction scope:

- commits on success, rolls back on block failure
- requires `requires db`
- rejects `spawn`, `await`, early `return`, and `break`/`continue` inside the block
- rejects non-`db` capability usage inside the block

## HTTP request/response primitives

Service routes can directly access HTTP headers/cookies without custom runtime glue:

- `request.header(name: String) -> String?` reads inbound headers (case-insensitive)
- `request.cookie(name: String) -> String?` reads inbound cookie values
- `response.header(name: String, value: String)` appends response headers
- `response.cookie(name: String, value: String)` appends `Set-Cookie` headers
- `response.delete_cookie(name: String)` emits cookie-expiration `Set-Cookie` headers

Observability baseline for HTTP runtime:

- request ID precedence: inbound `x-request-id`, then `x-correlation-id`, otherwise generated
  `req-<hex>`
- runtime emits `X-Request-Id` on runtime-owned HTTP responses
- `request.header("x-request-id")` returns the resolved lifecycle request ID inside route handlers
- `FUSE_REQUEST_LOG=structured` enables one JSON request log line per handled request on stderr
- `FUSE_METRICS_HOOK=stderr` enables one metrics line (`metrics: <json>`) per handled request
- canonical production health route pattern (non-built-in):
  `get "/health" -> Map<String, String>: return {"status": "ok"}`
- no runtime plugin extension system (explicit non-goal)

## HTTP client API

Modules with `requires network` can also issue outbound HTTP requests:

- `http.request(method: String, url: String, body?: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error`
- `http.get(url: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error`
- `http.post(url: String, body: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error`

`0.9.6` intentionally supports `http://` only. `https://...` fails with `Err(http.error)` and code
`unsupported_scheme`.

Runtime behavior:

- `2xx` responses return `Ok(http.response)`
- non-`2xx` responses return `Err(http.error)` with `code = "http_status"`
- `timeout_ms` defaults to `30000`; `0` disables the socket timeout
- request/response bodies are plain `String`
- response/error headers are exposed as `Map<String, String>` with lowercase header names

Response and error shapes:

- `http.response.method`, `url`, `status`, `headers`, `body`
- `http.error.code`, `message`, `method`, `url`, `status?`, `headers`, `body?`

## Strict architecture mode

`fuse check --strict-architecture` enables additional architecture validation:

- capability purity (declared `requires` capabilities must be used)
- cross-layer import-cycle rejection
- error-domain isolation (boundary signatures in a module cannot mix domains from multiple modules)

## Package commands

| Command | Description |
|---|---|
| `fuse check` | Type-check and validate a project |
| `fuse run` | Run a file or package |
| `fuse dev` | Run with file watching and live reload |
| `fuse test` | Run in-language test blocks |
| `fuse build` | Produce build artifacts and optional AOT output |
| `fuse clean --cache` | Remove `.fuse-cache` directories under a selected root |
| `fuse deps lock` | Refresh `fuse.lock` or check it for drift |
| `fuse deps publish-check` | Check workspace manifest/lock readiness for publish |
| `fuse migrate` | Run database migrations |
| `fuse lsp` | Start the language server |

Global CLI output option:

- `--color auto|always|never` controls ANSI colors for diagnostics/status output and runtime
  `log(...)` level tags.
  `auto` is default and respects `NO_COLOR`.
- `--diagnostics json` switches CLI diagnostics on stderr to JSON Lines suitable for editor/CI
  consumers. Diagnostic entries use fields:
  `kind="diagnostic"`, `level`, `code?`, `message`, `path?`, `line?`, `column?`, `span_start`, `span_end`.
  Command-step entries use:
  `kind="command_step"`, `command`, `message`.
- `fuse check|run|build|test|clean` emit consistent stderr step markers:
  `[command] start`, `[command] ok|failed|validation failed`.
- `--frozen` is supported by `fuse check|run|build|test` and fails with
  `[FUSE_LOCK_FROZEN]` if dependency resolution would rewrite `fuse.lock`.
- `fuse test --filter <pattern>` runs only test blocks whose names contain `<pattern>`
  (case-sensitive substring match).
- `--strict-architecture` enables strict architecture checks in semantic analysis
  (primarily used with `fuse check` and `fuse build`).

Build-specific options:

- `fuse build --release` emits a deployable AOT binary using the default output path
  `.fuse/build/program.aot` (`.fuse/build/program.aot.exe` on Windows) unless
  `[build].native_bin` is configured.
- `fuse build --aot` forces AOT output in debug profile.
- AOT-emitting builds (`--aot`, `--release`, or `[build].native_bin`) print
  `[build] aot [n/6] ...` progress stages for compile/link steps.
- `fuse build` remains the explicit non-AOT local development path (cache artifacts only).

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
- `[assets]` — CSS asset paths, file watching, content hashing
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
- `fuse deps lock` refreshes the lockfile for the selected package.
- `fuse deps lock --check` fails with `[FUSE_LOCK_OUT_OF_DATE]` when the current lockfile
  differs from the resolved dependency graph.
- `fuse deps publish-check` walks all `fuse.toml` files under a workspace root and reports
  missing entry files or out-of-date lockfiles per package.
- Lockfile format/load errors include remediation guidance to regenerate `fuse.lock`.

### Build artifacts

Cache outputs are stored in `.fuse/build/` (`program.native`).
Cache validity uses content hashes (module graph + `fuse.toml` + `fuse.lock`) in `program.meta` v3.
Native/IR cache reuse also requires matching build fingerprints (target triple, Rust toolchain, CLI version).
`fuse check` also writes incremental metadata (`check.meta` / `check.strict.meta`) and skips
unchanged modules by hash on warm runs.
Workspace checks and the language server may also persist reusable data under `.fuse-cache/`
(`check-*.tsv`, `lsp-index-*.json`) until explicitly pruned.

Deployable AOT output:

- `fuse build --release` emits `.fuse/build/program.aot` (`.exe` on Windows) by default.
- `fuse build --aot` also emits AOT output (debug profile).
- `[build].native_bin` overrides the AOT output path and remains supported.
- AOT binaries embed build metadata:
  `mode`, `profile`, `target`, `rustc`, `cli`, `runtime_cache`, and `contract`.
  Use `FUSE_AOT_BUILD_INFO=1 <aot-binary>` to print this metadata and exit.
- `FUSE_AOT_STARTUP_TRACE=1 <aot-binary>` emits a startup diagnostic line with PID + build metadata.
- Startup order contract:
  `FUSE_AOT_BUILD_INFO=1` short-circuits before startup trace and before app execution.
- AOT build/link failures are deterministic command failures with `error:` diagnostics and
  `[build] failed` step footer.
- Runtime failures in AOT binaries emit a stable fatal envelope:
  `fatal: class=<runtime_fatal|panic> pid=<...> message=<...> <build-info>`.
  For `class=panic`, message starts with
  `panic_kind=<panic_static_str|panic_string|panic_non_string>`.
- AOT runtime exit codes are stable: `0` success, `1` runtime failure, `101` panic.
- Unix `SIGINT`/`SIGTERM` use deterministic graceful shutdown for service loops
  (`shutdown: runtime=<ast|native> signal=<...> handled_requests=<n>`) with clean exit.
- AOT runtime config resolution is deterministic: env -> config file (`FUSE_CONFIG` or
  `config.toml` in process cwd) -> config defaults.
- AOT runtime does not auto-load `.env`; only process environment is observed.
- Optional release logging posture: if `FUSE_AOT_REQUEST_LOG_DEFAULT=1` and `FUSE_REQUEST_LOG` is
  unset, AOT runtime defaults to structured request logs.
- AOT runtime is sealed: no dynamic backend fallback, no JIT compilation for app execution, and no
  runtime source compilation.

Use `fuse build --clean` to clear `.fuse/build`.
Use `fuse clean --cache [<path>|--manifest-path <path>]` to prune `.fuse-cache` directories
under the current root or a selected package/workspace root.

## Config loading

Config values are resolved in order:

1. Environment variables
2. Config file (`config.toml` by default; override with `FUSE_CONFIG`)
3. Default expressions in `config` blocks

The CLI loads `.env` from the package directory and sets only missing variables.

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `FUSE_DB_URL` | — | Database connection URL (`sqlite://path`) |
| `DATABASE_URL` | — | Fallback DB URL when `FUSE_DB_URL` is unset |
| `FUSE_DB_POOL_SIZE` | `1` | SQLite connection pool size |
| `FUSE_CONFIG` | `config.toml` | Config file path |
| `FUSE_HOST` | `127.0.0.1` | HTTP server bind host |
| `FUSE_SERVICE` | — | Selects service when multiple are declared |
| `FUSE_MAX_REQUESTS` | — | Stop server after N requests (useful for tests) |
| `FUSE_LOG` | `info` | Minimum log level (`trace`/`debug`/`info`/`warn`/`error`) |
| `FUSE_COLOR` | `auto` | ANSI color mode (`auto`/`always`/`never`) |
| `NO_COLOR` | — | Disables ANSI color when set |
| `FUSE_REQUEST_LOG` | — | `structured` for JSON request logging on stderr |
| `FUSE_METRICS_HOOK` | — | `stderr` for per-request metrics lines |
| `FUSE_DEV_RELOAD_WS_URL` | — | Dev HTML script injection (`/__reload` client) with reload + compile-error overlay events |
| `FUSE_OPENAPI_JSON_PATH` | — | Built-in OpenAPI JSON endpoint path |
| `FUSE_OPENAPI_UI_PATH` | — | Built-in OpenAPI UI path |
| `FUSE_ASSET_MAP` | — | Logical-path to public-URL mappings for `asset()` |
| `FUSE_VITE_PROXY_URL` | — | Fallback proxy for unknown routes to Vite dev server |
| `FUSE_SVG_DIR` | — | Override SVG base directory for `svg.inline` |
| `FUSE_STATIC_DIR` | — | Serve static files from this directory |
| `FUSE_STATIC_INDEX` | `index.html` | Fallback file for directory requests |
| `FUSE_DEV_MODE` | — | Enables development-mode runtime behavior |
| `FUSE_AOT_BUILD_INFO` | — | Print AOT build metadata and exit (AOT only) |
| `FUSE_AOT_STARTUP_TRACE` | — | Emit startup diagnostic line (AOT only) |
| `FUSE_AOT_REQUEST_LOG_DEFAULT` | — | Default to structured request logging (AOT release) |

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
| Authority parity | `./scripts/authority_parity.sh` | AST/native semantic authority plus AST/native/AOT observable parity lock (errors/JSON/logging/panic taxonomy/transaction/spawn) |
| LSP suite | `./scripts/lsp_suite.sh` | LSP contracts, navigation, completions, code actions |
| LSP performance | `./scripts/lsp_perf_reliability.sh` | Cancellation handling and responsiveness budgets |
| LSP incremental | `./scripts/lsp_workspace_incremental.sh` | Workspace cache correctness |
| Benchmarks | `./scripts/use_case_bench.sh` | Real-world workload metrics (`--median-of-3` available for reliability runs) |
| Reliability repeat | `./scripts/reliability_repeat.sh --iterations 2` | Repeat-run stability checks for parity/LSP/benchmark-sensitive paths |
| AOT startup/throughput benchmark | `./scripts/aot_perf_bench.sh` | Cold-start distribution + steady-state throughput comparison (JIT-native vs AOT) |
| AOT startup SLO gate | `./scripts/check_aot_perf_slo.sh` | Enforces `ops/AOT_RELEASE_CONTRACT.md` cold-start improvement thresholds (`p50`/`p95`) |
| Packaging verifier regression | `./scripts/packaging_verifier_regression.sh` | Cross-platform CLI+AOT archive and VSIX verifier coverage (including Windows `.exe` naming) |
| Release integrity regression | `./scripts/release_integrity_regression.sh` | Checks checksum metadata, SPDX SBOM generation, and provenance validation against fixture release bundles |
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
| Official reference image (release tags) | `ghcr.io/dmitrijkiltau/fuse-aot-demo:<tag>` |
| VS Code extension | `dist/fuse-vscode-<platform>.vsix` |
| Release checksums | `dist/SHA256SUMS` |
| Release checksum signature | `dist/SHA256SUMS.sig` |
| Release checksum certificate | `dist/SHA256SUMS.pem` |
| Release metadata | `dist/release-artifacts.json` |
| Release SBOMs | `dist/<artifact>.spdx.json` |
| Release provenance attestation | `dist/release-provenance.json` |
| Release provenance signature | `dist/release-provenance.sig` |
| Release provenance certificate | `dist/release-provenance.pem` |

Supported release matrix platforms:
`linux-x64`, `macos-arm64`, `windows-x64`.

Reproducibility + static profile policy: `ops/AOT_RELEASE_CONTRACT.md`.

```bash
# Build release binaries
./scripts/build_dist.sh --release

# Package host CLI bundle (archive + integrity check)
./scripts/package_cli_artifacts.sh --release

# Package host AOT reference bundle (archive + integrity check)
./scripts/package_aot_artifact.sh --release --manifest-path .

# Build official reference container image from release archive
./scripts/package_aot_container_image.sh --archive dist/fuse-aot-linux-x64.tar.gz --tag v0.8.0

# Package VS Code extension with bundled LSP (.vsix + integrity check)
./scripts/package_vscode_extension.sh --release

# Generate checksums and JSON metadata for release publication
./scripts/generate_release_checksums.sh

# Generate deterministic SPDX JSON SBOMs for all release payloads
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" ./scripts/generate_release_sboms.sh

# Reproducible metadata timestamp (optional)
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" ./scripts/generate_release_checksums.sh

# Install a packaged VSIX example
code --install-extension dist/fuse-vscode-linux-x64.vsix

# Regenerate GitHub guide markdown
./scripts/generate_guide_docs.sh
```

For downloaded tagged release assets:

```bash
sha256sum -c SHA256SUMS

cosign verify-blob \
  --certificate SHA256SUMS.pem \
  --signature SHA256SUMS.sig \
  --certificate-identity "https://github.com/dmitrijkiltau/fuse/.github/workflows/release-artifacts.yml@refs/tags/vX.Y.Z" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS

cosign verify-blob \
  --certificate release-provenance.pem \
  --signature release-provenance.sig \
  --certificate-identity "https://github.com/dmitrijkiltau/fuse/.github/workflows/release-artifacts.yml@refs/tags/vX.Y.Z" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  release-provenance.json
```

## Repo structure

| Path | Contents |
|---|---|
| `crates/fusec` | Compiler: parser, semantic analysis, native runtime/JIT, LSP |
| `crates/fuse` | Package-oriented CLI wrapper |
| `crates/fuse-rt` | Shared runtime library |
| `examples/` | Sample programs and packages |
| `guides/` | GitHub-facing guide markdown (generated + migration docs) |
| `tools/vscode/` | VS Code extension (syntax highlighting + LSP client) |
| `spec/` | Normative language/runtime contracts |
| `ops/` | Release/incident contracts |
| `governance/` | Identity/policy/process |

## Documentation map

README is the single onboarding front door. Start here, then follow concern ownership below.
If two documents disagree, defer to the owning document listed for that tier.

### Spec contracts (normative)

| Document | Scope |
|---|---|
| `spec/fls.md` | Formal language specification (syntax, grammar, AST, type system) |
| `spec/runtime.md` | Runtime semantics (validation, JSON, config, HTTP, builtins, DB) |

### Product and guides (non-normative)

| Document | Scope |
|---|---|
| `guides/onboarding.md` | Onboarding walkthrough |
| `guides/reference.md` | Generated developer reference |
| `guides/migrations/0.8-to-0.9.md` | Migration guide for `0.8.x -> 0.9.0` |
| `guides/migrations/0.1-to-0.2.md` | Migration guide for `0.1.x -> 0.2.0` |

### Operations contracts

| Document | Scope |
|---|---|
| `ops/AOT_RELEASE_CONTRACT.md` | AOT production release contract, SLO thresholds, and reproducibility policy |
| `ops/AOT_ROLLBACK_PLAYBOOK.md` | Incident rollback plan (AOT primary, JIT-native fallback) |
| `ops/DEPLOY.md` | Canonical deployment guide (VM, Docker, systemd, Kubernetes) |
| `ops/RELEASE.md` | Release checklist and publication workflow |
| `ops/FLAKE_TRIAGE.md` | Checklist for diagnosing and closing intermittent CI/test failures |
| `ops/BENCHMARKS.md` | Workload matrix and benchmark definitions |

### Governance and policy

| Document | Scope |
|---|---|
| `governance/scope.md` | Project constraints, roadmap priorities, and explicit non-goals |
| `governance/IDENTITY_CHARTER.md` | Language identity boundaries and "will not do" list |
| `governance/EXTENSIBILITY_BOUNDARIES.md` | Allowed extension surfaces and stability tiers |
| `governance/VERSIONING_POLICY.md` | Compatibility guarantees and deprecation rules |
| `governance/LSP_ROADMAP.md` | Editor capability baseline and planned improvements |

### Contribution process

| Document | Scope |
|---|---|
| `CONTRIBUTING.md` | Contribution standards, required checks, and RFC criteria |
| `GOVERNANCE.md` | Maintainer roles, decision model, and escalation |
| `CODE_OF_CONDUCT.md` | Contributor behavior expectations |
| `SECURITY.md` | Vulnerability disclosure and response policy |
| `rfcs/` | RFC process, template, and index |

## License

Apache-2.0. See `LICENSE`.
