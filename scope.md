# Scope + constraints

This document reflects the current scope in this repo and what is planned next.

## Target platforms

**Intended runtime targets:**

* **Linux x64/arm64**
* **macOS arm64/x64**
* **Windows x64**

The current implementation is a Rust interpreter + VM, so it runs wherever the host binary runs.

**Runtime modes (supported):**

* **CLI apps**
* **HTTP services** (builtin server/runtime)

**Later (non-MVP):**

* WASM (nice, but not in MVP)
* Embedded (no)
* Mobile (no)

## Host implementation language

**Rust**.

* Great for writing compilers/runtimes.
* Safe concurrency.
* Distributable single-binary toolchain.
* Good ecosystem for parsing, LSP, and codegen.

## Execution model

**Current:**

* AST interpreter backend
* Bytecode + VM backend
* No native compiler/codegen yet

**Planned:**

* Native compiler (Cranelift/LLVM TBD)
* Faster `fuse run` loop once codegen exists

## Feature scope

**Language core (implemented)**

* `let` / `var`
* `fn`
* `type` structs
* `without` type derivations
* `enum`
* `import` (namespaced module imports + named imports for local scope)
* module-qualified type references in type positions (`Foo.User`)
* `config`, `service`, `app`
* `test` declarations
* `if` / `else`, `match` (struct/enum/Option/Result patterns)
* `for` / `while` / `break` / `continue`
* string interpolation via `${expr}` (escape `$` as `\$`)
* optionals (`T?`)
* fallible results (`T!E` or `T!` with default error)
* refined types on primitives (`String(1..80)`, `Int(0..130)`)
* generics for `List<T>`, `Map<K,V>`, `Result<T,E>`, `Option<T>`
* `migration` declarations
* `spawn` / `await` / `box` concurrency (AST backend only)

**Runtime / "boilerplate killer" (implemented)**

* JSON encode/decode for structs/enums
* validation derived from refined types
* config loading (env > config file > defaults)
* config/env/CLI parsing is limited to scalar types + `Option` + refined ranges
* HTTP request binding + response encoding
* error JSON + HTTP status mapping
* builtins: `print`, `log`, `db`, `assert`, `env`, `serve`
* SQLite-backed DB access + migrations via `fusec --migrate`
* CLI arg binding for `fn main` when running with program args (AST backend only)

**Tooling (implemented)**

* parser + semantic analysis
* formatter via `fusec --fmt`
* OpenAPI 3.0 generation via `fusec --openapi`
* `fusec` flags: `--check`, `--run`, `--migrate`, `--test`, `--openapi`, `--backend`, `--app`
* package manifest (`fuse.toml`) + `fuse run/test/build`

**Tooling (planned)**

* lockfile + dependency resolution
* LSP (not day one)

## Non-goals (explicitly)

* Full ORM / query language
* Macro system
* Metaprogramming beyond basic derives (at first)
* Custom operator overloads
* Multiple inheritance / traits at MVP (interfaces later, maybe)
* "Everything async by default" (no, we like sleep)
