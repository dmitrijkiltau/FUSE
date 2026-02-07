# **FUSE**

*Write intent. Get software.*

FUSE is a small, strict, "default-sane" language for CLI apps and HTTP services. This document
describes the current implementation in this repo (parser + semantic analysis + AST interpreter + VM).

## The core vibe (today)

### 1) Small, strict, multi-file MVP

Programs can span multiple files via `import`. Module imports are namespaced; named imports bring items into scope, and module-qualified access works for values and types.

### 2) Strong types, low ceremony

Types exist so your code doesn't lie. You shouldn't have to negotiate with the compiler to get work done.

### 3) Boundaries are first-class

Config, JSON, validation, and HTTP routing are built into the runtime so you don't hand-roll glue.

## Syntax: aggressively readable

* Indentation-based blocks (yes, like Python, but strict).
* No semicolons.
* `let` for immutable, `var` for mutable.
* Functions are `fn`.
* Structs are `type`.
* Enums are `enum`.
* String interpolation uses `${expr}` inside double quotes.

### Hello World (app + optional CLI)

```fuse
fn main(name: String = "world"):
  print("Hello, ${name}!")

app "hello":
  main()
```

Run with `fusec --run` to execute the `app`. If you pass CLI flags (for example `--name=Codex`),
`fusec` calls `main` directly and binds flags to its parameters; the `app` block is skipped when
program args are present.

## Data model: types that validate at boundaries

```fuse
type User:
  id: Id
  email: Email
  name: String(1..80)
  age: Int(0..130) = 18
```

What the runtime does today:

* JSON encode/decode for structs and enums.
* Validation for refined types (ranges, Email).
* Default values applied during struct construction, JSON decoding, and config loading.

## Functions: small and explicit

```fuse
fn greet(user: User) -> String:
  "Hi ${user.name}"
```

Expression-last returns implicitly, but you can `return` when you feel dramatic.

## Errors: Result + optional sugar

```fuse
type std.Error.NotFound:
  message: String

fn find_user(id: Id) -> User?:
  return null

fn get_user(id: Id) -> User!std.Error.NotFound:
  let user = find_user(id) ?! std.Error.NotFound(message="User ${id} not found")
  return user
```

* `T?` is optional (`null` represents None).
* `T!` is `Result<T, Error>`.
* `T!E` is `Result<T, E>`.
* `?!` turns an `Option`/`Result` into a typed error inside a fallible function.

## HTTP: you describe endpoints, FUSE handles JSON + validation

```fuse
config App:
  port: Int = env("PORT") ?? 3000

type UserCreate:
  email: Email
  name: String(1..80)

service Users at "/api":
  post "/users" body UserCreate -> UserCreate:
    return body

app "users":
  serve(App.port)
```

The runtime currently handles:

* path + body decoding and validation
* JSON responses
* mapping `Result` errors to HTTP statuses

## What works today (MVP)

* Parser + semantic analysis for `fn`, `type`, `enum`, `config`, `service`, `app`
* AST interpreter, VM, and experimental native backend (`--backend native`)
* `import` module loading (namespaced modules + named imports)
* module-qualified type references in type positions (`Foo.User`, `Foo.Config`)
* Built-ins: `print(...)`, `log(...)`, `db.exec/query/one`, `assert(...)`, `env(...)`, `serve(...)`, `task.id/done/cancel`
* SQLite-backed DB access (`db.exec/query/one`) + migrations (`migration` + `fusec --migrate`)
* tests via `test "name":` + `fusec --test` (AST backend)
* `spawn`/`await`/`box` concurrency
* `for`/`while`/`break`/`continue` loops
* range expressions (`a..b`) evaluate to inclusive numeric lists
* `without` type derivations
* Config loading (env > config file > defaults)
* JSON encode/decode and refined-type validation
* HTTP routing + error JSON mapping
* CLI flag binding for `fn main` when running `fusec --run <file> -- <args>`
* OpenAPI 3.0 generation via `fusec --openapi` (services, schemas, refined types, error responses)
* package tooling (`fuse.toml`, `fuse run/test/build`)

Today, `native` keeps VM-compatible semantics, with an initial Cranelift JIT fast-path for
direct Int/Bool arithmetic/control-flow function calls. Unsupported instructions fall back to VM.

## Package tooling

`fuse` reads `fuse.toml` from the current directory (or nearest parent) to find the entrypoint:

```
[package]
entry = "src/main.fuse"
app = "Api"
backend = "native"

[build]
openapi = "build/openapi.json"
native_bin = "build/app"

[dependencies]
Auth = { git = "https://github.com/org/auth.fuse", tag = "v0.3.1" }
Utils = { path = "../utils" }
```

`fuse run` and `fuse test` use `package.entry`. `fuse build` runs checks and emits OpenAPI
if `build.openapi` is set. If `build.native_bin` is set, it links a standalone native binary
at that path (native backend; config loading uses `FUSE_CONFIG` + env overrides).

Dependencies are fetched into `.fuse/deps` and locked in `fuse.lock`. Use `dep:` import
paths to reference dependency modules:

```
import Auth from "dep:Auth/lib"
```

Git dependencies support `tag`, `branch`, `rev`, or `version` (resolved as a tag, trying
`v<version>` first).

`fuse build` emits a compiled IR cache at `.fuse/build/program.ir` and a native image cache at
`.fuse/build/program.native`. When present, `fuse run` uses the cached artifact for faster startup
(unless you pass CLI args, which currently bypass the cache).

Use `fuse build --clean` to remove `.fuse/build` and force a fresh compile on the next run.

## "Okay but what's novel?"

Not the syntax. The novelty is the **contract at boundaries**:
config, JSON, validation, and HTTP routing are language-level and consistent across the interpreter,
VM, and current VM-compatible native path.
OpenAPI generation is built-in; richer tooling is planned; see the scope and runtime docs for the roadmap.

## Scope

> [scope.md](scope.md)

## Formal Language Specification

> [fls.md](fls.md)

# Runtime Semantics

> [runtime.md](runtime.md)
