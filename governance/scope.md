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

For a full cross-reference of project documents, see the
[Documentation map](../README.md#documentation-map).

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

See also: [Runtime semantics](../spec/runtime.md), [README](../README.md).

---

## Current boundary (what this repo is aiming to deliver)

FUSE is currently scoped as a strict, typed language with integrated boundary/runtime tooling for:

- language authoring (`fn`, `type`, `enum`, modules/imports, services/config/apps)
- backend execution across AST/native with semantic parity gates (including operator semantics such as `Bool` equality/inequality); VM removed per RFC 0007
- integrated runtime boundary handling (validation, JSON/config/CLI/HTTP binding, error mapping)
- compile-time module capability declarations (`requires db|crypto|network|time`) with
  boundary-call and cross-module leakage checks
- typed error-domain boundaries on function/service returns (explicit `T!Domain`, no implicit `T!`)
- structured-concurrency lifetime checks for `spawn`/`await` (no detached/orphaned task bindings)
- deterministic `transaction:` block semantics with compile-time transactional restrictions
- strict architecture compile mode (`--strict-architecture`) enforcing capability purity,
  cross-layer import-cycle rejection, and error-domain isolation
- SQLite-backed DB runtime and migration workflow
- HTML DSL component composition with implicit `attrs`/`children`, using boundary-safe conventions:
  pass presentation attrs through `attrs`, nested markup through `children`, and normalize typed
  boundary data before render
- service-oriented package workflow (`fuse check/run/dev/test/build`,
  `fuse clean --cache`, `fuse deps lock`, `fuse deps publish-check`,
  frozen lock enforcement)
- HTTP handler request/response primitives for header and cookie handling

Detailed behavior is intentionally kept out of this doc and lives in `../spec/fls.md` and `../spec/runtime.md`.
Identity guardrails are defined in `IDENTITY_CHARTER.md`.

See also: [Formal language specification](../spec/fls.md), [Runtime semantics](../spec/runtime.md#runtime-surface-and-ownership).

---

## Priority roadmap

Completed in `0.8.0`:

1. Runtime capability parity for `time` and `crypto`
2. Native IR call-target lowering completion for previously unsupported forms
3. LSP modularization with stable behavior and preserved test contracts
4. Example/reference coverage expansion for major language/runtime features
5. DB/config ergonomics improvements (query-builder CRUD + structured config overrides)
6. Developer workflow upgrades (`fuse check` incremental, `fuse dev` compile overlay, diagnostics JSON, AOT progress)
7. Pre-tag cleanup and redundancy removal (`M7A`) plus docs-to-guides migration (`M7B`)

Completed in `0.9.0`:

1. Native backend performance hardening (hot-path lowering, JSON encoding fast paths,
   route matching cache, GC allocation guard, tighter perf regression gates)
2. Concurrency throughput and observability upgrades (true async native `spawn`,
   lower-contention task scheduling, structured concurrency metrics output)
3. Dependency/package workflow hardening for larger multi-package repositories
   (`dep:`/`root:` transitive resolution, cycle diagnostics, `fuse check --workspace`)
4. LSP scalability for large workspaces (progressive indexing, persisted workspace
   index cache, 50-file latency budget enforcement harness)
5. Release automation simplification (`bump_version.sh`, `release_preflight.sh`,
   unified `package_release.sh`, workflow dry-run support)
6. HTML DSL expansion and cleanup (`component` declarations, compile-time `aria-*`
   checks, canonicalized no-comma attribute shorthand with migration diagnostics)
7. DB layer boundary-first upgrades (typed query row decoding via `.all<T>()`/`.one<T>()`,
   `upsert`, migration namespace isolation by `(package, name)`)

Next priorities (post-`0.9.0`):

1. Broaden component/UI ergonomics while keeping the static boundary model strict.
2. Continue package/dependency workflow evolution for publishable multi-package repos.
3. Refine LSP relevance and indexing UX beyond current latency/scalability gates.
4. Expand DB/runtime ergonomics without crossing into full ORM territory.
5. Strengthen release artifact integrity/provenance automation in CI.

Likely future candidates (not committed MVP scope):

- richer interface/abstraction mechanisms
- expanded database/runtime ergonomics
- stronger packaging/dependency workflows

See also: [Backends](../spec/runtime.md#backends), [Builtins and runtime subsystems](../spec/runtime.md#builtins-and-runtime-subsystems).

---

## Non-goals (explicit)

Identity-level non-goals (no generics, no macros, no reflection, no operator overloading,
no inheritance model, no backend dialects) are defined authoritatively in
[IDENTITY_CHARTER.md](IDENTITY_CHARTER.md#explicit-will-not-do-list).

Additional scope non-goals:

- full ORM / heavyweight query language
- "everything async by default"

See also: [FUSE identity charter](IDENTITY_CHARTER.md), [README](../README.md).
