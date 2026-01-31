# Runtime semantics (current implementation)

This document describes the behavior of the AST interpreter and VM in this repo. It is deliberately
conservative: anything not listed here is either unsupported or not implemented yet.

## Backends

* **AST interpreter**: executes the parsed AST directly.
* **VM**: lowers to bytecode and executes the VM.

Most runtime behavior is shared, with a few differences called out below.

## Error model

### Recognized error names

The runtime recognizes a small set of error struct names when formatting error JSON and mapping
HTTP statuses:

* `ValidationError`
* `Error`
* `BadRequest`
* `Unauthorized`
* `Forbidden`
* `NotFound`
* `Conflict`

These are not built-in types in the language (except `Error`); they are matched by name at runtime.

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

* `ValidationError` uses `message` and `fields` (list of structs with `path`, `code`, `message`).
* `Error` uses `code` and `message`. Other fields are ignored for JSON output.
* `BadRequest`, `Unauthorized`, `Forbidden`, `NotFound`, `Conflict` use their `message` field if
  present, otherwise a default message.
* Any other error value renders as `internal_error`.

### HTTP status mapping

Status mapping uses the error name first, then `Error.status` if present:

* `ValidationError` -> 400
* `BadRequest` -> 400
* `Unauthorized` -> 401
* `Forbidden` -> 403
* `NotFound` -> 404
* `Conflict` -> 409
* `Error` with `status: Int` -> that status
* anything else -> 500

### Result types + `?!`

* `T!` is `Result<T, Error>`.
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
* `Map<K,V>` -> JSON object. Keys are strings; non-string keys are rejected.
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

## CLI binding (AST backend only)

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
* `env(name: String) -> String?` returns an env var or `null`.
* `serve(port)` starts the HTTP server on `FUSE_HOST:port`.

## Unsupported or partial features

* `migration` and `test` are parsed but not executed.
* `for`/`while`/`break`/`continue` are parsed and type-checked but error at runtime.
* `spawn`/`await`/`box` are parsed and type-checked but error at runtime.
* Assignment targets are limited to identifiers.
* `..` range expressions are only used inside refined type arguments.
