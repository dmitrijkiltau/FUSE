# RFC: Interface Contracts Initial Slice

## Scope

This RFC records the initial v1.1 `interface` / `impl` slice implemented in the compiler and LSP.
It intentionally excludes `where` constraints and any generic interface-bound syntax.

## Accepted surface

- Top-level `interface Name:` declarations.
- Top-level `impl Interface for Type:` blocks.
- `Self` inside interface member signatures and impl member signatures/bodies.
- Import/export visibility for interface names alongside existing type/enum/function exports.

## Static rules

- Interfaces are compile-time contracts, not runtime values and not ordinary types.
- An impl must satisfy every required interface member for its `(interface, target)` pair.
- Impl signatures must match the interface member after substituting `Self` with the concrete
  target type.
- Duplicate impls for the same `(interface, target)` pair in one package are rejected.
- Orphan impls are rejected unless the current package owns the interface or the target type.

## Method model

- Instance members use an implicit immutable `self: ConcreteTarget`.
- Associated members do not receive `self`.
- Member resolution is closed-set and static over the visible impl set for the concrete receiver.

## Permanent exclusions

- Dynamic dispatch and runtime interface checks.
- Negative impl bounds.
- Blanket impls over parameterized types.
- Default interface method bodies.
- Interface inheritance/composition.

## Deferred work

- `where` clause syntax for interface-constrained generics.
- Additional LSP actions beyond the initial definition/symbol/diagnostic coverage.
