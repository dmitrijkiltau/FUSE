# RFC: Generic Callables and `where` Clauses (v1.1)

## Document contract

- `Normative`: Yes, for the generic callable and `where` surface introduced in v1.1.
- `Front door`: No. Start from `README.md`.
- `Owned concerns`: type parameter declarations on `fn`, interface members, impl methods, and
  `component`; trailing `where` clauses; explicit call-site type arguments; type inference at
  generic call sites; frontend monomorphization.
- `Conflict policy`: any conflict with `spec/fls.md` or `spec/runtime.md` resolves in favor of
  those documents. Any conflict with `governance/IDENTITY_CHARTER.md` resolves in favor of the
  charter.

See also: [RFC interface contracts](RFC_INTERFACE_CONTRACTS.md),
[Language spec](../spec/fls.md), [Runtime spec](../spec/runtime.md),
[Language reference](../guides/reference.md).

---

## Summary

This RFC extends the v1.1 interface-contract base with explicit generic callable declarations,
trailing `where` clauses, explicit call-site type arguments, and frontend monomorphization. All
generic dispatch remains a compile-time, static-dispatch feature. No runtime polymorphism, vtables,
or trait-object behavior is introduced.

---

## Motivation

The v1.1 interface base (see `RFC_INTERFACE_CONTRACTS.md`) lets multiple types share a named
behavioral contract, but callables cannot yet express that a type parameter must satisfy such a
contract. Authors are forced to duplicate function signatures or hard-code concrete types at
package boundaries. This RFC closes that gap by allowing `fn`, interface members, impl methods, and
`component` to carry type parameters and `where` constraints, resolved entirely at compile time
through frontend monomorphization.

---

## Design

### Type parameter syntax

Type parameters are written in angle brackets immediately after the callable name:

```fuse
fn decode<T>(text: String) -> T where T: Encodable:
  return T.decode(text)
```

The `TypeParams` production is:

```ebnf
TypeParams  ::= "<" Ident { "," Ident } ">"
Constraint  ::= Ident ":" Ident
WhereClause ::= "where" Constraint { "," Constraint }
```

`where` is contextual-only. It is parsed as a declaration keyword only in trailing clause position
after a declaration head. It is not added to the globally reserved keyword set. Existing source
using `.where(...)` in query-builder expressions is unaffected.

### Declarations that accept type parameters

| Declaration form | Type params allowed |
|---|---|
| `fn` | yes |
| interface member | yes |
| impl method | yes |
| `component` | yes |
| `type`, `enum`, `interface` header | no |
| `impl` block | no |
| `app`, `test`, service routes | no |

### Call-site type arguments

Explicit type arguments follow the callable name at the call site:

```fuse
let user = decode<User>(text)
```

Type arguments may also be inferred from value argument types and receiver type:

```fuse
fn round_trip<T>(x: T) -> T where T: Encodable:
  let encoded = x.encode()
  return T.decode(encoded)

let result = round_trip(User(name="Ada"))   # T inferred as User
```

Return-type context does not drive inference. Underconstrained calls require explicit type
arguments and are diagnosed as `FUSE_GENERIC_INFERENCE`.

### Constrained member resolution

When a type parameter `T` is constrained by `where T: I`, both instance and associated member
calls on `T` are resolved through the interface `I` at compile time:

```fuse
fn process<T>(x: T) where T: Encodable:
  let s = x.encode()        # instance call resolved through Encodable
  let v = T.decode(s)       # associated call resolved through Encodable
```

### Generic interface members and impl methods

Interface members and impl methods may be independently generic:

```fuse
interface Mappable:
  fn map<U>(f: fn(Self) -> U) -> U where U: Encodable

impl Mappable for User:
  fn map<U>(f: fn(Self) -> U) -> U where U: Encodable:
    return f(self)
```

An impl method must match the corresponding interface member's generic arity, parameter types,
return type, error type, and `where` constraints exactly after substituting `Self` with the
concrete target type. Mismatches are diagnosed as `FUSE_IMPL_SIGNATURE_MISMATCH`.

### Typed generic components

`component` declarations may carry type parameters and a trailing `where`:

```fuse
interface Renderable:
  fn render_text() -> String

component Button<T>(item: T) where T: Renderable:
  return button(attrs class="btn"):
    item.render_text()
```

The existing `component Layout:` form without explicit params remains valid. The new form adds
explicit params before the implicit `attrs` and `children`; the component model is otherwise
unchanged.

### Monomorphization

The frontend runs a monomorphization pass before interface desugaring and before
interpreter/native lowering. For every unique `(callable, concrete-type-args)` call site, the
pass emits a concrete copy of the callable with all type parameters substituted. Cross-module
generic calls are rewritten to canonical internal names of the form `m{id}::fn_TypeArg`.

Backends receive only concrete, non-generic call graphs. There is no runtime generic dispatch.

---

## Diagnostics

| Code | Trigger |
|---|---|
| `FUSE_GENERIC_DUPLICATE_TYPE_PARAM` | duplicate type parameter name in one declaration |
| `FUSE_GENERIC_CALL_TYPE_ARG` | wrong type-argument arity, or type args on a non-generic callable |
| `FUSE_GENERIC_INFERENCE` | generic call is underconstrained; explicit type arguments required |
| `FUSE_WHERE_UNKNOWN_INTERFACE` | `where` references an interface name not in scope |
| `FUSE_WHERE_MULTI_CONSTRAINT` | more than one interface constraint on one type parameter |

`FUSE_IMPL_SIGNATURE_MISMATCH` continues to cover generic impl/interface member incompatibility.

---

## Compatibility

All programs valid on `1.0.x` remain valid. `where` is contextual-only, so existing
query-builder uses of `.where(...)` are unaffected. The existing `component Ident:` form
remains valid.

Generic type parameters are not added to `type`, `enum`, or `interface` declarations in v1.1.
That surface, along with multiple interface constraints per type parameter, is deferred.

---

## Out of scope for v1.1

- Generic `type`, `enum`, and `interface` declarations
- Generic `impl` blocks
- Multiple interface constraints on one type parameter
- Return-type-based inference
- Generic `app`, `test`, and service-route declarations
- Runtime generic dispatch or trait-object behavior
