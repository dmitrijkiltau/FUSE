# FUSE

FUSE is a small, strict language for building CLI apps and HTTP services.
This document is the product overview companion.

Start at `../README.md` for onboarding, installation, and command workflows.
This file is intentionally non-normative context.

## Document contract

- `Normative`: No.
- `Front door`: No. The single onboarding front door is `../README.md`.
- `Owned concerns`: high-level product framing, design intent, and quick contextual examples.
- `Conflict policy`: if this file conflicts with `../spec/fls.md`, `../spec/runtime.md`,
  `../governance/scope.md`, or `../governance/VERSIONING_POLICY.md`, defer to those documents.

---

## Document role

Use this file to answer:

- what FUSE is optimizing for
- where to find authoritative specs for syntax, semantics, and runtime behavior
- what tooling/workflow exists at a high level

Do not use this file as the final authority for parser/runtime edge cases.

---

## Developer navigation

Primary references while working in this codebase:

- `../README.md` is the front door for setup, build/test/release commands, and doc routing.
- `../governance/IDENTITY_CHARTER.md` defines language identity, hard boundaries, and "will not do" constraints.
- `../spec/fls.md` is the source of truth for language syntax and static semantics (lexer, grammar, AST shape, type system, module rules).
- `../spec/runtime.md` is the source of truth for runtime semantics (validation, JSON/config/CLI/HTTP binding, errors, builtins, DB, and `spawn`/`await` concurrency model).
- `../governance/scope.md` defines constraints, roadmap priorities, and explicit non-goals.
- `../governance/EXTENSIBILITY_BOUNDARIES.md` defines allowed extension surfaces and stability boundaries.
- `../ops/BENCHMARKS.md` defines real-world workload benchmarks and metric collection.
- `../governance/VERSIONING_POLICY.md` defines language/runtime/tooling versioning and compatibility guarantees.

If a detail appears in multiple docs, treat `../governance/IDENTITY_CHARTER.md` as authoritative
for identity boundaries, `../spec/fls.md` for syntax/static rules, and `../spec/runtime.md` for
runtime behavior.

See also: [What FUSE optimizes for](#what-fuse-optimizes-for), [Guiding idea](#guiding-idea), [FUSE identity charter](../governance/IDENTITY_CHARTER.md).

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

See also: [Quick examples](#quick-examples), [Formal language specification](../spec/fls.md), [Runtime semantics](../spec/runtime.md).

---

## Quick examples

### CLI app

```fuse
fn main(name: String = "world"):
  print("Hello, ${name}!")

app "hello":
  main()
```

### HTTP service

```fuse
requires network

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

For detailed route binding and error/status behavior, see `../spec/runtime.md`.

See also: [Runtime semantics](../spec/runtime.md), [Formal language specification](../spec/fls.md), [Package workflow (summary)](#package-workflow-summary).

---

## Implementation snapshot

FUSE currently ships with:

- parser + semantic analysis + formatter
- AST interpreter backend
- native backend (Cranelift JIT)
- semantic parity gates across AST/native backends
- module imports (relative, `root:`, and `dep:` paths)
- compile-time module capability declarations (`requires db|crypto|network|time`)
- typed error-domain boundaries on function/service returns (`T!Domain`, no implicit `T!`)
- structured-concurrency checks for `spawn`/`await` task lifetimes (no detached/orphaned tasks)
- deterministic `transaction:` blocks (commit on success, rollback on failure) with compile-time restrictions
- strict architecture mode (`--strict-architecture`) for capability purity, cross-layer cycle rejection, and error-domain isolation
- package tooling via `fuse.toml` and `fuse` commands

Detailed capability matrices and caveats live in:

- `../spec/runtime.md` for execution behavior
- `../governance/scope.md` for current scope vs planned work

See also: [Package workflow (summary)](#package-workflow-summary), [Runtime semantics](../spec/runtime.md), [Scope + constraints](../governance/scope.md).

---

## Semantic authority contract

FUSE follows a single semantic authority model: parser + frontend canonicalization define language
semantics; backends are execution strategies over canonical forms. Backend-specific reinterpretation
of source syntax is a correctness bug.

Full pipeline and parity release gates are documented in
[Backends](../spec/runtime.md#backends).

See also: [Runtime surface and ownership](../spec/runtime.md).

---

## Package workflow (summary)

Typical commands:

- `fuse check`
- `fuse check --strict-architecture`
- `fuse run`
- `fuse dev`
- `fuse test`
- `fuse build`
- `fuse build --aot`
- `fuse build --aot --release`

Minimal manifest:

```toml
[package]
entry = "src/main.fuse"
app = "Api"
backend = "native"
```

`fuse.toml` supports additional build/serve/assets/dependency settings. See `../README.md` for command details and examples.

Dependency contract highlights:

- `[dependencies]` supports local `path` sources and git sources (`git` with optional `rev` / `tag` / `branch` / `version` and optional `subdir`).
- `fuse.lock` records resolved dependency sources for reproducible resolution.
- conflicting transitive dependency specs by name are rejected deterministically.

See also: [Implementation snapshot](#implementation-snapshot), [Runtime semantics](../spec/runtime.md), [Scope + constraints](../governance/scope.md).

---

## Guiding idea

FUSE is not trying to invent new syntax. The differentiator is a consistent contract at boundaries: types, validation, and transport behavior are aligned by default.

For the formal spec, start at `../spec/fls.md`. For concrete runtime behavior, start at `../spec/runtime.md`.

See also: [Formal language specification](../spec/fls.md), [Runtime semantics](../spec/runtime.md), [Scope + constraints](../governance/scope.md).
