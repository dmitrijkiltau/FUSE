# **FUSE**

*Write intent. Get software.*

FUSE is a small, strict, "default-sane" language for CLI apps and HTTP services. This document
describes the current implementation in this repo (parser + semantic analysis + AST interpreter + VM).

## The core vibe (today)

### 1) Small, strict, multi-file MVP

Programs can span multiple files via `import`. Imports are flattened into a single program
namespace (no module qualifiers yet).

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
`fusec` calls `main` directly and binds flags to its parameters (AST backend only); the `app` block
is skipped when program args are present.

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
type NotFound:
  message: String

fn find_user(id: Id) -> User?:
  return null

fn get_user(id: Id) -> User!NotFound:
  let user = find_user(id) ?! NotFound(message="User ${id} not found")
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
* AST interpreter and VM backends
* `import` module loading (flattened namespace)
* Built-ins: `print(...)`, `env(...)`, `serve(...)`
* Config loading (env > config file > defaults)
* JSON encode/decode and refined-type validation
* HTTP routing + error JSON mapping
* CLI flag binding for `fn main` when running `fusec --run <file> -- <args>` (AST backend only)

## Not implemented yet (planned)

* module namespaces / qualified imports
* logging, DB/migrations, tests, docs/OpenAPI generation
* `spawn`/`await`/`box` concurrency
* `for`/`while`/`break`/`continue` at runtime
* `without` type derivations
* package tooling (`fuse.toml`, `fuse run/test/build`)

## "Okay but what's novel?"

Not the syntax. The novelty is the **contract at boundaries**:
config, JSON, validation, and HTTP routing are language-level and consistent across the interpreter and VM.
Docs generation and richer tooling are planned; see the scope and runtime docs for the roadmap.

## Scope

> [scope.md](scope.md)

## Formal Language Specification

> [fls.md](fls.md)

# Runtime Semantics

> [runtime.md](runtime.md)
