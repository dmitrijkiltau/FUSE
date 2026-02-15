# Scope + constraints

This document defines project boundaries: what FUSE targets, what it intentionally does not target,
and where near-term effort is going.

Companion references for implementation work:

- `fuse.md` gives the product-level overview
- `fls.md` specifies language and static-semantics details
- `runtime.md` specifies execution/runtime behavior

---

## Constraints

### Target platforms

Primary runtime targets:

- Linux (`x64`, `arm64`)
- macOS (`arm64`, `x64`)
- Windows (`x64`)

Current implementation is Rust-based and runs wherever the host toolchain binaries run.

### Runtime modes

Supported:

- CLI apps
- HTTP services (built-in server/runtime)

Not in MVP target:

- WASM deployment
- embedded targets
- mobile targets

### Host implementation language

Rust is the implementation language for compiler/runtime/tooling.

Rationale:

- safety and predictable performance
- strong ecosystem for compiler + tooling infrastructure
- distributable single-binary tooling path

See also: [Runtime semantics](runtime.md), [FUSE overview](fuse.md).

---

## Current boundary (what this repo is aiming to deliver)

FUSE is currently scoped as a strict, typed language with integrated boundary/runtime tooling for:

- language authoring (`fn`, `type`, `enum`, modules/imports, services/config/apps)
- module-scoped function symbol resolution (local module + explicit imports)
- backend execution across AST/VM/native path with aligned semantics
- runtime boundary handling (validation, JSON/config/CLI/HTTP binding, error mapping)
- service-oriented package workflow (`fuse check/run/dev/test/build`)

Detailed behavior is intentionally kept out of this doc and lives in `fls.md` and `runtime.md`.

See also: [Formal language specification](fls.md), [Runtime semantics](runtime.md#runtime-surface-and-ownership).

---

## Priority roadmap

Near-term priorities:

1. Native backend maturity and predictability
2. Faster run/build iteration from cached/native artifacts
3. Concurrency model evolution beyond eager task execution
4. Tooling quality for multi-file projects (diagnostics, refactors, discoverability)

Likely future candidates (not committed MVP scope):

- richer interface/abstraction mechanisms
- expanded database/runtime ergonomics
- stronger packaging/dependency workflows

See also: [Backends](runtime.md#backends), [Builtins and runtime subsystems](runtime.md#builtins-and-runtime-subsystems).

---

## Non-goals (explicit)

- full ORM / heavyweight query language
- macro system
- broad metaprogramming beyond basic derivation forms
- custom operator overloading
- multiple inheritance at MVP
- "everything async by default"

See also: [Guiding idea](fuse.md#guiding-idea), [FUSE overview](fuse.md).
