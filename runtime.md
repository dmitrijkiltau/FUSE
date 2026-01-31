# Runtime semantics and contracts (MVP)

This document specifies the core runtime behavior that the compiler and standard library must implement. It is the contract that keeps "write intent, get software" consistent across CLI and HTTP.

## Goals

- Validate at boundaries by default.
- Deterministic error shapes and mappings.
- Zero config glue: config, CLI, HTTP, and JSON are first-class.
- Small runtime surface, but strict, predictable behavior.

## Error model

### Built-in error types

All errors are nominal types. The runtime ships these built-ins:

```
type Error:
  code: String
  message: String
  details: Map<String, String> = {}
  status: Int? = null

type ValidationError:
  message: String = "validation failed"
  fields: List<ValidationField> = []

type ValidationField:
  path: String
  code: String
  message: String

type NotFound:
  message: String = "not found"

type Unauthorized:
  message: String = "unauthorized"

type Forbidden:
  message: String = "forbidden"

type Conflict:
  message: String = "conflict"

type BadRequest:
  message: String = "bad request"
```

### Result types

- `T!` is `Result<T, Error>`.
- `T!E` is `Result<T, E>`.

`?!` sugar (from spec) is implemented as:

- If `expr` is `Option<T>` and is `None`, return `Err(ErrValue)`.
- If `expr` is `Result<T, E>` and is `Err`, return `Err(ErrValue)` (or wrap in `Error` for `T!`).

### Error JSON shape

HTTP error responses are JSON with a single `error` object:

```
{
  "error": {
    "code": "validation_error",
    "message": "validation failed",
    "fields": [
      { "path": "email", "code": "invalid_email", "message": "invalid email" }
    ]
  }
}
```

Rules:

- For `ValidationError`, `fields` is populated.
- For other errors, `fields` is omitted or empty.
- `Error.code` is a stable machine string (lower snake case).

### HTTP status mapping

Mapping uses type name first, then `Error.status` if present:

- `ValidationError` -> 400
- `BadRequest` -> 400
- `Unauthorized` -> 401
- `Forbidden` -> 403
- `NotFound` -> 404
- `Conflict` -> 409
- `Error` with `status` -> that status
- anything else -> 500

## Validation model

### When validation happens

Validation is performed at all decode boundaries:

- JSON decode
- CLI argument parsing
- HTTP request binding
- Config loading
- `refine<T>` calls
- `Type(...)` construction

Validation inside the runtime is optional and off by default (dev mode can enable it).

### Default values

Defaults are applied before validation. If a field is optional and a default is present:

- Missing field -> default applied.
- Explicit `null` -> stays `None`.

### Validation errors

`ValidationError.fields` use dot paths and indexes:

- `user.email`
- `items[0].id`

## JSON codec derivation

### Struct encoding

- Structs encode to JSON objects with field names as declared.
- Encoding includes all fields (including defaults).
- Optional `None` encodes as `null`.

### Struct decoding

- Missing field with default -> default value.
- Missing field with no default -> error.
- Optional fields accept both missing and `null` as `None`.
- Unknown fields -> error by default.

### Enum encoding

Enums use a tagged object format:

```
{"type":"Variant","data":...}
```

Rules:

- No payload: omit `data`.
- Single payload: `data` is the value.
- Multiple payloads: `data` is an array.

### Built-in types

- `Id`, `Email` -> JSON string.
- `Bytes` -> base64 string.
- `Map<K,V>` -> JSON object if `K` is `String`, otherwise JSON array of pairs.

## Config binding

Each `config` block compiles to a struct plus a loader function that reads configuration from:

1. A config file (`config.toml` by default, overridable via `FUSE_CONFIG` or `--config`).
2. Environment variables (override config file).
3. Field defaults (from the config block expressions).

Config file format is TOML with a section per config:

```
[App]
port = 3000
dbUrl = "sqlite://app.db"
```

Env override naming:

- `APP_PORT`, `APP_DB_URL` for config `App`.

Field expressions may still call `env()` explicitly; explicit `env()` is evaluated only if the field was not overridden by file or env.

Loaded config values are validated after resolution and cached for the process lifetime.

## CLI binding

The `app` block exposes a CLI from `main`:

- Each parameter becomes a flag: `--name`.
- Required parameters (no default, not optional) are required flags.
- Optional parameters (`T?`) are optional flags returning `None` when missing.
- Parameters with defaults are optional flags using the default.

Type parsing:

- `Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`.
- `List<T>` uses repeated flags: `--tag a --tag b`.
- `Bool` supports `--flag` and `--no-flag`.

Errors:

- Validation error -> exit code 2 with error JSON on stderr.
- Any other error -> exit code 1.

## HTTP runtime contract

Each `service` block generates a router and a server entry:

- Path params are extracted and validated from route patterns.
- `body` is decoded and validated as JSON.
- Return values are encoded as JSON with `content-type: application/json`.
- Interpreter MVP environment knobs:
  - `FUSE_HOST` (default `127.0.0.1`) controls bind host.
  - `FUSE_SERVICE` selects the service when multiple are declared.
  - `FUSE_MAX_REQUESTS` stops the server after N requests (useful for tests).

Error handling:

- `Result` errors map using the rules above.
- Unexpected panics map to 500 with code `internal_error`.

## Logging

The `log` module exposes:

```
log.debug "msg", key=value
log.info  "msg", key=value
log.warn  "msg", key=value
log.error "msg", key=value
```

Defaults:

- JSON line output.
- `debug/info` -> stdout, `warn/error` -> stderr.
- Level from `FUSE_LOG` (default `info`).
- Each record includes `ts`, `level`, `msg`, `fields`, `module`.

## Concurrency model

- `spawn` creates a task and returns `Task<T>`.
- `await task` waits and yields `T` (or propagates the task error).
- `await all` waits for all tasks spawned in the current scope.

### Shared state (`box`)

`box` creates a shared mutable cell:

```
var counter = box 0
counter += 1
```

Semantics:

- `box` is the only shared mutable type.
- Mutations are atomic via an internal lock.
- `counter += 1` is sugar for `counter.set(counter.get() + 1)`.

## Stdlib surface (MVP)

Modules and key items:

- `json`: `encode`, `decode`, `decode_relaxed`
- `log`: logging functions (above)
- `env`: `get(name: String) -> String?`
- `time`: `now()`, `sleep(ms: Int)`
- `net/http`: server primitives used by `service` codegen (mostly internal)
- `errors`: built-in error constructors (optional namespace)

`db` is a separate built-in surface but remains minimal for MVP; the compiler only assumes `db.connect` and basic `table` operations from migrations.
