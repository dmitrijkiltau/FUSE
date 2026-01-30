# Scope + constraints

## Target platforms

**MVP targets:**

* **Linux x64/arm64**
* **macOS arm64/x64**
* **Windows x64**

Because if it can’t run where humans actually deploy things, it’s just a hobby with extra steps.

**Runtime modes (both supported):**

* **CLI apps** (default)
* **HTTP services** (builtin server/runtime)

**Later (non-MVP):**

* WASM (nice, but don’t pretend it’s free)
* Embedded (no)
* Mobile (no)

## Host implementation language

**Rust**.

* Great for writing compilers/runtimes.
* Safe concurrency.
* Distributable single-binary toolchain.
* Good ecosystem for parsing (logos, nom), LSP, and codegen.

## Interpreter vs compiler

**Compiler-first**, with a “fast dev loop” mode.

* Primary output: **native executable** (via LLVM or Cranelift).
* Secondary output (optional later): **bytecode + small VM** for super-fast `fuse run` reload.

**MVP choice:** compile to **bytecode + VM** *or* compile to **C** and use system compiler is tempting, but gross.
Best pragmatic MVP: **Cranelift** (via `cranelift-codegen`) to emit native quickly without full LLVM complexity.

## MVP feature set (what ships first)

**Language core**

* Modules + imports
* `let` / `var`
* `fn`
* `type` structs
* `enum`
* pattern matching (minimal)
* string interpolation
* optionals (`T?`)
* fallible results (`T!E` or `T!` with default error)
* refined types on primitives (`String(1..80)`, `Int(0..130)`)
* basic generics for `List<T>`, `Map<K,V>`, `Result<T,E>`, `Option<T>`
* interpolation in double-quoted strings via `${expr}` (escape `$` as `\$`)

**Runtime / “boilerplate killer” MVP**

* JSON encode/decode auto-derive for structs/enums
* validators auto-generated from refined types
* CLI arg parsing from `main` signature
* HTTP `service` block that generates routing + request binding + response encoding
* typed errors mapping to HTTP responses
* minimal logging (`log.info`, `log.warn`, `log.error`)

**Tooling MVP**

* formatter (`fuse fmt`)
* test runner (`fuse test`)
* package/deps file (`fuse.toml`) with lockfile
* LSP later, not day one

## Non-goals (explicitly)

* Full ORM / query language
* Macro system
* Metaprogramming beyond basic derives (at first)
* Custom operator overloads
* Multiple inheritance / traits at MVP (can add interfaces later)
* “Everything async by default” (no, we like sleep)
