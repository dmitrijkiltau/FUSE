# FUSE Identity Charter

This charter defines what FUSE is, what it is not, and which boundaries are non-negotiable.
Its purpose is to prevent feature creep and keep language/runtime decisions coherent.

## Mission

FUSE is a strict, deterministic language for building small CLI apps and HTTP services with
contract-first boundaries:

- typed inputs/outputs
- built-in validation
- predictable JSON/config/CLI/HTTP binding
- consistent backend semantics (AST/native; VM deprecated per RFC 0007)

## Product Identity

FUSE is:

- a boundary-centric application language
- optimized for service and automation workloads
- intentionally small and constrained

FUSE is not:

- a general-purpose "do everything" framework
- a macro/metaprogramming playground
- a dynamic runtime with reflective behavior

## Non-Negotiable Rules

1. Semantic authority:
   - AST + frontend canonicalization define semantics.
   - VM/native are execution strategies, not semantic authorities.
2. Determinism:
   - Equivalent programs must behave the same across backends.
   - Backend-specific semantic behavior is a bug.
3. Boundary-first contracts:
   - Validation and binding behavior is explicit and spec-defined.
   - "Magic" implicit behavior at boundaries is out of scope.
4. Small surface area:
   - New syntax/features must justify complexity against core mission.

## Explicit "Will Not Do" List

FUSE will not add the following language classes in the current product identity:

- user-defined generics / parametric polymorphism
- ad-hoc polymorphism (trait-style overload systems)
- runtime reflection/introspection APIs that alter semantic behavior
- macro systems (compile-time code generation / syntax rewriting by users)
- operator overloading or custom operators
- inheritance-heavy object model design
- backend-specific language dialects

Existing built-in generic containers (`Option<T>`, `Result<T,E>`, `List<T>`, `Map<K,V>`) are part
of the fixed core type system and do not imply user-defined generic abstractions.

## Change Filter (Required for New Features)

A proposal should be rejected if any answer is "no":

1. Does it strengthen CLI/HTTP boundary-centric development?
2. Does it preserve deterministic cross-backend semantics?
3. Can it be specified clearly in `../spec/fls.md` and `../spec/runtime.md` without hidden behavior?
4. Does it keep the language smaller or equally simple relative to its value?
5. Is it compatible with this charter's "Will Not Do" list?

If a valuable feature fails this filter, the charter must be changed explicitly first.

## Authority and Precedence

For identity/scope disputes:

1. `IDENTITY_CHARTER.md` (this document)
2. `scope.md`
3. `../guides/fuse.md`

For semantic/runtime details, `../spec/fls.md` and `../spec/runtime.md` remain authoritative.
