# AST Single-Source Authority Plan

## Goal

Make AST the single semantic authority for FUSE:

- Parser + frontend canonicalization define language behavior.
- VM and Native only execute canonical form.
- Backend-specific semantic interpretation is removed.

This plan assumes breaking changes are acceptable.

## Status

- M0 parity gates: completed.
- M1 frontend canonicalization: completed.
  - Added pass module and wired it into module loading and sema entrypoints.
  - Canonicalized valid HTML attribute shorthand to explicit positional args.
- M2 backend-local HTML shorthand semantics: completed.
  - Removed backend-local shorthand rewriting from AST interpreter and IR lowering.
  - Added shared shorthand diagnostics/validation helper:
    `crates/fusec/src/frontend/html_shorthand.rs`.
- M2.1 shared HTML tag builtin resolver: completed.
  - Added shared tag-resolution helper:
    `crates/fusec/src/frontend/html_tag_builtin.rs`.
  - Canonicalizer, sema, IR lower, and interpreter now delegate to that helper.
- M3 shared call binding + default semantics: completed.
  - Added shared binder module: `crates/fusec/src/callbind.rs`.
  - Interpreter call execution and named call APIs now use shared binding.
  - IR lowering now uses shared binding for local/imported function calls with defaults.
  - VM/native public call APIs use shared positional binding checks.
  - Added parity coverage for default evaluation order and imported defaults:
    `crates/fusec/tests/ast_authority_parity.rs`.
- M4 runtime type semantics: completed.
  - Added shared runtime-types module: `crates/fusec/src/runtime_types.rs`.
  - Centralized shared helpers used by AST/VM/Native/JIT:
    - `split_type_name`
    - `value_to_json` / `json_to_value`
    - `validation_error_value` / `validation_field_value`
  - Added shared host trait + canonical runtime semantics entrypoints:
    - `parse_env_value`
    - `decode_json_value`
    - `validate_value`
  - Interpreter, VM, Native VM, and ConfigEvaluator now delegate these paths to shared runtime-types logic.
  - Native JIT validation now delegates to shared runtime-types logic via a host adapter.

## Findings (Current Drift)

1. Canonical runtime semantics now run through shared `runtime_types`.
   - Parse/decode/validate behavior is now delegated by AST/VM/Native/ConfigEvaluator/JIT validation.
   - Risk of semantic drift in those paths is substantially reduced.

2. Cleanup follow-up remains.
   - Some backend-local helper methods are now dead code (legacy wrappers no longer on hot paths).
   - Recommended next pass:
     - remove unused legacy decode/validate/env helper methods from backends,
     - keep only wrappers + shared runtime-types authority.

## Target Architecture

```
Source
  -> Parse (raw AST)
  -> Frontend Canonicalization Passes (all desugaring + symbol-bound rewrites)
  -> Canonical AST (semantic source of truth)
  -> Sema checks
  -> Backend lowering/execution
       - AST interpreter executes canonical AST
       - VM lowers canonical AST to IR
       - Native compiles canonical AST/IR
```

Backend rule: no backend may reinterpret source sugar.

## Milestones

### M0: Parity Baseline Gates (Before Refactor)

Add tests that lock current intended semantics and expose drift:

- Config env override for `List`, `Map`, and user type JSON across `ast|vm|native`.
- HTML name shadowing cases (`config`, imported symbol, local binding) across backends.
- Public function extra-arg behavior parity for AST/VM/Native embedding APIs.
- HTML shorthand error parity (invalid attr value, mixed positional + shorthand).

Suggested file: `crates/fusec/tests/ast_authority_parity.rs`.

### M1: Frontend Canonicalization Pass

Introduce `crates/fusec/src/frontend/canonicalize.rs` and run it once after module load, before sema/backend execution.

Responsibilities:

- Canonicalize HTML attr shorthand into explicit attrs map.
- Canonicalize HTML children argument shape (including block sugar).
- Resolve whether callee is HTML tag via a single symbol-aware rule.
- Remove backend-local dependence on `CallArg.name` for HTML semantics.

### M2: Remove Backend-Local HTML Semantics

After M1, simplify:

- `sema/check.rs`: type-check canonical HTML call shape only.
- `ir/lower.rs`: lower canonical HTML call only; no shorthand logic.
- `interp/mod.rs`: remove `eval_html_tag_call_expr`; evaluate canonical calls through normal builtin path.

Keep one shared diagnostics text source for HTML sugar errors.

### M3: Centralize Call Binding and Default Argument Semantics

Add shared call binder (for example `crates/fusec/src/callbind.rs`) and use it in:

- AST interpreter call path.
- IR lowering for function calls/defaults.
- Public embedding API call helpers.

Single policy:

- Reject extra args.
- Reject missing required args.
- Evaluate defaults in declaration order with previous bound params visible.

### M4: Centralize Runtime Type Decode/Validate/JSON Semantics

Create shared runtime-type module (for example `crates/fusec/src/runtime_types.rs`) for:

- `parse_env_value`
- `decode_json_value` (including tagged `Result`)
- `validate_value`
- `value_to_json` / `json_to_value`
- validation error construction

Adopt in AST/VM/Native and ConfigEvaluator; keep native JIT hostcalls as wrappers to shared logic.

Unify config env policy across all backends (recommended: JSON decode for non-simple types everywhere).

### M5: Documentation and CI Gates

- Add a short architecture contract section in `fuse.md` and `runtime.md`.
- Extend smoke/parity jobs to include AST/VM/Native authority tests.
- Block merges on parity regressions for canonical semantics.

## Exit Criteria

- No backend-specific sugar lowering remains.
- One shared call-binding semantics implementation exists.
- One shared decode/validation semantics implementation exists.
- AST/VM/Native parity suite passes for canonical semantics.
