# Scope + constraints

This document defines project boundaries: what FUSE targets, what it intentionally does not target,
and where near-term effort is going.

It is a planning/boundary document, not a semantic specification. Behavioral correctness remains
owned by `../spec/fls.md` (syntax/static semantics) and `../spec/runtime.md` (runtime semantics).

## Document contract

- `Normative`: No for language/runtime semantics.
- `Front door`: No. Start onboarding from `../README.md`.
- `Owned concerns`: project constraints, delivery boundaries, near-term priorities, and explicit
  non-goals.
- `Conflict policy`: syntax/static/runtime behavior defers to `../spec/fls.md` and `../spec/runtime.md`; identity
  constraints defer to `IDENTITY_CHARTER.md`.

Companion references for implementation work:

- `../README.md` is the repository front door for setup/workflow/document routing
- `../guides/fuse.md` gives product-level narrative context
- `IDENTITY_CHARTER.md` defines non-negotiable language identity boundaries
- `EXTENSIBILITY_BOUNDARIES.md` defines allowed extension surfaces and stability boundaries
- `../ops/BENCHMARKS.md` defines real-world workload benchmarks and metric collection
- `VERSIONING_POLICY.md` defines language/runtime/tooling compatibility and deprecation rules
- `../spec/fls.md` specifies language and static-semantics details
- `../spec/runtime.md` specifies execution/runtime behavior

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

See also: [Runtime semantics](../spec/runtime.md), [README](../README.md), [FUSE overview companion](../guides/fuse.md).

---

## Current boundary (what this repo is aiming to deliver)

FUSE is currently scoped as a strict, typed language with integrated boundary/runtime tooling for:

- language authoring (`fn`, `type`, `enum`, modules/imports, services/config/apps)
- backend execution across AST/VM/native with semantic parity gates
- integrated runtime boundary handling (validation, JSON/config/CLI/HTTP binding, error mapping)
- SQLite-backed DB runtime and migration workflow
- service-oriented package workflow (`fuse check/run/dev/test/build`)

Detailed behavior is intentionally kept out of this doc and lives in `../spec/fls.md` and `../spec/runtime.md`.
Identity guardrails are defined in `IDENTITY_CHARTER.md`.

See also: [Formal language specification](../spec/fls.md), [Runtime semantics](../spec/runtime.md#runtime-surface-and-ownership).

---

## Priority roadmap

Near-term priorities:

1. AOT production performance/operability hardening (startup SLOs, observability hooks, rollback readiness)
2. Native backend maturity and predictability
3. Concurrency throughput and observability improvements on the existing worker-pool + deterministic `spawn` model
4. Faster run/build iteration with hash-validated cache artifacts
5. Tooling quality for multi-file projects (diagnostics, refactors, discoverability)
6. Publishable VS Code distribution artifacts (`.vsix`)

Likely future candidates (not committed MVP scope):

- richer interface/abstraction mechanisms
- expanded database/runtime ergonomics
- stronger packaging/dependency workflows

See also: [Backends](../spec/runtime.md#backends), [Builtins and runtime subsystems](../spec/runtime.md#builtins-and-runtime-subsystems).

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

See also: [FUSE identity charter](IDENTITY_CHARTER.md), [Guiding idea](../guides/fuse.md#guiding-idea), [README](../README.md).
