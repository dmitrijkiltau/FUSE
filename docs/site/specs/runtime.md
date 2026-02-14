# Build + Operate

This guide covers how FUSE applications behave at runtime.

---

## 1) Boundary behavior you can rely on

FUSE applies type contracts when data crosses boundaries:

- HTTP body and route params
- config values (`env` > config file > defaults)
- CLI flag binding for `fn main`
- struct construction

Supported shapes include scalars, `Option<T>`, and structured values via JSON text for lists/maps/structs/enums.

---

## 2) Error handling and status mapping

Use typed results:

- `T!` for default error base
- `T!E` for explicit error type
- `expr ?! err` for option/result conversion

In HTTP services, `std.Error.*` names map to status codes and standardized error JSON.

Typical mappings:

- `std.Error.Validation` -> `400`
- `std.Error.NotFound` -> `404`
- `std.Error.Conflict` -> `409`
- unknown error types -> `500`

Need a quick refresh on route/type syntax before wiring errors? Jump to [Language Tour](fls.md#5-service-signatures).

---

## 3) HTTP behavior

At runtime:

- typed path params are parsed from route templates
- `body` binds typed JSON payloads
- success responses default to `application/json`
- `Html` successes render as `text/html; charset=utf-8`

Useful runtime environment knobs:

- `FUSE_HOST`
- `FUSE_SERVICE`
- `FUSE_MAX_REQUESTS`
- `FUSE_OPENAPI_JSON_PATH`, `FUSE_OPENAPI_UI_PATH`

---

## 4) Builtins, DB, and migration flow

Common builtins:

- `print`, `log`, `assert`, `env`
- `serve`, `asset`, `svg.inline`
- HTML helpers (`div`, `html.text`, `html.render`, ...)
- DB helpers (`db.exec`, `db.query`, `db.one`, `db.from`)

Current DB path is SQLite-focused.

Schema lifecycle:

- `migration` blocks run via `fusec --migrate`
- `test` blocks run via `fusec --test`

---

## 5) Concurrency, loops, and logging

Runtime control primitives:

- `spawn` / `await`
- `box` shared mutable cell
- `for`, `while`, indexing, and range expressions (`a..b`)

Logging patterns:

- `log("message")`
- `log("warn", "message")`
- structured output with extra data arguments

If you are deciding whether current capabilities and constraints match your production needs, continue with [Limits + Roadmap](scope.md).
