# v0.1 Feature Completion Plan

This plan covers the three currently deferred runtime capabilities:

1. non-range refinements (regex + custom predicates)
2. JSON decoding for `Result<T,E>`
3. SQLite connection pooling

The goal is to implement them before cutting `v0.1.0`, then remove the related "out-of-scope for `0.1.x`" wording from docs.

---

## Scope and constraints

- Keep AST/VM/native behavior aligned.
- Preserve existing syntax and runtime behavior unless explicitly extended.
- Land tests with each milestone; do not defer test coverage to the end.
- Keep release gates strict (`scripts/release_smoke.sh` must stay green).

---

## Milestone 0: Design freeze + baseline

Decision lock date: 2026-02-15

- [x] Refinement syntax/contract:
  - keep existing range shorthand (`String(1..80)`)
  - add call-style constraints in refined args:
    - `regex("<pattern>")` for string-like bases
    - `predicate(<fn_ident>)` where function signature is `fn(<base>) -> Bool`
  - mixed constraints are allowed and evaluated left-to-right
- [x] `Result<T,E>` JSON decode contract:
  - support tagged shape for request-body decode:
    - `{"type":"Ok","data":...}`
    - `{"type":"Err","data":...}`
  - invalid/missing tag is a validation error
- [x] DB pooling configuration contract:
  - `FUSE_DB_POOL_SIZE` controls pool size (default `1`)
  - optional config fallback: `App.dbPoolSize`
  - pool size must be integer `>= 1`; invalid values are runtime/config errors
- [x] Add focused baseline fixtures (ignored tests) that express target behavior and fail today.

Suggested files:

- `crates/fusec/tests/parser_fixtures.rs`
- `crates/fusec/tests/sema_golden.rs`
- `crates/fusec/tests/config_runtime.rs`
- `crates/fusec/tests/golden_outputs.rs`

---

## Milestone 1: Non-range refinements

### 1.1 Syntax and semantic model

Target supported forms (in addition to existing range shorthand):

- `String(regex("^[a-z0-9_-]+$"))`
- `String(1..80, regex("^[A-Z]"))`
- `Int(predicate(is_valid_age))`

Rules:

- keep `T(min..max)` as shorthand range constraint
- allow multiple constraints, evaluated left-to-right
- `predicate(name)` must reference `fn(<base>) -> Bool`

### 1.2 Compiler/sema work

- [x] Add a shared refinement constraint parser from `TypeRefKind::Refined.args` to a typed internal model.
- [x] Validate constraint/base-type compatibility in sema:
  - `regex(...)` only on string-like refined bases
  - `predicate(...)` function exists and signature matches base type
- [x] Improve diagnostics for invalid refinement expressions.

Primary files:

- `crates/fusec/src/sema/check.rs`
- `crates/fusec/src/sema/types.rs`
- new shared helper module (recommended): `crates/fusec/src/refinement.rs`

### 1.3 Runtime/backends work

- [x] Replace range-only refinement execution in all runtimes with shared constraint evaluation.
- [x] Keep existing range behavior unchanged.
- [x] Add regex engine dependency and cache compiled patterns per runtime instance.

Primary files:

- `crates/fusec/src/interp/mod.rs` (`check_refined`, parse range helpers)
- `crates/fusec/src/vm/mod.rs` (`check_refined`, parse range helpers)
- `crates/fusec/src/native/mod.rs` (both refinement validation sections)
- `crates/fusec/src/native/jit.rs` (refinement helpers)

### 1.4 OpenAPI updates

- [x] Map regex constraints to OpenAPI `pattern` where possible.
- [x] Preserve existing min/max mapping from range constraints.

Primary file:

- `crates/fusec/src/openapi.rs` (`refined_constraints`, `extract_range`)

### 1.5 Tests

- [x] Parser: new fixtures for regex/predicate refinement syntax.
- [x] Sema: golden diagnostics for invalid predicate signature/unknown predicate.
- [x] Runtime parity: AST/VM/native pass/fail cases for regex/predicate.

---

## Milestone 2: JSON decode for `Result<T,E>`

### 2.1 JSON contract

Support tagged JSON form:

```json
{ "type": "Ok", "data": ... }
{ "type": "Err", "data": ... }
```

Decode rules:

- `type = "Ok"` -> decode `data` as `T`
- `type = "Err"` -> decode `data` as `E`
- invalid/missing `type` -> validation error

Optional compatibility extension (decision at M0):

- allow untagged payload as implicit `Ok` when it decodes cleanly as `T`

### 2.2 Runtime/backends work

- [x] Implement `Result` decode path in all `decode_json_value` implementations.
- [x] Remove `"Result is not supported for JSON body"` branches.
- [x] Keep config/env/CLI `Result` parsing unchanged unless explicitly expanded in a follow-up.

Primary files:

- `crates/fusec/src/interp/mod.rs` (`decode_json_value`)
- `crates/fusec/src/vm/mod.rs` (`decode_json_value`)
- `crates/fusec/src/native/mod.rs` (both decode sections used by native runtime paths)

### 2.3 OpenAPI updates

- [x] For request-body `Result<T,E>` types, emit `oneOf` schema with tagged `Ok`/`Err` shape.
- [x] Keep response mapping behavior unchanged unless explicitly broadened.

Primary file:

- `crates/fusec/src/openapi.rs`

### 2.4 Tests

- [x] Add runtime tests for body decoding `Result<T,E>` on AST/VM/native.
- [x] Add malformed payload diagnostics tests (missing `type`, bad `data`, unknown variant).

---

## Milestone 3: DB connection pooling

### 3.1 Pool API and config

- [x] Implement and enforce pool-size configuration contract:
  - `FUSE_DB_POOL_SIZE` (default `1`)
  - optional config fallback: `App.dbPoolSize`
  - pool size must be integer `>= 1`; invalid values are runtime errors

Runtime knobs:

- `FUSE_DB_POOL_SIZE` (default `1`)
- optional config fallback: `App.dbPoolSize`

Target behavior:

- acquire/release pooled SQLite connections for DB builtins
- preserve transactional correctness for migrations/tests requiring same connection

### 3.2 DB layer refactor

- [x] Refactor DB layer to support pooled connections while preserving existing query API.
- [x] Add explicit transaction-scoped API to avoid `BEGIN`/`COMMIT` hopping across different pooled connections.

Primary file:

- `crates/fusec/src/db.rs`

### 3.3 Runtime integration

- [x] Replace single `Option<Db>` fields with pooled handle in:
  - AST runtime
  - VM runtime
  - native runtime / native heap accessors
- [x] Ensure migration execution uses transaction-scoped connection.

Primary files:

- `crates/fusec/src/interp/mod.rs` (`db_mut`, migration execution path)
- `crates/fusec/src/vm/mod.rs` (`db_mut`)
- `crates/fusec/src/native/value.rs` (`db_mut`)
- `crates/fusec/src/native/mod.rs`
- `crates/fusec/src/native/jit.rs` (host DB calls)

### 3.4 Tests

- [x] Add pool configuration tests (size parsing + defaults).
- [x] Add concurrency smoke test for parallel DB operations.
- [x] Add transaction integrity test for migrations.

---

## Milestone 4: Docs + release hardening

- [x] Update docs/specs to remove out-of-scope wording for completed features:
  - `README.md`
  - `runtime.md`
  - `scope.md` (updated boundary wording for pooled DB behavior)
  - `fls.md` (no additional changes required; existing sections already reflect implemented semantics)
- [x] Update `CHANGELOG.md` with concrete entries for all three features.
- [x] Keep release gate script aligned with added tests if new dedicated smoke tests are added.

---

## Test and release checklist for this plan

- [x] `scripts/cargo_env.sh cargo test -p fusec`
- [x] `scripts/cargo_env.sh cargo test -p fuse`
- [x] `scripts/release_smoke.sh`
- [x] verify new parser/sema/runtime coverage exists for each implemented feature

---

## Risk notes

- Refinements are currently duplicated across backend implementations; if code is not centralized first, parity bugs are likely.
- `native/mod.rs` contains multiple decode/validation paths; both must be updated for `Result` decode.
- DB pooling can silently break transaction semantics unless migration path pins a single connection for transaction scope.

---

## Recommended implementation order

1. Milestone 1 (refinement model + runtime parity)
2. Milestone 2 (`Result` JSON decode + OpenAPI update)
3. Milestone 3 (DB pooling + migration transaction safety)
4. Milestone 4 (docs/changelog/release hardening)

This order keeps language/runtime contract work ahead of infra refactors and minimizes simultaneous risk.
