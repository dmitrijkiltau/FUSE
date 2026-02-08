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
* Experimental `native` backend path (`--backend native`) backed by compiled IR image + VM compatibility runtime
* Native includes a Cranelift JIT slice for direct `fn` calls over Int/Bool arithmetic + control flow; unsupported instructions fail in native

**Planned:**

* True native codegen (Cranelift/LLVM TBD)
* Faster `fuse run` loop from native machine-code artifacts (beyond current IR/native image cache)
* Concurrency API maturity: task identity, cancellation hooks, and structured-concurrency semantics

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
* range expressions (`a..b`) produce numeric lists
* generics for `List<T>`, `Map<K,V>`, `Result<T,E>`, `Option<T>`
* `migration` declarations
* `spawn` / `await` / `box` concurrency

**Runtime / "boilerplate killer" (implemented)**

* JSON encode/decode for structs/enums
* validation derived from refined types
* config loading (env > config file > defaults)
* config/env/CLI parsing is limited to scalar types + `Option` + refined ranges
* HTTP request binding + response encoding
* error JSON + HTTP status mapping
* builtins: `print`, `log`, `db.exec/query/one/from`, `query.*`, `assert`, `env`, `serve`, `task.id`, `task.done`, `task.cancel`
* SQLite-backed DB access + migrations via `fusec --migrate`
* CLI arg binding for `fn main` when running with program args

**Tooling (implemented)**

* parser + semantic analysis
* formatter via `fusec --fmt`
* OpenAPI 3.0 generation via `fusec --openapi`
* `fusec` flags: `--check`, `--run`, `--migrate`, `--test`, `--openapi`, `--backend` (`ast|vm|native`), `--app`
* package manifest (`fuse.toml`) + `fuse run/test/build`
* dependency fetching + `fuse.lock`
* IR cache for fast `fuse run` (`.fuse/build/program.ir`)
* native image cache (`.fuse/build/program.native`) for `--backend native`
* native perf smoke check (`scripts/native_perf_check.sh`, optional budgets)
* LSP: diagnostics, formatting, go-to-definition, hover, rename, workspace symbols, project-wide defs/refs, find references, call hierarchy, code actions (missing import, qualify symbol, organize imports)

**Tooling (planned)**

* LSP UX: semantic tokens, docstring hover, inlay hints (types/params)
* CLI ergonomics: project‑wide `fmt`/`check`, better parse/sema spans on multi‑file errors

## Non-goals (explicitly)

* Full ORM / query language
* Macro system
* Metaprogramming beyond basic derives (at first)
* Custom operator overloads
* Multiple inheritance / traits at MVP (interfaces later, maybe)
* "Everything async by default" (no, we like sleep)
