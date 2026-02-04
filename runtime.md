# Runtime semantics (current implementation)

This document describes the behavior of the AST interpreter, VM, and native backend path in this repo. It is deliberately
conservative: anything not listed here is either unsupported or not implemented yet.

## Backends

* **AST interpreter**: executes the parsed AST directly.
* **VM**: lowers to bytecode and executes the VM.
* **Native (stage 1)**: uses a compiled native image (`program.native`) and VM-compatible runtime semantics, with a Cranelift JIT fast-path for direct Int/Bool arithmetic/control-flow function calls.

Most runtime behavior is shared, with a few differences called out below.

## Error model

### Recognized error names

The runtime recognizes a small set of error struct names when formatting error JSON and mapping
HTTP statuses. These live under a reserved namespace:

* `std.Error.Validation`
* `std.Error`
* `std.Error.BadRequest`
* `std.Error.Unauthorized`
* `std.Error.Forbidden`
* `std.Error.NotFound`
* `std.Error.Conflict`

Names outside `std.Error.*` do not participate in HTTP status mapping or error JSON formatting.

### Error JSON shape

Errors are rendered as JSON with a single `error` object:

```
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

* `std.Error.Validation` uses `message` and `fields` (list of structs with `path`, `code`, `message`).
* `std.Error` uses `code` and `message`. Other fields are ignored for JSON output.
* `std.Error.BadRequest`, `std.Error.Unauthorized`, `std.Error.Forbidden`, `std.Error.NotFound`,
  `std.Error.Conflict` use their `message` field if
  present, otherwise a default message.
* Any other error value renders as `internal_error`.

### HTTP status mapping

Status mapping uses the error name first, then `std.Error.status` if present:

* `std.Error.Validation` -> 400
* `std.Error.BadRequest` -> 400
* `std.Error.Unauthorized` -> 401
* `std.Error.Forbidden` -> 403
* `std.Error.NotFound` -> 404
* `std.Error.Conflict` -> 409
* `std.Error` with `status: Int` -> that status
* anything else -> 500

### Result types + `?!`

* `T!` is `Result<T, Error>` (the built-in error base).
* `T!E` is `Result<T, E>`.

`expr ?! err` rules:

* If `expr` is `Option<T>` and is `None`, return `Err(err)`.
* If `expr` is `Result<T, E>` and is `Err`, replace the error with `err`.
* If `expr ?!` omits `err`, `Option` uses a default error, and `Result` propagates the existing error.

## Validation model

Validation is applied at runtime in these places:

* Struct literal construction (`Type(...)`)
* JSON decode for HTTP body
* Config loading
* CLI flag binding
* Route parameter parsing

There is no global "validate on assignment" mode.

### Default values

Defaults are applied before validation:

* Missing field with default -> default is used.
* Missing optional field -> `null`.
* Explicit `null` stays `null` (even if a default exists).

### Built-in refinements

Refinements are range-based only:

* `String(1..80)` length constraint
* `Int(0..130)` numeric range
* `Float(0.0..1.0)` numeric range

Other refinements (regex, custom predicates) are not implemented.

### `Id` and `Email`

* `Id` is a non-empty string.
* `Email` uses a simple `local@domain` check with a `.` in the domain.

## JSON encoding/decoding

### Structs

* Encode to JSON objects with declared field names.
* All fields are included (including defaults).
* `null` represents an optional `None`.

### Struct decoding

* Missing field with default -> default value.
* Missing field with no default -> error.
* Optional fields accept missing or `null`.
* Unknown fields -> error.

### Enums

Enums use a tagged object format:

```
{ "type": "Variant", "data": ... }
```

Rules:

* No payload: omit `data`.
* Single payload: `data` is the value.
* Multiple payloads: `data` is an array.

### Built-in types and generics

* `String`, `Id`, `Email`, `Bytes` -> JSON string.
* `Bool`, `Int`, `Float` -> JSON number/bool.
* `List<T>` -> JSON array.
* `Map<K,V>` -> JSON object. At runtime, `Map<K,V>` requires `K = String`; non-string keys are rejected.
* `Result<T,E>` is **not** supported in JSON decoding.

`Bytes` are treated as plain strings; base64 is not implemented.

## Config loading

Config values are resolved in this order:

1. Environment variables (override config file)
2. Config file (default `config.toml`, overridable via `FUSE_CONFIG`)
3. Default expressions

Config file format is a minimal TOML-like subset:

```
[App]
port = 3000
dbUrl = "sqlite://app.db"
```

Notes:

* Only section headers and `key = value` pairs are supported.
* Values are parsed as strings (with basic `"` escapes) and then converted using the same rules as env vars.

Env override naming is derived from config and field names:

* `App.port` -> `APP_PORT`
* `dbUrl` -> `DB_URL`
* Hyphens become underscores, and camelCase is split into `SNAKE_CASE`.

Type support for config values is the same as env parsing:

* simple scalars (`Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`)
* `Option<T>` where `null`/empty is allowed
* refined ranges on those base types

`List`, `Map`, `Result`, and user-defined types are not supported for config values.

## CLI binding

CLI binding is enabled when you pass program arguments after the file name (or after `--`):

```
fusec --run file.fuse -- --name=Codex
```

Rules:

* Flags only (no positional arguments).
* `--flag value` and `--flag=value` are supported.
* `--flag` sets a `Bool` to `true`, `--no-flag` sets it to `false`.
* Unknown flags are validation errors.
* Only scalar types and `Option<T>` are supported (same as env parsing).
* Multiple values for the same flag are rejected.
* CLI binding calls `fn main` directly (the `app` block is ignored when program args are present).

Validation errors are printed as JSON on stderr and usually exit with code 2.

## HTTP runtime

### Routing

* Paths are split on `/` and matched segment-by-segment.
* Route params use `{name: Type}` and must occupy the whole segment.
* Params are parsed with the same rules as env parsing (simple types + optional/refined).
* `body` introduces a JSON request body and is bound to the name `body` in the handler.

### Response

* Successful values encode as JSON with `Content-Type: application/json`.
* `Result` errors are mapped using the status rules above.
* Unsupported HTTP methods return `405` with `internal_error` JSON.

### Environment knobs

* `FUSE_HOST` (default `127.0.0.1`) controls bind host.
* `FUSE_SERVICE` selects the service when multiple are declared.
* `FUSE_MAX_REQUESTS` stops the server after N requests (useful for tests).

## Builtins (current)

* `print(value)` prints a stringified value to stdout.
* `log(...)` writes a log line to stderr (see Logging below).
* `db.exec/query/one` execute SQL against the configured database (see Database below).
* `assert(cond, message?)` throws a runtime error when `cond` is false.
* `env(name: String) -> String?` returns an env var or `null`.
* `serve(port)` starts the HTTP server on `FUSE_HOST:port`.

## Database (SQLite only)

Database access is intentionally minimal and currently uses SQLite via a single connection.

Configuration:

* `FUSE_DB_URL` (preferred) or `DATABASE_URL`
* `App.dbUrl` if config has been loaded

URL format:

* `sqlite://path` or `sqlite:path`

Builtins:

* `db.exec(sql)` executes a SQL batch (no return value).
* `db.query(sql)` returns `List<Map<String, Value>>` (column names -> values).
* `db.one(sql)` returns the first row as a map, or `null`.

Value mapping:

* `NULL` -> `null`
* integers -> `Int`
* reals -> `Float`
* text -> `String`
* blobs -> hex string

No parameter binding or connection pooling is implemented yet.

## Migrations

`migration <name>:` declares a migration block. Run them with:

```
fusec --migrate path/to/file.fuse
```

Rules:

* Migrations are collected from all loaded modules.
* They run in ascending order by migration name.
* Applied migrations are tracked in `__fuse_migrations`.
* Only “up” migrations exist today (no down/rollback).
* Migrations are executed by the AST interpreter.

## Tests

`test "name":` declares a test block. Run tests with:

```
fusec --test path/to/file.fuse
```

Rules:

* Tests are collected from all loaded modules.
* They run in ascending order by test name.
* Tests are executed by the AST interpreter.
* Failures are reported and the process exits non-zero.

## Concurrency

`spawn:` creates a task and returns `Task<T>` where `T` is the block result. Tasks execute eagerly
today (no parallelism), but errors are captured and surfaced when awaited.

`await expr` waits on a task and yields its result.

Tasks are currently opaque runtime values: there is no exposed task identity, status inspection,
or lifecycle control beyond a minimal task API:

* `task.id(t: Task<T>) -> Id` returns a stable task identity.
* `task.done(t: Task<T>) -> Bool` returns completion state.
* `task.cancel(t: Task<T>) -> Bool` attempts cancellation.

With today's eager execution model, spawned tasks complete immediately, so `task.done` is usually
`true` and `task.cancel` usually returns `false`.

`box expr` creates a shared mutable cell. Boxed values are transparently dereferenced in most
expressions; assigning to a boxed binding updates the shared cell. Passing a box into `spawn`
shares state across tasks.

## Loops

`for` iterates over `List<T>` values and `Map<K, V>` values (iterates the map values).

`break` exits the nearest loop, and `continue` skips to the next iteration.

## Indexing

`list[idx]` reads a list element. `idx` must be an `Int` and within bounds.
Out-of-bounds indexes raise a runtime error (no auto-extend).

`map[key]` reads a map element. Missing keys return `null`.

Assignment targets allow:

* `list[idx] = value` (bounds-checked).
* `map[key] = value` (insert or overwrite).

Optional access in assignment targets (e.g. `foo?.bar = x`, `items?[0] = x`) errors if the base is `null`.

## Ranges

`a..b` evaluates to a `List` of numbers from `a` to `b` (inclusive).

* Only numeric bounds are allowed.
* If `a > b`, the range raises a runtime error.
* Float ranges step by `1.0` (for example, `1.5..3.5` yields `[1.5, 2.5, 3.5]`).

## Logging

`log` is a lightweight builtin for runtime logging. It is intentionally minimal and shared by
all runtime backends.

Usage:

* `log("message")` logs at `INFO`.
* `log("warn", "message")` logs at `WARN`.
* If there are 2+ args and the first is a known level (`trace`, `debug`, `info`, `warn`, `error`),
  it is treated as the level; the rest are stringified and joined with spaces.
* If there is at least one extra argument after the message, `log` emits JSON (see below).

Output format:

* `[LEVEL] message` to stderr.
* JSON logs are emitted as a single line to stderr.

Filtering:

* `FUSE_LOG` sets the minimum level (default `info`).

Structured logging:

* `log("info", "message", data)` emits JSON:
  `{"level":"info","message":"message","data":<json>}`
* If multiple data values are provided, `data` is a JSON array.
