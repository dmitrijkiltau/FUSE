# **FUSE**

*Write intent. Get software.*

FUSE is a small, strict, "default-sane" language for CLI apps and HTTP services. This document
describes the current implementation in this repo (parser + semantic analysis + AST interpreter + VM).

---

## The core vibe (today)

### 1) Small, strict, multi-file MVP

Programs can span multiple files via `import`. Module imports are namespaced; named imports bring items into scope, and module-qualified access works for values and types.

### 2) Strong types, low ceremony

Types exist so your code doesn't lie. You shouldn't have to negotiate with the compiler to get work done.

### 3) Boundaries are first-class

Config, JSON, validation, and HTTP routing are built into the runtime so you don't hand-roll glue.

---

## Syntax: aggressively readable

* Indentation-based blocks (yes, like Python, but strict).
* No semicolons.
* `let` for immutable, `var` for mutable.
* Functions are `fn`.
* Structs are `type`.
* Enums are `enum`.
* String interpolation uses `${expr}` inside double quotes.
* Long call/member chains and call arguments can be split across lines.
* HTML block DSL sugar is supported for call expressions:
  `div(): ...` lowers to `div({}, [...])` and is restricted to calls that return `Html`.

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

---

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
* `Bytes` are stored as raw bytes at runtime and use base64 text at JSON/config/CLI boundaries.
* `Html` is a runtime tree type built via `html.text/raw/node`; `html.render` turns it into `String`.
* HTML block form is compile-time sugar over `List<Html>` children; no implicit string-to-`Html` coercion.

---

## Functions: small and explicit

```fuse
fn greet(user: User) -> String:
  "Hi ${user.name}"
```

Expression-last returns implicitly, but you can `return` when you feel dramatic.

---

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

---

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
* JSON responses by default
* `Html` route responses with `Content-Type: text/html; charset=utf-8`
* mapping `Result` errors to HTTP statuses

### HTMX-friendly pattern (no special runtime integration)

```fuse
type NoteInput:
  title: String(1..80)

service Notes at "/api":
  post "/notes" body NoteInput -> Html:
    return html.node("li", {"class": "note-row"}, [html.text(body.title)])
```

This is the intended pattern for server-driven fragment swaps: return `Html` directly from
mutating routes, keep normal HTTP status/error behavior, and avoid introducing a client-side model.

---

## What works today (MVP)

* Parser + semantic analysis for `fn`, `type`, `enum`, `config`, `service`, `app`
* AST interpreter, VM, and experimental native backend (`--backend native`)
* `import` module loading (namespaced modules + named imports)
* module-qualified type references in type positions (`Foo.User`, `Foo.Config`)
* Built-ins: `print(...)`, `log(...)`, `db.exec/query/one`, `db.from`/`query.*`, `assert(...)`, `env(...)`, `serve(...)`, `task.id/done/cancel`, `html.text/raw/node/render`
* SQLite-backed DB access with parameter binding + query builder (`db.from`/`query.*`) + migrations (`migration` + `fusec --migrate`)
* tests via `test "name":` + `fusec --test` (AST backend)
* `spawn`/`await`/`box` concurrency
* `for`/`while`/`break`/`continue` loops
* range expressions (`a..b`) evaluate to inclusive numeric lists
* `without` type derivations
* Config loading (env > config file > defaults; `.env` is loaded into env for missing values)
* JSON encode/decode and refined-type validation
* HTTP routing + error JSON mapping
* CLI flag binding for `fn main` when running `fusec --run <file> -- <args>`
* OpenAPI 3.0 generation via `fusec --openapi` (services, schemas, refined types, error responses)
* package tooling (`fuse.toml`, `fuse dev/run/test/build`)

Today, `native` keeps VM-compatible semantics, with an initial Cranelift JIT fast-path for
direct Int/Bool arithmetic/control-flow function calls. Unsupported instructions
fail the native backend.

---

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

[serve]
openapi_ui = true
openapi_path = "/docs"

[dependencies]
Auth = { git = "https://github.com/org/auth.fuse", tag = "v0.3.1" }
Utils = { path = "../utils" }
```

`fuse run` and `fuse test` use `package.entry`. `fuse dev` runs `fuse run` in watch mode and
restarts on `.fuse` changes (plus `[assets].scss` when configured), injecting a minimal live-reload script into
HTML responses. `fuse build` runs checks and emits OpenAPI
if `build.openapi` is set. If `build.native_bin` is set, it links a standalone native binary
at that path (native backend; config loading uses `FUSE_CONFIG` + env overrides).
OpenAPI UI serving is enabled automatically in `fuse dev` (route defaults to `/docs`) and is
available for `fuse run` when `[serve].openapi_ui = true`. The spec is generated ahead-of-time by
the CLI and served from a file (`no runtime spec generation`).
`fuse check` semantically checks the package module graph starting at `package.entry`;
`fuse fmt` formats that same module graph.

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
The native cache is versioned; a native cache version bump invalidates `.fuse/build/program.native`
even if source files are unchanged.
For native cold-start regression checks, run `scripts/native_perf_check.sh` and optionally set
`FUSE_NATIVE_COLD_MS` and `FUSE_NATIVE_WARM_MS` budgets.

Use `fuse build --clean` to remove `.fuse/build` and force a fresh compile on the next run.

---

## "Okay but what's novel?"

Not the syntax. The novelty is the **contract at boundaries**:
config, JSON, validation, and HTTP routing are language-level and consistent across the interpreter,
VM, and current VM-compatible native path.
OpenAPI generation is built-in; richer tooling is planned; see the scope and runtime docs for the roadmap.

---

## Related docs

- [scope.md](scope.md): feature scope, language core, runtime/boilerplate killer, tooling
- [fls.md](fls.md): lexing, parsing, semantic analysis, type system, imports, services, runtime notes
- [runtime.md](runtime.md): runtime behavior, built-ins, concurrency model, database access, HTTP binding
