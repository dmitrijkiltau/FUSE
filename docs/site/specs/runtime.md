# FUSE Runtime Guide

This page documents runtime behavior that matters when shipping FUSE applications.

---

## Boundary behavior

FUSE applies type contracts at boundaries:

- struct construction
- HTTP body and route-parameter binding
- config loading (`env` > config file > defaults)
- CLI flag binding to `fn main`

Supported boundary shapes include:

- scalars (`Int`, `Float`, `Bool`, `String`, `Id`, `Email`, `Bytes`)
- `Option<T>`
- structured values via JSON text for `List<T>`, `Map<String,V>`, structs, enums

`Bytes` use base64 text at JSON/config/CLI boundaries.

See also: [Syntax and Types](fls.md), [Language Guide](fuse.md).

---

## Error handling and status mapping

FUSE supports explicit result types:

- `T!` (`Result<T, Error>`)
- `T!E` (`Result<T, E>`)
- `expr ?! err` to convert option/result failures to typed errors

For HTTP services, `std.Error.*` names map to status codes and standardized JSON error payloads.

Common mappings:

- `std.Error.Validation` -> `400`
- `std.Error.NotFound` -> `404`
- `std.Error.Conflict` -> `409`
- unknown error type -> `500`

See also: [Language Guide](fuse.md), [Services and routes](fls.md#services-and-routes).

---

## HTTP behavior

Service runtime behavior:

- typed path params from route patterns (for example `{id: Id}`)
- `body` keyword binds typed JSON request body
- JSON responses by default (`application/json`)
- `Html` success responses rendered as `text/html; charset=utf-8`

Useful environment variables:

- `FUSE_HOST`
- `FUSE_SERVICE`
- `FUSE_MAX_REQUESTS`
- `FUSE_OPENAPI_JSON_PATH`, `FUSE_OPENAPI_UI_PATH`

See also: [Language Guide](fuse.md), [OpenAPI page](/openapi).

---

## Builtins and data access

Common builtins:

- `print`, `log`, `assert`, `env`
- `serve`
- `asset` and `svg.inline`
- HTML builders (`div`, `html.text`, `html.render`, ...)
- DB access (`db.exec`, `db.query`, `db.one`, `db.from`)

Database notes:

- SQLite only in current implementation
- positional parameter binding via `?`
- query builder supports `where`, `order_by`, `limit`, `one`, `all`

Migrations and tests:

- `migration` blocks via `fusec --migrate`
- `test` blocks via `fusec --test`

See also: [Scope and roadmap](scope.md), [Syntax and Types](fls.md).

---

## Concurrency, loops, and logging

Runtime control-flow features:

- `spawn`/`await` task model
- `box` shared mutable cell
- `for` and `while`
- list/map indexing and assignment
- inclusive ranges (`a..b`)

Logging:

- `log("message")`
- `log("warn", "message")`
- structured logs when extra data arguments are present

See also: [Syntax and Types](fls.md#expressions-and-control-flow), [Scope and roadmap](scope.md).
