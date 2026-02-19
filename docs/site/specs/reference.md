# FUSE Developer Reference

This document is the reference for building applications with FUSE.
If you are new to FUSE, start with [Onboarding Guide](onboarding.md) and [Boundary Contracts](boundary-contracts.md) before this reference.

---

## Install and Pre-Alpha Downloads

The docs Docker build generates prebuilt runnables and packages them into this docs app.
You can download the ZIP directly from the running docs site:

1. Download: [`/downloads/fuse-pre-alpha-linux-x64.zip`](/downloads/fuse-pre-alpha-linux-x64.zip)
2. Extract it and add the directory containing `fuse` and `fuse-lsp` to your `PATH`.
3. Verify:

```bash
fuse
```

Direct binary links are also available:

- [`/runnables/linux-x64/fuse`](/runnables/linux-x64/fuse)
- [`/runnables/linux-x64/fuse-lsp`](/runnables/linux-x64/fuse-lsp)

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

Built-in scalar/base types:

- `Int`, `Float`, `Bool`, `String`
- `Id`, `Email`, `Bytes`, `Html`, `Error`

Built-in generic types:

- `List<T>`, `Map<K,V>`, `Option<T>`, `Result<T,E>`, `Task<T>`

Type shorthand:

- `T?` -> `Option<T>`
- `T!` -> `Result<T, Error>`
- `T!E` -> `Result<T, E>`

Refinements:

- `String(1..80)`
- `Int(0..130)`
- `Float(0.0..1.0)`

Type derivation:

```fuse
type PublicUser = User without password, secret
```

---

## Imports and Modules

Supported forms:

```fuse
import Foo
import Utils from "./utils"
import Shared from "root:lib/shared"
import {A, B} from "./lib"
import Auth from "dep:Auth/lib"
```

Notes:

- module imports are qualified (`Foo.value`, `Foo.Type`)
- named imports bring symbols into local scope
- type references can be module-qualified (`Foo.User`)
- `root:` imports resolve from package root (`fuse.toml` directory)

---

## Services and HTTP Contracts

Service routes are typed at declaration time:

```fuse
service Api at "/api":
  get "/users/{id: Id}" -> User:
    return load_user(id)

  post "/users" body UserCreate -> User:
    return create_user(body)
```

Rules:

- path params are typed inside route path placeholders
- `body` binds typed JSON payload
- return type defines response contract

---

## Runtime Behavior

### Validation and boundary enforcement

Validation is applied at runtime in:

- struct construction
- HTTP body decode
- config loading
- CLI binding
- route parameter parsing

Default/null behavior:

- missing field with default -> default applied
- missing optional field -> `null`
- explicit `null` remains `null`

### JSON behavior

- structs encode/decode as JSON objects
- unknown fields are rejected on decode
- optional fields accept missing or `null`
- enums use tagged object shape (`type` + optional `data`)

Type notes:

- `Bytes` is base64 text at JSON/config/CLI boundaries
- `Html` responses are rendered as text
- runtime `Map<K,V>` requires `K = String`
- `Result<T,E>` uses tagged JSON decode (`{"type":"Ok"|"Err","data":...}`)

### Errors and HTTP status mapping

Recognized runtime error names:

- `std.Error.Validation`
- `std.Error`
- `std.Error.BadRequest`
- `std.Error.Unauthorized`
- `std.Error.Forbidden`
- `std.Error.NotFound`
- `std.Error.Conflict`

Status mapping:

- validation/bad request -> `400`
- unauthorized -> `401`
- forbidden -> `403`
- not found -> `404`
- conflict -> `409`
- unknown error types -> `500`

`expr ?! err` behavior:

- `Option<T>` `None` -> `Err(err)`
- `Result<T,E>` `Err` -> mapped to `err`
- `expr ?!` without explicit error uses default/propagated behavior

### Config and CLI binding

Config resolution order:

1. environment variables
2. config file (`config.toml` or `FUSE_CONFIG`)
3. default expressions

CLI binding:

- supports `--flag`, `--no-flag`, `--flag=value`, `--flag value`
- unknown or repeated flags are validation errors
- when args are present, `fn main` is invoked directly and `app` block is skipped

---

## Builtins

General builtins:

- `print`, `log`, `assert`, `env`, `serve`
- `asset`, `svg.inline`
- html constructors/helpers (`html`, `div`, `html.text`, `html.raw`, `html.node`, `html.render`)
- task helpers (`task.id`, `task.done`, `task.cancel`)

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

`docs/Dockerfile` builds `fuse` and `fuse-lsp`, then packages:

- `/downloads/fuse-pre-alpha-linux-x64.zip`
- `/runnables/linux-x64/fuse`
- `/runnables/linux-x64/fuse-lsp`

Build the docs image from repository root:

```bash
docker build -f docs/Dockerfile -t fuse-docs:pre-alpha .
```

Run the docs container:

```bash
docker run --rm -p 4080:4080 -e PORT=4080 -e FUSE_HOST=0.0.0.0 fuse-docs:pre-alpha
```

Then open <http://localhost:4080>.

You can also use Compose:

```bash
docker compose -f docs/docker-compose.yml up --build
```

---

## Runtime Environment Variables

Common runtime knobs:

- `FUSE_HOST`
- `FUSE_SERVICE`
- `FUSE_MAX_REQUESTS`
- `FUSE_CONFIG`
- `FUSE_OPENAPI_JSON_PATH`, `FUSE_OPENAPI_UI_PATH`
- `FUSE_ASSET_MAP`
- `FUSE_VITE_PROXY_URL`
- `FUSE_SVG_DIR`
- `FUSE_LOG`

---

## Constraints

Current practical constraints:

- SQLite-focused database runtime
- no full ORM layer
- task model is still evolving
- native backend path targets VM-compatible semantics
