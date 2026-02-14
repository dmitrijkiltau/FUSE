# Limits + Roadmap

This guide sets expectations for teams adopting FUSE.

---

## 1) What FUSE is optimized for

Today FUSE targets:

- CLI applications
- HTTP services
- contract-first development with typed boundaries

Key value proposition:

- one set of types drives validation, JSON binding, and API behavior

---

## 2) Stable capability baseline

You can build production-style services with:

- core language features (`fn`, `type`, `enum`, `match`, loops, imports)
- typed `service`, `config`, and `app` declarations
- package tooling (`fuse check/run/dev/test/build`)
- AST, VM, and native backend path

---

## 3) Current constraints to plan around

Current constraints include:

- SQLite-focused DB runtime
- no full ORM/query abstraction layer
- evolving task/concurrency model
- native backend still under active iteration

Adopt with explicit boundaries and clear upgrade paths in mind.

---

## 4) Explicit non-goals

FUSE is not currently trying to be:

- a macro-heavy metaprogramming ecosystem
- an "everything async by default" runtime
- a language centered on custom operator mechanics

Direction is deliberately pragmatic: strict core, explicit contracts, predictable behavior.

Need implementation details before committing architecture decisions? Review [Build + Operate](runtime.md) and then run through [Start Here](fuse.md) with your target service shape.
