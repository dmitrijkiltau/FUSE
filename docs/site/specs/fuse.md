# FUSE Language Guide

FUSE is a small, strict language for CLI apps and HTTP services.
This guide focuses on writing and shipping FUSE code.

---

## Quick start

Create a package with `fuse.toml`:

```toml
[package]
entry = "src/main.fuse"
app = "Api"
backend = "vm"
```

Define a program in `src/main.fuse`:

```fuse
config App:
  port: Int = env("PORT") ?? 3000

type UserCreate:
  email: Email
  name: String(1..80)

service Api at "/api":
  post "/users" body UserCreate -> UserCreate:
    return body

app "Api":
  serve(App.port)
```

Run it with `fuse run`.

See also: [Syntax and Types](fls.md), [Runtime Guide](runtime.md).

---

## Core language model

FUSE keeps a narrow, explicit core:

- indentation-based blocks (spaces only)
- declarations: `fn`, `type`, `enum`, `config`, `service`, `app`
- immutable `let`, mutable `var`
- first-class option/result types (`T?`, `T!`, `T!E`)
- refined primitives (`String(1..80)`, `Int(0..130)`)

Errors are explicit and typed:

```fuse
type std.Error.NotFound:
  message: String

fn get_user(id: Id) -> User!std.Error.NotFound:
  let user = find_user(id) ?! std.Error.NotFound(message="User not found")
  return user
```

See also: [Error handling and status mapping](runtime.md#error-handling-and-status-mapping), [Syntax and Types](fls.md).

---

## Services and boundaries

Services are contract-first: type signatures drive parsing, validation, and response encoding.

- route params are typed (`{id: Id}`)
- `body` binds typed JSON request body
- return `Html` for server-rendered fragments/pages
- return `Result` for typed failures

You define contracts once; the runtime enforces them at HTTP, config, and CLI boundaries.

See also: [Boundary behavior](runtime.md#boundary-behavior), [Services and routes](fls.md#services-and-routes).

---

## Day-to-day workflow

Common commands:

- `fuse check` - semantic checks
- `fuse run` - run selected app
- `fuse dev` - run with watch + live reload
- `fuse test` - run `test` blocks
- `fuse build` - build artifacts (OpenAPI, optional native outputs)

Use `fusec --openapi` or package build settings to generate OpenAPI from service/type declarations.

See also: [Scope and roadmap](scope.md), [Runtime Guide](runtime.md).
