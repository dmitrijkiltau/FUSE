# FUSE Scope and Roadmap

This page sets expectations for FUSE users: what it targets now, and what is intentionally out of scope.

---

## What FUSE targets

FUSE is optimized for:

- CLI applications
- HTTP services
- contract-first development with typed boundaries

Primary developer benefits:

- validation from types
- first-class config loading
- consistent JSON and HTTP behavior
- generated OpenAPI from service/type declarations

See also: [Language Guide](fuse.md), [Runtime Guide](runtime.md).

---

## Current capability baseline

You can rely on:

- core language features (`fn`, `type`, `enum`, `match`, loops, imports)
- service/config/app declarations
- package workflow (`fuse check/run/dev/test/build`)
- AST, VM, and native backend path
- builtins for HTTP, HTML rendering, logging, assets, and SQLite access

See also: [Syntax and Types](fls.md), [Runtime Guide](runtime.md).

---

## Known constraints

Current limitations to account for:

- SQLite-only DB runtime
- no full ORM/query language
- task model is still evolving
- native backend remains under active iteration

Model your project around the stable language/runtime contracts first.

See also: [Builtins and Data Access](runtime.md#builtins-and-data-access), [Concurrency, Loops, and Logging](runtime.md#concurrency-loops-and-logging).

---

## Explicit non-goals

FUSE is not currently trying to be:

- a macro-heavy metaprogramming platform
- an "everything async by default" runtime
- a language with broad custom operator mechanics

Design direction favors clarity, explicitness, and predictable boundaries.

See also: [Language Guide](fuse.md), [Syntax and Types](fls.md).
