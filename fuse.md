# FUSE

*Write intent. Get software.*

FUSE is a small, strict language for building CLI apps and HTTP services.
This document is the overview. It explains the model and how to navigate the rest of the docs.

---

## Developer navigation

Primary references while working in this codebase:

- `IDENTITY_CHARTER.md` defines language identity, hard boundaries, and "will not do" constraints.
- `fls.md` is the source of truth for language syntax and static semantics (lexer, grammar, AST shape, type system, module rules).
- `runtime.md` is the source of truth for runtime semantics (validation, JSON/config/CLI/HTTP binding, errors, builtins, DB, and `spawn`/`await` concurrency model).
- `scope.md` defines constraints, roadmap priorities, and explicit non-goals.
- `EXTENSIBILITY_BOUNDARIES.md` defines allowed extension surfaces and stability boundaries.
- `BENCHMARKS.md` defines real-world workload benchmarks and metric collection.
- `VERSIONING_POLICY.md` defines language/runtime/tooling versioning and compatibility guarantees.

If a detail appears in multiple docs, treat `IDENTITY_CHARTER.md` as authoritative for identity/scope boundaries, `fls.md` for syntax/static rules, and `runtime.md` for runtime behavior.

See also: [What FUSE optimizes for](#what-fuse-optimizes-for), [Guiding idea](#guiding-idea), [FUSE identity charter](IDENTITY_CHARTER.md).

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

See also: [Quick taste](#quick-taste), [Formal language specification](fls.md), [Runtime semantics](runtime.md).

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

See also: [Runtime semantics](runtime.md), [Formal language specification](fls.md), [Package workflow (summary)](#package-workflow-summary).

---

## Current implementation (summary)

FUSE currently ships with:

- parser + semantic analysis + formatter
- AST interpreter backend
- VM backend
- native backend path targeting VM-compatible semantics
- module imports (relative paths, package-root paths via `root:`, and dependency paths via `dep:`)
- module-scoped function symbols (local-first, then named-import resolution)
- package tooling via `fuse.toml` and `fuse` commands

Detailed capability matrices and caveats live in:

- `runtime.md` for execution behavior
- `scope.md` for current scope vs planned work

See also: [Package workflow (summary)](#package-workflow-summary), [Runtime semantics](runtime.md), [Scope + constraints](scope.md).

---

## Semantic authority contract

FUSE follows a single semantic authority model:

- parser + frontend canonicalization define language semantics
- canonical AST is the semantic program
- VM and native are execution strategies over canonical forms
- backend-specific reinterpretation of source syntax is a correctness bug

Pipeline:

1. source parses into AST
2. frontend canonicalization lowers syntax sugar on AST forms (for example HTML block children and string-child lowering)
3. semantic checks run on canonical AST
4. VM/native lower or execute canonical forms with equivalent behavior

Authority/parity release gates:

- `./scripts/semantic_suite.sh` (parser/sema/boundary semantic contract suite)
- `./scripts/authority_parity.sh` (explicit semantic-authority suite)
- `./scripts/release_smoke.sh` (includes authority parity + full smoke checks)

See also: [Backends](runtime.md#backends), [Runtime surface and ownership](runtime.md#runtime-surface-and-ownership).

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

See also: [Current implementation (summary)](#current-implementation-summary), [Runtime semantics](runtime.md), [Scope + constraints](scope.md).

---

## Guiding idea

FUSE is not trying to invent new syntax. The differentiator is a consistent contract at boundaries: types, validation, and transport behavior are aligned by default.

For the formal spec, start at `fls.md`. For concrete runtime behavior, start at `runtime.md`.

See also: [Formal language specification](fls.md), [Runtime semantics](runtime.md), [Scope + constraints](scope.md).
