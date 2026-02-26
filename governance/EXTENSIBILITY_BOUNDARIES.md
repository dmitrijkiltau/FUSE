# FUSE Extensibility Boundaries

This document defines how FUSE can be extended without breaking deterministic language semantics.

Companion references:

- `IDENTITY_CHARTER.md` for non-negotiable product boundaries
- `../spec/fls.md` for syntax/static semantics
- `../spec/runtime.md` for runtime/boundary semantics
- `LSP_ROADMAP.md` for editor capability planning

## Core rule

Semantic authority stays in parser + frontend canonicalization + semantic analysis.
Backends execute canonical forms; they do not define language behavior.

## Extension classes

### 1) User macros and syntax plugins

Status: `NOT ALLOWED`

- No user macro system.
- No compile-time user syntax rewriting.
- No custom operators or parser plugins.

Reason: these extension classes break the small deterministic core and conflict with
`IDENTITY_CHARTER.md`.

### 2) Custom backends

Status: `INTERNAL ONLY` (maintainer-owned, in-tree)

- Public backend set is fixed to `ast | native`.
- `vm` is **deprecated** (RFC 0007) and retained only during the deprecation window;
  it will be removed in the next breaking minor release.
- No dynamic backend plugin loading API is exposed.
- Any new backend must consume canonical frontend outputs, not raw source re-interpretation.

Required for adding/changing a backend:

1. No backend-local sugar lowering.
2. No backend-specific semantic behavior.
3. AST/native parity tests and release gates stay green.
4. `../guides/fuse.md`, `../spec/runtime.md`, and tests are updated in the same change.

### 3) Type-system extensions

Status: `CLOSED TO USER PLUGINS`

Users can:

- declare `type` and `enum`
- compose fixed core containers (`Option<T>`, `Result<T,E>`, `List<T>`, `Map<K,V>`)

Users cannot:

- register custom type constructors in the compiler
- add user-defined generic systems/traits/polymorphism frameworks
- add runtime reflection hooks that alter type behavior

Core type changes are language changes and require spec + tests, not per-project plugins.

### 4) Runtime hooks

Status: `ALLOWED (NON-SEMANTIC ONLY)`

Currently supported:

- package build hook: `[assets.hooks].before_build`

Boundary:

- hooks may orchestrate build assets/tooling
- hooks must not alter compiler/runtime language semantics

If a hook changes semantic meaning of source, it is out of scope and must be rejected.

### 5) IDE/editor hooks

Status: `ALLOWED VIA LSP`

- Stable integration boundary is Language Server Protocol (`fuse-lsp`) plus editor grammar assets.
- Editor tooling should integrate through LSP requests/capabilities, not internal Rust modules.
- Internal LSP/compiler crate APIs are not a stable extension contract.

## Stability tiers for extension surfaces

- `STABLE`: documented language semantics and supported protocol-level editor behavior
- `PROVISIONAL`: optional tooling behavior that may evolve between minor releases
- `INTERNAL`: compiler/runtime internals with no compatibility guarantee

Any new extension point must declare its tier explicitly before adoption.

## Acceptance checklist for new extension points

A proposal is eligible only if all answers are "yes":

1. Does it preserve canonical AST semantic authority?
2. Does it preserve AST/native observable parity?
3. Does it keep boundary contracts explicit and deterministic?
4. Is the extension surface and stability tier clearly documented?
5. Are semantic tests and release gates added/updated with the change?
