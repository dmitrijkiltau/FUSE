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

Type derivation:

`type PublicUser = User without password, secret` creates a new nominal type derived from `User`
with listed fields removed. Field types/defaults are preserved for retained fields.

Base types can be module-qualified (`Foo.User`). Unknown base types or fields are errors.

---

## Imports and Modules

`import` declarations are resolved at load time.

- Module imports register an alias for qualified access (`Foo.bar`, `Foo.Config.field`, `Foo.Enum.Variant`).
- Named imports bring specific items into local scope.

Resolution rules:

- `import Foo` loads `Foo.fuse` from the current file directory.
- `import X from "path"` loads `path` relative to current file; `.fuse` is added if missing.
- `import {A, B} from "path"` loads module and imports listed names into local scope.
- `import X from "root:path/to/module"` loads from package root (`fuse.toml` directory); if no manifest is found, root falls back to the entry module directory.

Notes:

- module imports do not automatically import all members into local scope
- named imports do not create a module alias
- function symbols are module-scoped (not global across all loaded modules)
- unqualified function calls resolve in this order: current module, then named imports
- module-qualified calls (`Foo.bar`) resolve against the referenced module alias
- duplicate named imports in one module are load-time errors
- duplicate function names across different modules are allowed
- module-qualified type references are valid in type positions (`Foo.User`, `Foo.Config`)
- dependency modules use `dep:` import paths (for example, `dep:Auth/lib`)
- root-qualified modules use `root:` import paths (for example, `root:lib/auth`)

Module capabilities:

- modules may declare capability requirements with top-level `requires` declarations
- allowed capabilities are `db`, `crypto`, `network`, and `time`
- duplicate capability declarations in one module are semantic errors
- capability checks are compile-time only (no runtime capability guard)
- calls requiring capabilities are rejected when the current module does not declare them
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

HTTP-specific route primitives (`request.header/cookie` and
`response.header/cookie/delete_cookie`) are runtime semantics owned by `runtime.md`.

---

## Runtime Behavior

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

Status mapping uses the error name first, then `std.Error.status` if present:

- `std.Error.Validation` / `Validation` -> 400
- `std.Error.BadRequest` / `BadRequest` -> 400
- `std.Error.Unauthorized` / `Unauthorized` -> 401
- `std.Error.Forbidden` / `Forbidden` -> 403
- `std.Error.NotFound` / `NotFound` -> 404
- `std.Error.Conflict` / `Conflict` -> 409
- `std.Error` / `Error` with `status: Int` -> that status
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

Database builtins:

- `db.exec`, `db.query`, `db.one`
- `db.from` + query builder methods

Current DB mode is SQLite-focused.

---

## Tooling and Package Commands

Common package commands:

- `fuse check`
- `fuse run`
- `fuse dev`
- `fuse test`
- `fuse build`

Compiler/runtime CLI operations include:

- `fusec --check`
- `fusec --run`
- `fusec --test`
- `fusec --migrate`
- `fusec --openapi`

`fuse.toml` sections commonly used:

- `[package]`
- `[build]`
- `[serve]`
- `[assets]`, `[assets.hooks]`
- `[vite]`
- `[dependencies]`

---

## Run Docs with Docker

`docs/Dockerfile` builds the `fuse` CLI from source, then runs `fuse build --aot --release` to produce the docs AOT binary.
Guide docs generation is skipped in Docker because generated docs are committed.
Downloadable release artifacts are not served by the docs app; use GitHub Releases instead.

Build the docs image from repository root:

```bash
docker build -f docs/Dockerfile -t fuse-docs:0.6.0 .
```

Run the docs container:

```bash
docker run --rm -p 4080:4080 -e PORT=4080 -e FUSE_HOST=0.0.0.0 fuse-docs:0.6.0
```

Then open <http://localhost:4080>.

You can also use Compose:

```bash
docker compose --project-directory . -f docs/docker-compose.yml up --build
```

---

## Runtime Environment Variables

- `FUSE_HOST` (default `127.0.0.1`) controls bind host
- `FUSE_SERVICE` selects service when multiple are declared
- `FUSE_MAX_REQUESTS` stops server after N requests (useful for tests)
- `FUSE_DEV_RELOAD_WS_URL` enables dev HTML script injection (`/__reload` client)
- `FUSE_OPENAPI_JSON_PATH` + `FUSE_OPENAPI_UI_PATH` enable built-in OpenAPI UI serving
- `FUSE_ASSET_MAP` provides logical-path -> public-URL mappings for `asset(path)`
- `FUSE_VITE_PROXY_URL` enables fallback proxying of unknown routes to Vite dev server
- `FUSE_SVG_DIR` overrides SVG base directory for `svg.inline`

---

## Constraints

Current practical constraints:

- SQLite-focused database runtime
- no full ORM layer
- task model is still evolving
- native backend uses Cranelift JIT
