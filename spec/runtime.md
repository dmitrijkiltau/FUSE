# Runtime semantics (current implementation)

This document is the canonical source for runtime behavior in this repo.
It describes the AST interpreter and native backend path semantics.

Companion references:

- `fls.md` defines syntax and static-semantics guarantees.
- `../governance/scope.md` captures roadmap direction and non-goals.

## Document contract

- `Normative`: Yes.
- `Front door`: No. Start onboarding from `../README.md`.
- `Owned concerns`: backend runtime behavior, boundary binding (validation/JSON/config/CLI/HTTP),
  runtime error model, builtins, DB/migrations/tests execution, and concurrency semantics.
- `Conflict policy`: syntax/static semantics defer to `fls.md`; roadmap/planning defers to
  `../governance/scope.md`; release-operations policy defers to `../ops/AOT_RELEASE_CONTRACT.md`
  and `../ops/RELEASE.md`.
- `Exclusions`: grammar, AST shape, and parsing rules are owned by `fls.md`; project planning by
  `../governance/scope.md`.

Normative terms in this document:

- `must` / `must not` indicate required runtime behavior
- `may` indicates allowed implementation latitude that preserves observable behavior

---

## Backends

- **AST interpreter**: executes parsed AST directly.
- **Native**: uses a compiled native image (`program.native`) and IR-compatible runtime semantics,
  with a Cranelift JIT fast-path for direct function execution.

Most runtime behavior is shared across backends.

Semantic authority contract:

- parser + frontend canonicalization define language semantics
- canonical AST/lowered forms are the semantic program seen by all backends
- native is an execution strategy over that canonical program
- backend-specific reinterpretation of source syntax is considered a bug
- shared runtime semantics (call binding, decode/validate/JSON conversion) are centralized and consumed by AST/native paths

Canonical relationship:

```text
Source -> Parser -> AST -> Lowering passes -> Canonical program
                                           -> Native execution
```

Native backend note:

- If native compilation/execution fails for a function, the run fails (no automatic backend fallback).
- `fuse build --release` emits a standalone native executable wrapper over compiled native artifacts.
  `fuse build --aot` also emits AOT output (debug profile).
  The wrapper embeds build metadata (`mode`, `profile`, `target`, `rustc`, `cli`, `runtime_cache`, `contract`),
  exposes it via `FUSE_AOT_BUILD_INFO=1`, supports startup tracing via `FUSE_AOT_STARTUP_TRACE=1`,
  and emits fatal envelopes as:
  `fatal: class=<runtime_fatal|panic> pid=<...> message=<...> <build-info>`.
  For `class=panic`, `message` starts with
  `panic_kind=<panic_static_str|panic_string|panic_non_string>`.
- AOT fallback is an operational decision (not automatic); incident fallback guidance is tracked in
  `../ops/AOT_ROLLBACK_PLAYBOOK.md`.

### AOT runtime contract (`v0.7.0` baseline)

This section freezes runtime guarantees for binaries emitted by
`fuse build --release` and `fuse build --aot`.

Startup order guarantees:

1. If `FUSE_AOT_BUILD_INFO=1`, the binary must print exactly one build-info line to stdout and
   exit with code `0`.
2. In build-info mode, startup tracing and program execution must not run.
3. Otherwise, if `FUSE_AOT_STARTUP_TRACE=1`, the binary must emit one startup line on stderr
   before loading configs/types and before executing app logic.
   Startup line format is stable:
   `startup: pid=<pid> mode=<...> profile=<...> target=<...> rustc=<...> cli=<...> runtime_cache=<...> contract=<...>`.
4. Runtime then loads embedded type metadata, resolves config values, and executes the compiled app
   entrypoint.

Signal handling semantics:

- service runtime installs deterministic graceful handlers for `SIGINT` and `SIGTERM` on Unix.
- on signal, service loops stop accepting new requests and exit cleanly with status `0`.
- graceful signal shutdown is logged as:
  `shutdown: runtime=<ast|native> signal=<SIGINT|SIGTERM|unknown> handled_requests=<n>`.
- graceful signal shutdown is not emitted as a fatal envelope.

Shutdown and exit-code contract:

- success exit: `0`
- handled runtime failure (`class=runtime_fatal`): `1`
- process panic (`class=panic`): `101`
- AOT build/link failures are CLI build-time failures, not runtime exits from the produced binary.

Fatal envelope invariants:

- fatal lines must use:
  `fatal: class=<runtime_fatal|panic> pid=<...> message=<...> mode=<...> profile=<...> target=<...> rustc=<...> cli=<...> runtime_cache=<...> contract=<...>`
- `message` is single-line sanitized text (`\n`/`\r` escaped).
- `class=panic` messages must start with
  `panic_kind=<panic_static_str|panic_string|panic_non_string>`.

Observability consistency guarantees:

- HTTP request ID propagation and structured request logging semantics are identical to
  [Observability baseline](#observability-baseline).
- release AOT binaries may default request logging to structured mode when
  `FUSE_AOT_REQUEST_LOG_DEFAULT` is truthy and `FUSE_REQUEST_LOG` is unset.
- Startup trace and fatal envelopes must include the same build-info key set
  (`mode`, `profile`, `target`, `rustc`, `cli`, `runtime_cache`, `contract`).

Deterministic config resolution order in AOT binaries:

1. process environment variable override
2. config file (`FUSE_CONFIG`; default `config.toml` in process working directory)
3. config field default expression

AOT runtime does not implicitly load `.env`; only the invoking environment is observed.

Backend/runtime sealing guarantees:

- no dynamic backend fallback in AOT runtime
- no JIT compilation for app execution in AOT runtime
- no source-level runtime compilation in AOT binaries
- no runtime reinterpretation of source syntax by backend-specific fallback logic

### Function symbol resolution

Function symbols are module-scoped. Resolution rules (unqualified, qualified, duplicate names)
are defined in [Imports and modules](fls.md#imports-and-modules-current).

---

## Expression operator behavior

Comparison behavior is shared across AST/native backends:

- `==` / `!=` support same-typed pairs for `Int`, `Float`, `Bool`, `String`, and `Bytes`.
- `<`, `<=`, `>`, `>=` support numeric pairs (`Int`, `Float`) only.
- unsupported comparison operand pairs produce runtime errors.

---

## Error model

### Recognized error names

The runtime recognizes a small set of error struct names for standardized HTTP status mapping
and error JSON formatting.

Preferred canonical names (from `std.Error`):

- `std.Error.Validation`
- `std.Error`
- `std.Error.BadRequest`
- `std.Error.Unauthorized`
- `std.Error.Forbidden`
- `std.Error.NotFound`
- `std.Error.Conflict`

Compatibility short names are also recognized (`Validation`, `Error`, `BadRequest`,
`Unauthorized`, `Forbidden`, `NotFound`, `Conflict`), which commonly occur after named imports.
Other names do not participate in standardized mapping/formatting behavior.

### Error JSON shape

Errors are rendered as JSON with a single `error` object:

```json
{
  "error": {
    "code": "validation_error",
    "message": "validation failed",
    "fields": [
      { "path": "email", "code": "invalid_value", "message": "invalid email address" }
    ]
  }
}
```

Rules:

- `std.Error.Validation` / `Validation` uses `message` and `fields`
  (list of structs with `path`, `code`, `message`).
- `std.Error` / `Error` uses `code` and `message`. Other fields are ignored for JSON output.
- `std.Error.BadRequest` / `BadRequest`, `std.Error.Unauthorized` / `Unauthorized`,
  `std.Error.Forbidden` / `Forbidden`, `std.Error.NotFound` / `NotFound`,
  `std.Error.Conflict` / `Conflict` use their `message` field if present, otherwise a default message.
- Any other error value renders as `internal_error`.

### HTTP status mapping

Status mapping uses the error name first, then `std.Error.status` if present:

- `std.Error.Validation` / `Validation` -> 400
- `std.Error.BadRequest` / `BadRequest` -> 400
- `std.Error.Unauthorized` / `Unauthorized` -> 401
- `std.Error.Forbidden` / `Forbidden` -> 403
- `std.Error.NotFound` / `NotFound` -> 404
- `std.Error.Conflict` / `Conflict` -> 409
- `std.Error` / `Error` with `status: Int` -> that status
- anything else -> 500

### Result types + `?!`

- `T!E` is `Result<T, E>`.
- `T!` is a compile-time error (explicit error domains are required).
- for function/service return boundaries, each error domain must be a declared nominal `type` or `enum`

`expr ?! err` rules:

- If `expr` is `Option<T>` and is `None`, return `Err(err)`.
- If `expr` is `Result<T, E>` and is `Err`, replace the error with `err`.
- If `expr ?!` omits `err`, `Result` propagates the existing error.
- `Option<T> ?!` without an explicit `err` is a compile-time error.

See also: [Boundary model](#boundary-model), [Type system (current static model)](fls.md#type-system-current-static-model).

---

## Boundary model

### Validation

Validation is applied at runtime in these places:

- struct literal construction (`Type(...)`)
- JSON decode for HTTP body
- config loading
- CLI flag binding
- route parameter parsing

There is no global "validate on assignment" mode.

#### Default values

Defaults are applied before validation:

- missing field with default -> default is used
- missing optional field -> `null`
- explicit `null` stays `null` (even if a default exists)

#### Built-in refinements

Refinements support range, regex, and predicate constraints:

- `String(1..80)` length constraint
- `String(regex("^[a-z0-9_-]+$"))` pattern constraint
- `String(1..80, regex("^[a-z]"), predicate(is_slug))` mixed constraints, left-to-right
- `Int(0..130)` numeric range
- `Float(0.0..1.0)` numeric range

Rules:

- `regex("...")` is valid on string-like refined bases (`String`, `Id`, `Email`).
- `predicate(fn_name)` requires a function signature `fn(<base>) -> Bool`.

#### `Id` and `Email`

- `Id` is a non-empty string.
- `Email` uses a simple `local@domain` check with a `.` in the domain.

### JSON encoding/decoding

#### Structs

- encode to JSON objects with declared field names
- all fields are included (including defaults)
- `null` represents optional empty value

#### Struct decoding

- missing field with default -> default value
- missing field with no default -> error
- optional fields accept missing or `null`
- unknown fields -> error

#### Enums

Enums use a tagged object format:

```json
{ "type": "Variant", "data": ... }
```

Rules:

- no payload: omit `data`
- single payload: `data` is the value
- multiple payloads: `data` is an array

#### Built-in types and generics

- `String`, `Id`, `Email` -> JSON string
- `Bytes` -> JSON base64 string (standard alphabet with `=` padding)
- `Html` -> JSON string via `html.render(...)` output
- `Bool`, `Int`, `Float` -> JSON number/bool
- `List<T>` -> JSON array
- `Map<K,V>` -> JSON object (runtime requires `K = String`)
- user-defined `struct` and `enum` decode with same validation model as struct literals
- `Result<T,E>` -> tagged object:
  - `{"type":"Ok","data":...}` decodes as `Ok(T)`
  - `{"type":"Err","data":...}` decodes as `Err(E)`

`Bytes` use base64 text at JSON/config/CLI boundaries. Runtime values are raw bytes.
`Html` values are runtime trees and are not parsed from config/env/CLI.

### Config loading

Config values resolve in this order:

1. environment variables (override config file)
2. config file (default `config.toml`, overridable via `FUSE_CONFIG`)
3. default expressions

The `fuse` CLI also loads `.env` from the package directory (if present) and injects any missing
variables before this resolution. Existing environment variables are never overridden by `.env`.

Config file format is a minimal TOML-like subset:

```toml
[App]
port = 3000
dbUrl = "sqlite://app.db"
```

Notes:

- only section headers and `key = value` pairs are supported
- values are parsed as strings (with basic `"` escapes), then converted using env-var conversion rules

Env override naming derives from config and field names:

- `App.port` -> `APP_PORT`
- `dbUrl` -> `DB_URL`
- hyphens become underscores; camelCase splits to `SNAKE_CASE`

Type support levels for config values (env and file values):

- **Full**: scalars (`Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`) and `Option<T>`.
- **Structured via JSON text**: `List<T>`, `Map<String,V>`, user-defined `struct`, user-defined `enum`.
- **Rejected**: `Html`, `Map<K,V>` where `K != String`, `Result<T,E>`.

Compatibility notes:

- `Bytes` must be valid base64 text; invalid base64 is a validation error.
- for structured values, parse failures (invalid JSON/type mismatch/unknown field) surface as
  validation errors on the target field path.

### CLI binding

CLI binding is enabled when program args are passed after the file (or after `--`):

```bash
fusec --run file.fuse -- --name=Codex
```

Rules:

- flags only (no positional arguments)
- `--flag value` and `--flag=value` are supported
- `--flag` sets `Bool` to `true`; `--no-flag` sets it to `false`
- unknown flags are validation errors
- multiple values for the same flag are rejected
- binding calls `fn main` from the root module directly; `app` block is ignored when program args are present

Type support levels mirror config/env parsing:

- **Full**: scalar types and `Option<T>`.
- **Structured via JSON text**: `List<T>`, `Map<String,V>`, user-defined `struct`, user-defined `enum`.
- **Rejected**: `Html`, `Map<K,V>` with non-`String` keys, `Result<T,E>`.

For `Bytes`, CLI values must be base64 text.

Validation errors are printed as JSON on stderr and usually exit with code 2.

`fuse` CLI wrapper output contract (`check|run|build|test`):

- emits stderr step markers: `[command] start` and `[command] ok|failed|validation failed`
- keeps JSON validation payloads uncolored/machine-readable
- `run` CLI argument validation failures exit with code `2`

### HTTP runtime

#### Routing

- paths are split on `/` and matched segment-by-segment
- route params use `{name: Type}` and must occupy the whole segment
- params parse with env-like scalar/optional/refined rules
- `body` introduces a JSON request body bound to `body` in the handler

#### Response

- successful values encode as JSON with `Content-Type: application/json` by default
- if route return type is `Html` (or `Result<Html, E>` on success), response is rendered once with
  `Content-Type: text/html; charset=utf-8`
- route handlers may append response headers via `response.header(name, value)`
- route handlers may manage cookies via `response.cookie(name, value)` and
  `response.delete_cookie(name)` (emitted as `Set-Cookie` headers)
- `Result` errors are mapped using the status rules above
- unsupported HTTP methods return `405` with `internal_error` JSON
- no HTMX-specific runtime mode: HTMX-style flows are ordinary `Html` route returns

#### Request primitives

- route handlers may read inbound headers with `request.header(name)` (case-insensitive)
- route handlers may read cookie values with `request.cookie(name)`
- `request.*` and `response.*` primitives are only valid while evaluating an HTTP route handler

#### Environment knobs

- `FUSE_HOST` (default `127.0.0.1`) controls bind host
- `FUSE_SERVICE` selects service when multiple are declared
- `FUSE_MAX_REQUESTS` stops server after N requests (useful for tests)
- `FUSE_DEV_RELOAD_WS_URL` enables dev HTML script injection (`/__reload` client)
- `FUSE_OPENAPI_JSON_PATH` + `FUSE_OPENAPI_UI_PATH` enable built-in OpenAPI UI serving
- `FUSE_ASSET_MAP` provides logical-path -> public-URL mappings for `asset(path)`
- `FUSE_VITE_PROXY_URL` enables fallback proxying of unknown routes to Vite dev server
- `FUSE_SVG_DIR` overrides SVG base directory for `svg.inline`
- `FUSE_STATIC_DIR` serves static files from the given directory at the service base path
- `FUSE_STATIC_INDEX` (default `index.html`) sets the fallback file served for directory requests
  when `FUSE_STATIC_DIR` is configured
- `FUSE_AOT_REQUEST_LOG_DEFAULT` (AOT release only) enables structured request logging default
  when `FUSE_REQUEST_LOG` is unset

#### Observability baseline

Request ID propagation:

- each HTTP request resolves one request ID with precedence:
  1. inbound `x-request-id` (if valid)
  2. inbound `x-correlation-id` (if valid)
  3. runtime-generated ID (`req-<hex>`)
- runtime normalizes the resolved value into request headers, so
  `request.header("x-request-id")` returns the lifecycle request ID inside route handlers
- runtime emits `X-Request-Id` on HTTP responses for runtime-owned handlers and runtime-generated
  status/error responses
- Vite proxy fallback forwards `X-Request-Id` upstream and injects it into the proxied response

Structured request logging mode:

- opt-in via `FUSE_REQUEST_LOG=structured` (`1`/`true` are also accepted)
- emits one JSON line to stderr per handled request with stable fields:
  `event`, `runtime`, `request_id`, `method`, `path`, `status`, `duration_ms`, `response_bytes`
- disabled by default; does not change runtime semantics
- release AOT binaries support optional default posture:
  if `FUSE_AOT_REQUEST_LOG_DEFAULT` is truthy and `FUSE_REQUEST_LOG` is unset,
  runtime sets `FUSE_REQUEST_LOG=structured` before startup

Metrics hook extension point (non-semantic):

- opt-in via `FUSE_METRICS_HOOK=stderr`
- emits one line per handled request on stderr as:
  `metrics: <json>`
- stable JSON fields:
  `metric` (`http.server.request`), `runtime`, `request_id`, `method`, `path`, `status`,
  `duration_ms`
- unsupported/empty hook values are treated as no-op
- hook emission is best-effort and must not change request/response behavior

Deterministic panic taxonomy:

- fatal envelope class remains `runtime_fatal` for handled runtime errors and `panic` for
  process-level panics
- `panic` envelope messages include `panic_kind=<panic_static_str|panic_string|panic_non_string>`
  for deterministic panic payload classification

Production health route convention (non-built-in):

- runtime does not auto-register `/health`.
- canonical minimal route pattern is:
  `get "/health" -> Map<String, String>: return {"status": "ok"}`
- production guidance should treat this pattern as the default liveness/readiness contract unless a
  service-specific contract is documented.

Explicit non-goal:

- no runtime plugin extension system (no runtime-loaded plugin/module capability).

See also: [Builtins and runtime subsystems](#builtins-and-runtime-subsystems), [Services and declaration syntax](fls.md#services-and-declaration-syntax).

---

## Builtins and runtime subsystems

### Builtins (current)

- `print(value)` prints stringified value to stdout
- `input(prompt: String = "") -> String` prints optional prompt and reads one line from stdin
- `log(...)` writes log lines to stderr (see Logging)
- `db.exec/query/one` execute SQL against configured DB
- `db.from(table)` builds parameterized queries
- `transaction:` opens a constrained DB transaction scope (`BEGIN`/`COMMIT`/`ROLLBACK`)
- `assert(cond, message?)` throws runtime error when `cond` is false
- `env(name: String) -> String?` returns env var or `null`
- `asset(path: String) -> String` resolves to hashed/static public URL when asset map is configured
- `serve(port)` starts HTTP server on `FUSE_HOST:port`
- `request.header(name: String) -> String?` reads inbound HTTP headers
- `request.cookie(name: String) -> String?` reads inbound HTTP cookie values
- `response.header(name: String, value: String)` appends response headers
- `response.cookie(name: String, value: String)` appends HTTP-only session cookies
- `response.delete_cookie(name: String)` emits cookie expiration headers
- HTML tag builtins (`html`, `head`, `body`, `div`, `meta`, `button`, ...)
- `html.text`, `html.raw`, `html.node`, `html.render`
- `svg.inline(path: String) -> Html`
- `json.encode(value) -> String` serializes a value to a JSON string
- `json.decode(text: String) -> Value` parses a JSON string into a runtime value

`input` behavior notes:

- prompt text is written without a trailing newline
- trailing `\n`/`\r\n` is trimmed from the returned line
- in non-interactive mode with no stdin data, runtime fails with:
  `input requires stdin data in non-interactive mode`
- `input()` / `input("...")` resolve to the CLI input builtin; HTML input tags remain available
  through tag-form calls such as `input(type="text")`

Compile-time sugar affecting HTML builtins:

- HTML block syntax (`div(): ...`) lowers to normal calls with explicit attrs + `List<Html>` children
- bare string literals in HTML blocks lower to `html.text(...)`
- attribute shorthand (`div(class="hero")`) lowers to attrs maps

### Compile-time capability requirements

Static capability checks are enforced by semantic analysis (see `fls.md`) and have no runtime
fallback behavior.

- modules declare capabilities with top-level `requires` declarations
- `db.exec/query/one/from` calls require `requires db`
- `serve(...)` calls require `requires network`
- `time(...)` / `time.*` calls require `requires time` (capability placeholder; no runtime API yet)
- `crypto.*` calls require `requires crypto` (capability placeholder; no runtime API yet)
- calls to imported module functions require the caller to declare the callee module's capabilities
- `transaction:` blocks require `requires db`, forbid non-`db` module capabilities, and reject
  non-`db` capability usage inside the block
- `--strict-architecture` enables additional compile-time checks:
  - capability purity (no unused declared capabilities)
  - cross-layer import-cycle rejection
  - error-domain isolation across module boundaries

### Database (SQLite only)

Database access is intentionally minimal and currently uses SQLite via a pooled set of
connections.

Configuration sources:

- `FUSE_DB_URL` (preferred) or `DATABASE_URL`
- `App.dbUrl` if config has been loaded
- `FUSE_DB_POOL_SIZE` (default `1`) for pool sizing
- `App.dbPoolSize` as optional fallback when `FUSE_DB_POOL_SIZE` is unset

URL format:

- `sqlite://path` or `sqlite:path`

Builtins:

- `db.exec(sql, params?)` executes SQL batch (no return value)
- `db.query(sql, params?)` returns `List<Map<String, Value>>`
- `db.one(sql, params?)` returns first row map or `null`
- `db.from(table)` returns `Query` builder
- `transaction:` opens a transaction, executes its block, commits on success, and rolls back on
  block failure

Query builder methods (immutable style; each returns a new `Query`):

- `Query.select(columns)`
- `Query.where(column, op, value)`
- `Query.order_by(column, dir)` where `dir` is `asc`/`desc`
- `Query.limit(n)` where `n >= 0`
- `Query.one()`
- `Query.all()`
- `Query.exec()`
- `Query.sql()` and `Query.params()` for inspection/debugging

Parameter binding:

- SQL uses positional `?` placeholders with `List` params
- supported param types: `null`, `Int`, `Float`, `Bool`, `String`, `Bytes`
  (boxed/results are unwrapped)
- `in` expects non-empty list and expands to `IN (?, ?, ...)`

Identifier constraints:

- table/column names must be identifiers (`col` or `table.col`)
- `where` operators: `=`, `!=`, `<`, `<=`, `>`, `>=`, `like`, `in` (case-insensitive)
- `order_by` direction: `asc` or `desc`

Value mapping:

- `NULL` -> `null`
- integers -> `Int`
- reals -> `Float`
- text -> `String`
- blobs -> `Bytes`

Connection pool behavior:

- DB calls use pooled SQLite connections.
- the active connection is pinned for migration and `transaction:` scopes (`BEGIN`/`COMMIT`/`ROLLBACK`).
- pool-size values must be integer `>= 1`; invalid values report runtime/config errors.

### Migrations

`migration <name>:` declares a migration block.

Run migrations with:

```bash
fusec --migrate path/to/file.fuse
```

Rules:

- migrations are collected from all loaded modules
- run order is ascending by migration name
- applied migrations are tracked in `__fuse_migrations`
- only up migrations exist today (no down/rollback)
- migrations execute via AST interpreter

### Tests

`test "name":` declares a test block.

Run tests with:

```bash
fusec --test path/to/file.fuse
```

Rules:

- tests are collected from all loaded modules
- run order is ascending by test name
- tests execute via AST interpreter
- failures report non-zero exit

### Concurrency

`spawn:` creates a task and returns `Task<T>` where `T` is block result.
Spawned tasks run on a shared worker pool. Execution is asynchronous relative to the caller
and may overlap with other spawned tasks.

`await expr` waits on a task and yields its result.

Structured concurrency is enforced at compile time:

- detached task expressions are invalid
- spawned task bindings must be awaited before scope exit
- spawned task bindings cannot be reassigned before `await`
- `transaction:` blocks reject `spawn` and `await`

Task surface (v0.2.0):

- `Task<T>` remains an opaque runtime type
- task helper builtins were removed (`task.id`, `task.done`, `task.cancel`)
- task values are consumed via `await` only

Spawn determinism restrictions are enforced at compile time by semantic analysis.
See [Spawn static restrictions](fls.md#spawn-static-restrictions-v020) for the full list.

`box expr` creates a shared mutable cell. Boxed values are transparently dereferenced in most
expressions; assigning boxed bindings updates shared cell state. `spawn` blocks cannot capture or
use boxed state.

### Loops

- `for` iterates over `List<T>` and `Map<K, V>` values (map iteration yields values)
- `break` exits nearest loop
- `continue` skips to next iteration

### Indexing

- `list[idx]` reads list element; `idx` must be in-bounds `Int`
- out-of-bounds list access raises runtime error
- `map[key]` reads map element; missing key yields `null`

Assignment targets allow:

- `list[idx] = value` (bounds-checked)
- `map[key] = value` (insert/overwrite)

Optional access in assignment targets (for example `foo?.bar = x`, `items?[0] = x`) errors when base is `null`.

### Ranges

`a..b` evaluates to inclusive numeric `List`.

- only numeric bounds are allowed
- if `a > b`, runtime error
- float ranges step by `1.0` (for example `1.5..3.5` -> `[1.5, 2.5, 3.5]`)

### Logging

`log` is a minimal runtime logging builtin shared by all backends.

Usage:

- `log("message")` logs at `INFO`
- `log("warn", "message")` logs at `WARN`
- if there are 2+ args and first arg is known level (`trace`, `debug`, `info`, `warn`, `error`),
  it is treated as level; the rest are stringified and joined with spaces
- if there is at least one extra argument after the message, `log` emits JSON

Output:

- `[LEVEL] message` to stderr
  (`LEVEL` token may be ANSI-colored; honors `FUSE_COLOR=auto|always|never` and `NO_COLOR`)
- JSON logs are emitted as a single stderr line

Filtering:

- `FUSE_LOG` sets minimum level (default `info`)

Structured logging:

- `log("info", "message", data)` emits JSON:
  `{"level":"info","message":"message","data":<json>}`
- if multiple data values are provided, `data` is a JSON array

See also: [Boundary model](#boundary-model), [Scope + constraints](../governance/scope.md), [README](../README.md).
