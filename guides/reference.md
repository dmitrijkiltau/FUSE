# FUSE Developer Reference

_Auto-generated from `spec/fls.md`, `spec/runtime.md`, and `governance/scope.md` by `scripts/generate_guide_docs.sh`._

This document is the reference for building applications with FUSE.
If you are new to FUSE, start with [Onboarding Guide](onboarding.md) and [Boundary Contracts](boundary-contracts.md) before this reference.

---

## Install and Downloads

Release artifacts are published on GitHub Releases:

- https://github.com/dmitrijkiltau/FUSE/releases

---

## Language at a Glance

Top-level declarations:

- `import`
- `fn`
- `type`
- `enum`
- `config`
- `service`
- `app`
- `migration`
- `test`

Core statements:

- `let` / `var`
- assignment
- `if` / `else`
- `match`
- `for` / `while`
- `break` / `continue`
- `return`

Core expression features:

- null-coalescing: `??`
- optional access: `?.`, `?[idx]`
- bang-chain conversion: `?!`
- ranges: `a..b`
- concurrency forms: `spawn`, `await`, `box`

---
## Types

- `Int`, `Float`, `Bool`, `String`, `Bytes`, `Html`
- `Id`, `Email`
- `Error`
- `List<T>`, `Map<K,V>`, `Option<T>`, `Result<T,E>`
- user-defined `type` and `enum` are nominal

Reserved namespace:

- `std.Error.*` is reserved for standardized runtime error behavior.

Type shorthand:

- `T?` desugars to `Option<T>`.
- `null` is the optional empty value.
- `x ?? y` is null-coalescing.
- `x?.field` and `x?[idx]` are optional access forms.
- `Some` / `None` are valid match patterns.

Result types:

- `T!E` desugars to `Result<T, E>`.
- `T!` is invalid; result types must declare an explicit error domain.
- for function/service return boundaries, each `E` must be a declared nominal `type` or `enum` (including chained forms like `T!AuthError!DbError`)
- `expr ?! err` applies bang-chain error conversion.
- `expr ?!` is propagation-only for `Result<T,E>`; `Option<T> ?!` requires an explicit `err`.

Refinements:

Refinements attach predicates to primitive base types in type positions:

- `String(1..80)`
- `Int(0..130)`
- `Float(0.0..1.0)`
- `String(regex("^[a-z0-9_-]+$"))`
- `String(1..80, regex("^[a-z]"), predicate(is_slug))`

Constraint forms:

- range literals (`1..80`, `0..130`, `0.0..1.0`)
- `regex("<pattern>")` on string-like bases
- `predicate(<fn_ident>)` where the function signature is `fn(<base>) -> Bool`

### Type inference

- local inference for `let` / `var`
- function parameter types are required
- function return type is optional

### Comparison operators

- Equality operators (`==`, `!=`) are defined for same-typed scalar pairs:
  `Int`, `Float`, `Bool`, `String`, and `Bytes`.
- Relational operators (`<`, `<=`, `>`, `>=`) are defined for numeric pairs (`Int`, `Float`).
- Comparisons outside supported operand pairs are invalid.

Runtime error behavior for unsupported pairs is defined in
[Expression operator behavior](runtime.md#expression-operator-behavior).

Type derivation:

`type PublicUser = User without password, secret` creates a new nominal type derived from `User`
with listed fields removed. Field types/defaults are preserved for retained fields.

Base types can be module-qualified (`Foo.User`). Unknown base types or fields are errors.

---

## Strings, Interpolation, and Comments

- String forms:
  - standard double-quoted strings: `"hello"`
  - triple-quoted strings: `"""hello\nworld"""` (multiline allowed)
- Escapes: `\n`, `\t`, `\r`, `\\`, `\"`. Unknown escapes pass through (`\$` produces `$`).
- Interpolation: `${expr}` inside both string forms.

- Line comment: `# ...`
- Doc comment: `## ...` attaches to the next declaration

## Indentation

FUSE uses Python-style block structure with strict space rules.

- Indentation is measured in spaces only (tabs are illegal).
- Indent width is not fixed, but must be consistent within a file.
- A block starts after `:` at end of line.
- New indentation level must be strictly greater than previous.
- Dedent closes blocks until indentation matches a previous level.
- Empty lines are ignored.
- Lines inside parentheses/brackets/braces ignore indentation semantics (implicit line joining).

INDENT/DEDENT reference algorithm:

- Maintain a stack `indents` starting with `[0]`.
- For each logical line not inside `()[]{}`:
  - Let `col` be count of leading spaces.
  - If `col > top(indents)`: emit `INDENT`, push `col`.
  - If `col < top(indents)`: while `col < top(indents)` emit `DEDENT`, pop;
    error if `col != top(indents)` after popping.
  - Else: continue.

---

## Match and Patterns

`match` executes the first case whose pattern matches the value.

Case forms:

- `Pattern -> Expr` is a single-expression case (sugar for `Pattern: return Expr`).
- `Pattern:` followed by an indented block is the full block form.

Pattern forms:

- `_` — wildcard, matches any value
- `Literal` — integer, float, string, or bool literal
- `None` — matches optional empty value
- `Some(x)` — matches optional present value, binds the payload to `x`
- `Ok(x)` / `Err(e)` — matches result variants, binds the payload
- `EnumVariant` — matches a no-payload enum variant by name
- `EnumVariant(x, y)` — matches an enum variant with positional payload bindings
- `TypeName(field = pattern, ...)` — matches struct fields by name

---
## Imports and Modules

`import` declarations are resolved at load time.

Import path classification:

- paths with no explicit extension are module imports and default to `.fuse`
- explicit `.fuse` paths are module imports
- explicit `.md` paths are Markdown asset imports
- explicit `.json` paths are JSON asset imports
- any other explicit extension in import position is rejected as unsupported

Module imports register an alias for qualified access (`Foo.bar`, `Foo.Config.field`, `Foo.Enum.Variant`).
Named imports bring specific items into local scope.

Module import resolution rules:

- `import Foo` loads `Foo.fuse` from the current file directory.
- `import X from "path"` loads `path` relative to current file; `.fuse` is added if missing.
- `import {A, B} from "path"` loads module and imports listed names into local scope.
- `import X as Y from "path"` loads `path` and registers the module under alias `Y` for qualified access.
- `import X from "root:path/to/module"` loads from package root (`fuse.toml` directory); if no manifest is found, root falls back to the entry module directory.

Asset import rules:

- asset imports are supported only as `import Name from "path.ext"` where `ext` is `.md` or `.json`
- `root:` and `dep:` path resolution apply to asset imports using the same repository/package rules
  as module imports
- `.md` imports bind a local immutable `String` containing the exact UTF-8 file contents
- `.json` imports bind a local immutable runtime value equivalent to `json.decode(text)`; static
  typing remains intentionally conservative
- asset imports are values, not modules: they do not create a namespace and do not expose named exports
- `import {A, B} from "./data.json"` and `import X as Y from "./data.json"` are load-time errors

Notes:

- module imports do not automatically import all members into local scope
- named imports do not create a module alias
- asset imports do not create a module alias or a named-export set
- function symbols are module-scoped (not global across all loaded modules)
- unqualified function calls resolve in this order: current module, then named imports
- module-qualified calls (`Foo.bar`) resolve against the referenced module alias
- duplicate imported binding names in one module are load-time errors
- duplicate function names across different modules are allowed
- module-qualified type references are valid in type positions (`Foo.User`, `Foo.Config`)
- dependency imports use `dep:` import paths (for example, `dep:Auth/lib` or `dep:Fixtures/data.json`)
- root-qualified imports use `root:` import paths (for example, `root:lib/auth` or `root:content/policy.md`)
- missing asset files, unreadable files, invalid UTF-8, invalid JSON syntax, unsupported asset
  forms, and unsupported explicit extensions are load-time diagnostics attached to the import path

Package dependency resolution (`dep:` imports):

- dependencies are declared in `fuse.toml` under `[dependencies]` using any of three syntaxes:
  `Auth = "./deps/auth"` (bare path), `Auth = { path = "./deps/auth" }` (inline table),
  `[dependencies.Auth] path = "./deps/auth"` (section table).
- `dep:<Name>/<path>` resolves against `<dep-root>` using ordinary import classification:
  module targets default to `.fuse` when no extension is present, while explicit `.md` / `.json`
  targets remain asset imports.
- dependency resolution is transitive: each dependency's own `fuse.toml` is read and its
  named sub-dependencies are merged into the consumer's dep map; the direct consumer's deps
  always shadow any same-named sub-dependencies.
- cross-package dependency cycles are a load-time error; the diagnostic identifies the full
  cycle path with `→` separators (for example, `circular import: A → B → A`).
- attempting to use an undeclared dependency name emits a structured error naming the unknown
  dep and listing all declared deps (for example, `unknown dependency 'Foo' — available: Auth, Math`).
- `fuse deps lock` rewrites `fuse.lock` for the selected package to match the resolved
  dependency graph.
- `fuse deps lock --check` must fail with code `FUSE_LOCK_OUT_OF_DATE` when the current
  lockfile differs from the resolved dependency graph.
- `fuse check|run|build|test --frozen` must fail with code `FUSE_LOCK_FROZEN` before command
  execution if dependency resolution would change `fuse.lock`.
- `fuse deps publish-check` walks all `fuse.toml` files under the selected root and reports
  per-package manifest-entry or lock-readiness failures.
- `fuse clean --cache` removes `.fuse-cache` directories under the selected root; when no path
  is supplied it uses the current working directory, and `--manifest-path <path>` may point to
  either a package directory or a `fuse.toml` file.
- the `fuse check --workspace` flag walks the directory tree from the current working directory,
  discovers all `fuse.toml` manifests that declare a `[package].entry`, and checks each package
  independently; results are summarised with a per-package pass/fail line followed by a total.
- in `--workspace` mode, a lightweight file-timestamp cache (`.fuse-cache/check-<hash>.tsv`) is
  maintained per entry point; a workspace check that hits a valid cache prints
  `check: ok (cached, no changes)` and exits immediately; the cache is invalidated after any
  diagnostic error; these caches are not automatically swept outside command-specific invalidation,
  so `fuse clean --cache` is the supported manual prune path.

Module capabilities:

- modules may declare capability requirements with top-level `requires` declarations
- allowed capabilities are `db`, `crypto`, `network`, and `time`
- duplicate capability declarations in one module are semantic errors
- capability checks are compile-time only (no runtime capability guard)
- calls requiring capabilities are rejected when the current module does not declare them
- `requires db` gates `db.exec/query/one/from` and query-builder calls reachable from `db.from(...)`
  (`select`, `where`, `order_by`, `limit`, `insert`, `upsert`, `update`, `delete`, `count`, `one`, `all`, `exec`, `sql`, `params`)
- typed query forms (`one<T>()`, `all<T>()`) are compile-time checked:
  the type argument must be a declared `type`, and `select([...])` columns must match its fields
- `requires network` gates `serve(...)` and outbound `http.*` client builtins
  (`http.request`, `http.get`, `http.post`)
- `requires time` gates access to runtime `time.*` builtins (`now`, `format`, `parse`, `sleep`)
- `requires crypto` gates access to runtime `crypto.*` builtins (`hash`, `hmac`, `random_bytes`, `constant_time_eq`)
- call sites to imported module functions must declare every capability required by the callee module
  (capability leakage across module boundaries is rejected)
- `transaction` blocks are valid only in modules with `requires db` and no additional capabilities

Strict architecture mode (`--strict-architecture`) adds compile-time architectural checks:

- capability purity: modules must not declare unused capabilities
- cross-layer cycle detection: import graphs that form cycles between logical layers are rejected
- error-domain isolation: a module's function/service boundary signatures must not mix error
  domains from multiple modules

---

## Services and HTTP Contracts

Route syntax uses typed path params inside the route string, for example:

```fuse
get "/users/{id: Id}" -> User:
  ...
```

The `body` keyword introduces the request body type:

```fuse
post "/users" body UserCreate -> User:
  ...
```

Binding/encoding/error semantics for routes are runtime behavior and are defined in `runtime.md`.

HTTP-specific route primitives (`request.header/cookie`,
`response.header/cookie/delete_cookie`, and outbound `http.request/get/post`) are runtime
semantics owned by `runtime.md`.

---

## Static Restrictions

### Spawn static restrictions

Inside a `spawn` block, semantic analysis rejects:

- `box` capture/use (including captured outer boxed bindings)
- runtime side-effect builtins (`db.*`, `serve`, `print`, `input`, `log`, `env`, `env_int`, `env_float`, `env_bool`, `asset`, `svg.inline`)
- mutation of captured outer bindings

Structured task lifetime checks are also enforced at compile time:

- detached task expressions are rejected
- spawned task bindings must be awaited before leaving lexical scope
- reassigning a spawned task binding before `await` is rejected

These restrictions are part of the language contract for deterministic cross-backend concurrency.

### Transaction static restrictions

`transaction:` defines a compiler-constrained block for deterministic DB transaction scope.

Inside a `transaction` block, semantic analysis rejects:

- `spawn` expressions
- `await` expressions
- early `return`
- loop control flow (`break` / `continue`)
- capability use outside `db`

Module-level guardrails for `transaction` blocks:

- the containing module must declare `requires db`
- the containing module must not declare non-`db` capabilities

---

## Runtime Behavior

### Expression operator behavior

Comparison behavior is shared across AST/native backends:

- `==` / `!=` support same-typed pairs for `Int`, `Float`, `Bool`, `String`, and `Bytes`.
- `<`, `<=`, `>`, `>=` support numeric pairs (`Int`, `Float`) only.
- unsupported comparison operand pairs produce runtime errors.

### Validation and boundary enforcement

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

### JSON behavior

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

### Errors and HTTP status mapping

The runtime recognizes a small set of error struct names for standardized HTTP status mapping
and error JSON formatting.

Canonical names (from `std.Error`):

- `std.Error.Validation`
- `std.Error`
- `std.Error.BadRequest`
- `std.Error.Unauthorized`
- `std.Error.Forbidden`
- `std.Error.NotFound`
- `std.Error.Conflict`

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

- `std.Error.Validation` uses `message` and `fields`
  (list of structs with `path`, `code`, `message`).
- `std.Error` uses `code` and `message`. Other fields are ignored for JSON output.
- `std.Error.BadRequest`, `std.Error.Unauthorized`,
  `std.Error.Forbidden`, `std.Error.NotFound`,
  `std.Error.Conflict` use their `message` field if present, otherwise a default message.
- Any other error value renders as `internal_error`.

Status mapping uses the error name first, then `std.Error.status` if present:

- `std.Error.Validation` -> 400
- `std.Error.BadRequest` -> 400
- `std.Error.Unauthorized` -> 401
- `std.Error.Forbidden` -> 403
- `std.Error.NotFound` -> 404
- `std.Error.Conflict` -> 409
- `std.Error` with `status: Int` -> that status
- anything else -> 500

`expr ?! err` behavior:

- `T!E` is `Result<T, E>`.
- `T!` is a compile-time error (explicit error domains are required).
- for function/service return boundaries, each error domain must be a declared nominal `type` or `enum`

`expr ?! err` rules:

- If `expr` is `Option<T>` and is `None`, return `Err(err)`.
- If `expr` is `Result<T, E>` and is `Err`, replace the error with `err`.
- If `expr ?!` omits `err`, `Result` propagates the existing error.
- `Option<T> ?!` without an explicit `err` is a compile-time error.

### Config and CLI binding

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
- runtime prints a hint when it detects a likely env-name typo
  (for example `APP_DBURL` vs expected `APP_DB_URL`)

Type support levels for config values (env and file values):

- **Full**: scalars (`Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`) and `Option<T>`.
- **Structured via JSON text**: `List<T>`, `Map<String,V>`, user-defined `struct`, user-defined `enum`.
- **Rejected**: `Html`, `Map<K,V>` where `K != String`, `Result<T,E>`.

Compatibility notes:

- `Bytes` must be valid base64 text; invalid base64 is a validation error.
- for structured values, parse failures (invalid JSON/type mismatch/unknown field) surface as
  validation errors on the target field path.

CLI binding:

CLI binding is enabled when program args are passed after the file (or after `--`):

```bash
fuse run file.fuse -- --name=Codex
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
- when AOT output is requested, `fuse build` emits deterministic progress stages:
  `[build] aot [n/6] ...`
- `--diagnostics json` switches diagnostics on stderr to JSON Lines:
  - diagnostic entries:
    `{"kind":"diagnostic","level":"error|warning","message":"...","path":"...","line":N,"column":N,"span_start":N,"span_end":N}`
  - command-step entries:
    `{"kind":"command_step","command":"check|run|build|test","message":"start|ok|failed|validation failed|..."}`
- keeps JSON validation payloads uncolored/machine-readable
- `run` CLI argument validation failures exit with code `2`

---

## HTTP Runtime

### Routing

- paths are split on `/` and matched segment-by-segment
- route params use `{name: Type}` and must occupy the whole segment
- params parse with env-like scalar/optional/refined rules
- `body` introduces a JSON request body bound to `body` in the handler

### Response

- successful values encode as JSON with `Content-Type: application/json` by default
- if route return type is `Html` (or `Result<Html, E>` on success), response is rendered once with
  `Content-Type: text/html; charset=utf-8`
- route handlers may append response headers via `response.header(name, value)`
- route handlers may manage cookies via `response.cookie(name, value)` and
  `response.delete_cookie(name)` (emitted as `Set-Cookie` headers)
- `Result` errors are mapped using the status rules above
- unsupported HTTP methods return `405` with `internal_error` JSON
- no HTMX-specific runtime mode: HTMX-style flows are ordinary `Html` route returns

### Request primitives

- route handlers may read inbound headers with `request.header(name)` (case-insensitive)
- route handlers may read cookie values with `request.cookie(name)`
- `request.*` and `response.*` primitives are only valid while evaluating an HTTP route handler

### Observability baseline

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

---

## Builtins

- `print(value)` prints stringified value to stdout
- `input(prompt: String = "") -> String` prints optional prompt and reads one line from stdin
- `log(...)` writes log lines to stderr (see Logging)
- `db.exec/query/one` execute SQL against configured DB
- `db.from(table)` builds parameterized queries
- `transaction:` opens a constrained DB transaction scope (`BEGIN`/`COMMIT`/`ROLLBACK`)
- `assert(cond, message?)` throws runtime error when `cond` is false
- `env(name: String) -> String?` returns env var or `null`
- `env_int(name: String) -> Int?` returns parsed env var as `Int` or `null`
- `env_float(name: String) -> Float?` returns parsed env var as `Float` or `null`
- `env_bool(name: String) -> Bool?` returns parsed env var as `Bool` or `null`
- `asset(path: String) -> String` resolves to hashed/static public URL when asset map is configured
- `serve(port)` starts HTTP server on `FUSE_HOST:port`
- `request.header(name: String) -> String?` reads inbound HTTP headers
- `request.cookie(name: String) -> String?` reads inbound HTTP cookie values
- `response.header(name: String, value: String)` appends response headers
- `response.cookie(name: String, value: String)` appends HTTP-only session cookies
- `response.delete_cookie(name: String)` emits cookie expiration headers
- `http.request(method: String, url: String, body?: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error`
- `http.get(url: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error`
- `http.post(url: String, body: String, headers?: Map<String, String>, timeout_ms?: Int) -> http.response!http.error`
- HTML tag builtins (`html`, `head`, `body`, `div`, `meta`, `button`, ...)
- `html.text`, `html.raw`, `html.node`, `html.render`
- `svg.inline(path: String) -> Html`
- `json.encode(value) -> String` serializes a value to a JSON string
- `json.decode(text: String) -> Value` parses a JSON string into a runtime value
- `time.now() -> Int` returns Unix epoch milliseconds
- `time.sleep(ms: Int)` blocks the current execution for `ms` milliseconds
- `time.format(epoch: Int, fmt: String) -> String` formats epoch milliseconds (UTC)
- `time.parse(text: String, fmt: String) -> Int!Error` parses text to epoch milliseconds
- `crypto.hash(algo: String, data: Bytes) -> Bytes` supports `sha256` / `sha512`
- `crypto.hmac(algo: String, key: Bytes, data: Bytes) -> Bytes` supports `sha256` / `sha512`
- `crypto.random_bytes(n: Int) -> Bytes` returns cryptographically secure random bytes
- `crypto.constant_time_eq(a: Bytes, b: Bytes) -> Bool` compares bytes in constant-time form

`input` behavior notes:

- prompt text is written without a trailing newline
- trailing `\n`/`\r\n` is trimmed from the returned line
- in non-interactive mode with no stdin data, runtime fails with:
  `input requires stdin data in non-interactive mode`
- `input()` / `input("...")` resolve to the CLI input builtin; HTML input tags remain available
  through tag-form calls such as `input(type="text")`

Typed env parsing notes:

- `env_int` / `env_float` / `env_bool` return `null` when the variable is unset.
- when the variable is set but parsing fails, runtime raises a fatal error.

HTTP client notes:

- outbound client calls are blocking runtime operations
- supported outbound schemes are `http://` and validated `https://`; any other well-formed scheme
  returns `Err(http.error)` with `code = "unsupported_scheme"`
- `2xx` responses return `Ok(http.response)`; non-`2xx` responses return `Err(http.error)` with
  `code = "http_status"`
- HTTPS handshake and certificate-validation failures return `Err(http.error)` with
  `code = "tls_error"`
- `timeout_ms` defaults to `30000`; `0` disables the timeout; negative values are invalid requests
- timeout failures use `code = "timeout"` and include the failing phase in the message when the
  runtime can distinguish it (`connect`, `tls handshake`, `write`, or `read`)
- request/response bodies are `String`
- request headers are sent after lowercase normalization; `host`, `connection`, and
  `content-length` are reserved and rejected if supplied by user code
- malformed URLs use `code = "invalid_url"`; other request-shaping failures such as invalid
  timeout/header/method data use `code = "invalid_request"`
- DNS/connect failures use `code = "network_error"`; malformed HTTP response bytes use
  `code = "invalid_response"`
- redirects remain manual in `0.9.x`; `3xx` responses surface through the same `http_status`
  error contract as other non-`2xx` responses
- `http.response` fields: `method: String`, `url: String`, `status: Int`,
  `headers: Map<String, String>`, `body: String`
- `http.error` fields: `code: String`, `message: String`, `method: String`, `url: String`,
  `status: Int?`, `headers: Map<String, String>`, `body: String?`
- response/error header maps expose lowercase header names
- when structured request logs or stderr metrics hooks are enabled, outbound requests emit
  `http.client.request` events carrying runtime, method, URL, outcome, status, response bytes,
  and error-code fields where available

Imported asset runtime values:

- `import Docs from "./README.md"` evaluates to an immutable `String` containing the exact UTF-8
  file contents
- `import SeedData from "./seed.json"` evaluates to an immutable runtime value equivalent to
  `json.decode(text)`
- asset loading and JSON parsing happen during load/check; successful execution does not perform an
  additional runtime file read for imported assets
- missing files, unreadable files, invalid UTF-8, and invalid JSON fail during load/check
- relative, `root:`, and `dep:` resolution semantics match ordinary import resolution
- asset imports are distinct from `asset(path)` public-URL lookup and `svg.inline(path)` HTML loading

Compile-time sugar affecting HTML builtins:

- HTML block syntax (`div(): ...`) lowers to normal calls with explicit attrs + `List<Html>` children
- bare string literals in HTML blocks lower to `html.text(...)`
- `if`/`for` child statements in HTML blocks lower to internal list-producing control expressions
- attribute shorthand (`div(class=expr)`) lowers to attrs maps
- comma-separated HTML attrs and map-literal HTML attrs are compile-time parser errors
  (`FUSE_HTML_ATTR_COMMA`, `FUSE_HTML_ATTR_MAP`); runtime semantics are unchanged

---

## Database (SQLite)

Database access is intentionally minimal and currently uses SQLite via a pooled set of
connections.

Configuration sources:

- `FUSE_DB_URL`
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
- `Query.insert(structValue)` builds `insert into ...` from struct fields
- `Query.upsert(structValue)` builds `insert or replace into ...` from struct fields
- `Query.update(column, value)` builds/extends `set` clauses
- `Query.delete()` builds `delete from ...`
- `Query.count()` executes a `count(*)` query and returns `Int`
- `Query.one()` returns first row `Map<String, Value>?`
- `Query.all()` returns `List<Map<String, Value>>`
- `Query.one<T>()` returns `T?` using boundary-style struct decode/validation for each row
- `Query.all<T>()` returns `List<T>` using boundary-style struct decode/validation for each row
- `Query.exec()`
- `Query.sql()` and `Query.params()` for inspection/debugging

Typed query constraints:

- typed query forms are valid only on `one<T>()` and `all<T>()`
- the type argument must be a declared `type`
- typed query forms require `select([...])` with string-literal columns before `one<T>()`/`all<T>()`
- selected column names must match the target type field names at compile time
- qualified column names are matched by their final segment (`users.id` -> `id`) during typed
  field validation
- typed-query compiler diagnostics use codes `FUSE_TYPED_QUERY_CALL`,
  `FUSE_TYPED_QUERY_TYPE_ARG`, `FUSE_TYPED_QUERY_SELECT`, and
  `FUSE_TYPED_QUERY_FIELD_MISMATCH` in JSON diagnostics output

Parameter binding:

- SQL uses positional `?` placeholders with `List` params
- supported param types: `null`, `Int`, `Float`, `Bool`, `String`, `Bytes`
  (boxed/results are unwrapped)
- `in` expects non-empty list and expands to `IN (?, ?, ...)`
- runtime DB failures include SQL text and a parameter summary

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
fuse migrate path/to/file.fuse
```

Rules:

- migrations are collected from all loaded modules
- run order is ascending by migration name
- applied migrations are tracked in `__fuse_migrations(package, name)` with a composite primary key
- migration package namespace is sourced from `[package].name` in the nearest `fuse.toml`
  (defaults to empty string when absent)
- legacy single-column history tables (`id` primary key) are upgraded in-place to `(package, name)`
  without re-running already-applied single-package migrations
- only up migrations exist today (no down/rollback)
- migrations execute via AST interpreter

### Tests

`test "name":` declares a test block.

Run tests with:

```bash
fuse test path/to/file.fuse
fuse test --filter smoke path/to/file.fuse
```

Rules:

- tests are collected from all loaded modules
- run order is ascending by test name
- `--filter <pattern>` runs only tests whose names contain the pattern (case-sensitive substring match)
- tests execute via AST interpreter
- failures report non-zero exit

Project check incremental mode:

- `fuse check` writes `.fuse/build/check.meta` (or `.fuse/build/check.strict.meta` with `--strict-architecture`)
- warm checks reuse module content hashes from this metadata and skip unchanged-module rechecks

---

## Concurrency

`spawn:` creates a task and returns `Task<T>` where `T` is block result.
Spawned tasks run on a shared worker pool. Execution is asynchronous relative to the caller
and may overlap with other spawned tasks.

`await expr` waits on a task and yields its result.

Structured concurrency is enforced at compile time:

- detached task expressions are invalid
- spawned task bindings must be awaited before scope exit
- spawned task bindings cannot be reassigned before `await`
- `transaction:` blocks reject `spawn` and `await`

`Task<T>` is an opaque runtime type; task values are consumed via `await` only.

Spawn determinism restrictions are enforced at compile time by semantic analysis.
See [Spawn static restrictions](fls.md#spawn-static-restrictions) for the full list.

`box expr` creates a shared mutable cell. Boxed values are transparently dereferenced in most
expressions; assigning boxed bindings updates shared cell state. `spawn` blocks cannot capture or
use boxed state.

---

## Loops, Indexing, and Ranges

### Loops

- `for` iterates over `List<T>` (yields each element) and `Map<K, V>` (yields values only; keys are not available in `for` loop bodies)
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

---

## Logging

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

---

## Tooling and Package Commands

Common package commands:

- `fuse check` — parse and semantic-check a package
- `fuse run` — run a package
- `fuse dev` — run in watch/dev mode with live reload
- `fuse test` — run test blocks
- `fuse build` — compile to a native binary
- `fuse clean --cache` — remove `.fuse-cache` directories under a selected root
- `fuse fmt` — format a source file
- `fuse openapi` — emit an OpenAPI JSON document
- `fuse migrate` — execute pending migration blocks
- `fuse lsp` — start the language server

Useful flags:

- `fuse build --clean` — remove `.fuse/build` before building
- `--workspace` — check all packages under the current directory
- `--strict-architecture` — enable architectural purity checks
- `--diagnostics json` — emit diagnostics as JSON Lines on stderr

`fuse.toml` manifest sections:

| Section | Purpose |
|---|---|
| `[package]` | Entry source file (`entry`), app/service name (`app`), runtime backend (`backend`) |
| `[build]` | Build outputs: `native_bin` binary path, `openapi` JSON output path |
| `[serve]` | Server defaults: `static_dir` for static file serving |
| `[assets]` | Named asset entries (CSS, JS) and `watch` flag |
| `[assets.hooks]` | Build hooks for asset processing |
| `[vite]` | Vite dev server integration settings |
| `[dependencies]` | Package dependencies for `dep:` import paths |

---

## Runtime Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FUSE_DB_URL` | — | Database connection URL (`sqlite://path`) |
| `FUSE_DB_POOL_SIZE` | `1` | SQLite connection pool size |
| `FUSE_CONFIG` | `config.toml` | Config file path |
| `FUSE_HOST` | `127.0.0.1` | HTTP server bind host |
| `FUSE_SERVICE` | — | Selects service when multiple are declared |
| `FUSE_MAX_REQUESTS` | — | Stop server after N requests (useful for tests) |
| `FUSE_LOG` | `info` | Minimum log level (`trace`, `debug`, `info`, `warn`, `error`) |
| `FUSE_COLOR` | `auto` | ANSI color mode (`auto`, `always`, `never`) |
| `NO_COLOR` | — | Disables ANSI color when set (any value) |
| `FUSE_REQUEST_LOG` | — | Set to `structured` (or `1`/`true`) for JSON request logging on stderr |
| `FUSE_METRICS_HOOK` | — | Set to `stderr` for per-request metrics lines |
| `FUSE_DEV_RELOAD_WS_URL` | — | Enables dev HTML script injection (`/__reload` client) with reload + compile-error overlay events |
| `FUSE_OPENAPI_JSON_PATH` | — | Enables built-in OpenAPI JSON endpoint at this path |
| `FUSE_OPENAPI_UI_PATH` | — | Enables built-in OpenAPI UI at this path |
| `FUSE_ASSET_MAP` | — | Logical-path to public-URL mappings for `asset(path)` |
| `FUSE_VITE_PROXY_URL` | — | Fallback proxy for unknown routes to Vite dev server |
| `FUSE_SVG_DIR` | — | Override SVG base directory for `svg.inline` |
| `FUSE_STATIC_DIR` | — | Serve static files from this directory |
| `FUSE_STATIC_INDEX` | `index.html` | Fallback file for directory requests when `FUSE_STATIC_DIR` is set |

### AOT binary environment variables

The following variables are only effective in compiled AOT binaries (`fuse build --release`):

| Variable | Description |
|---|---|
| `FUSE_AOT_BUILD_INFO` | Print AOT build metadata and exit |
| `FUSE_AOT_STARTUP_TRACE` | Emit a startup diagnostic line to stderr |
| `FUSE_AOT_REQUEST_LOG_DEFAULT` | Default to structured request logging when `FUSE_REQUEST_LOG` is unset |

---

## Constraints

Current practical constraints:

- SQLite-focused database runtime
- no full ORM layer
- task model is still evolving
