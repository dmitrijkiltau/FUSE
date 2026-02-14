# FUSE

*Write intent. Get software.*

FUSE is a small, strict language for building CLI apps and HTTP services.
This document is the overview. It explains the model and how to navigate the rest of the docs.

---

## Developer navigation

Primary references while working in this codebase:

- `fls.md` is the source of truth for language syntax and static semantics (lexer, grammar, AST shape, type system, module rules).
- `runtime.md` is the source of truth for runtime semantics (validation, JSON/config/CLI/HTTP binding, errors, builtins, DB, task model).
- `scope.md` defines constraints, roadmap priorities, and explicit non-goals.
- `README.md` covers install/build workflow and day-to-day commands.

If a detail appears in multiple docs, treat `fls.md` as authoritative for syntax/static rules and `runtime.md` as authoritative for runtime behavior.

---

## What FUSE optimizes for

### 1) Small and strict

The language intentionally keeps a narrow core:

- indentation-based blocks
- explicit declarations (`fn`, `type`, `enum`, `config`, `service`, `app`)
- strong types with minimal ceremony

### 2) Boundaries as first-class language concerns

Runtime surfaces are built in and consistent across backends:

- config loading
- JSON encoding/decoding
- validation
- HTTP request/response binding

### 3) One source of truth per concern

You describe contracts in FUSE types and route signatures. The runtime applies those contracts at boundaries instead of requiring repeated glue code.

---

## Quick taste

### CLI app

```fuse
fn main(name: String = "world"):
  print("Hello, ${name}!")

app "hello":
  main()
```

### HTTP service

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

For detailed route binding and error/status behavior, see `runtime.md`.

---

## Current implementation (summary)

FUSE currently ships with:

- parser + semantic analysis + formatter
- AST interpreter backend
- VM backend
- native backend path targeting VM-compatible semantics
- module imports (including dependency imports via `dep:`)
- package tooling via `fuse.toml` and `fuse` commands

Detailed capability matrices and caveats live in:

- `runtime.md` for execution behavior
- `scope.md` for current scope vs planned work

---

## Package workflow (summary)

Typical commands:

- `fuse check`
- `fuse run`
- `fuse dev`
- `fuse test`
- `fuse build`

Minimal manifest:

```toml
[package]
entry = "src/main.fuse"
app = "Api"
backend = "native"
```

`fuse.toml` supports additional build/serve/assets/dependency settings. See `README.md` for command details and examples.

---

## Guiding idea

FUSE is not trying to invent new syntax. The differentiator is a consistent contract at boundaries: types, validation, and transport behavior are aligned by default.

For the formal spec, start at `fls.md`. For concrete runtime behavior, start at `runtime.md`.
