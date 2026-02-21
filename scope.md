# Scope + constraints

This document defines project boundaries: what FUSE targets, what it intentionally does not target,
and where near-term effort is going.

Companion references for implementation work:

- `fuse.md` gives the product-level overview
- `IDENTITY_CHARTER.md` defines non-negotiable language identity boundaries
- `EXTENSIBILITY_BOUNDARIES.md` defines allowed extension surfaces and stability boundaries
- `BENCHMARKS.md` defines real-world workload benchmarks and metric collection
- `VERSIONING_POLICY.md` defines language/runtime/tooling compatibility and deprecation rules
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
- pooled SQLite DB execution with configurable pool sizing and migration transaction safety
- service-oriented package workflow (`fuse check/run/dev/test/build`)

Detailed behavior is intentionally kept out of this doc and lives in `fls.md` and `runtime.md`.
Identity guardrails are defined in `IDENTITY_CHARTER.md`.

See also: [Formal language specification](fls.md), [Runtime semantics](runtime.md#runtime-surface-and-ownership).

---

## Priority roadmap

Near-term priorities:

1. Native backend maturity and predictability
2. Real worker-pool concurrency with deterministic `spawn` restrictions
3. Faster run/build iteration with hash-validated cache artifacts
4. Tooling quality for multi-file projects (diagnostics, refactors, discoverability)
5. Publishable VS Code distribution artifacts (`.vsix`)

Likely future candidates (not committed MVP scope):

- richer interface/abstraction mechanisms
- expanded database/runtime ergonomics
- stronger packaging/dependency workflows

See also: [Backends](runtime.md#backends), [Builtins and runtime subsystems](runtime.md#builtins-and-runtime-subsystems).

---

## Non-goals (explicit)
- full ORM / heavyweight query language
- user macro/metaprogramming system
- user-defined generics / polymorphism systems
- runtime reflection/introspection features that alter language behavior
- custom operator overloading
- inheritance-heavy object model features
- backend-specific semantic dialects
- "everything async by default"

See also: [FUSE identity charter](IDENTITY_CHARTER.md), [Guiding idea](fuse.md#guiding-idea), [FUSE overview](fuse.md).
