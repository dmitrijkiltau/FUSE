# RFC 0007: VM Backend Deprecation

- Status: Implemented
- Authors: FUSE maintainers
- Created: 2026-02-26
- Updated: 2026-02-26
- Related PRs: TBD
- Related issues: VM_ANALYSIS.md, EXTENSIBILITY_BOUNDARIES.md §2

## Summary

Deprecate the VM bytecode backend (`--backend vm`) and establish a concrete removal timeline.
The public backend set changes from `ast | vm | native` to `ast | native`, with VM retained
as an internal, unsupported execution path during the deprecation window before full removal.

## Motivation

The VM backend is a structural duplicate of the Native backend. Both consume the same
`IrProgram` produced by `ir/lower.rs`; the only difference is that VM interprets IR
instructions in a loop (~3,013 LoC in `vm/mod.rs`) while Native JIT-compiles them via
Cranelift (~10,019 LoC). The VM provides no unique capability that Native does not also
provide.

Maintaining the VM imposes concrete costs:

1. **Triple implementation burden.** Every new runtime feature (builtin, hostcall, boundary
   behavior) must be implemented in AST, VM, *and* Native. Removing VM reduces this to two.
2. **Parity test surface.** The `authority_parity.sh` gate enforces three-way parity. With
   VM gone, parity narrows to AST vs Native — the only pair that matters for production.
3. **Default backend already changed.** As of the current release, `fuse run` defaults to
   Native (previously VM). The cached-run path tries `program.native` first, falling back
   to `program.ir` + VM only when no native artifact exists. VM is no longer on the primary
   execution path.
4. **AOT trajectory.** RFCs 0001–0006 establish Native/AOT as the production execution
   strategy. VM has no role in the AOT pipeline; it is an intermediate artifact from the
   pre-Native era.
5. **`--test` and `--migrate` are AST-only.** The two commands that bypass Native also
   bypass VM — they are hardcoded to the AST interpreter. VM has no exclusive operational
   role.

## Non-goals

- Removing the AST interpreter. AST remains the semantic authority and the backend for
  `--test` and `--migrate`.
- Removing the IR lowering layer (`ir/lower.rs`). Native depends on it.
- Changing language or runtime semantics. This RFC is purely about execution strategy.
- Removing `--backend vm` immediately. This RFC defines a deprecation window.

## Detailed design

### Phase 1: Deprecation (current release line, `0.4.x`)

1. **CLI deprecation diagnostic.** When `--backend vm` is explicitly passed, emit a
   `warning: the VM backend is deprecated and will be removed in a future release; use
   --backend native (default) or --backend ast` diagnostic to stderr before execution.
   The VM still executes normally.
2. **Documentation.** Mark VM as deprecated in:
   - `governance/EXTENSIBILITY_BOUNDARIES.md` §2 (backend set line)
   - `spec/runtime.md` (backend table)
   - `guides/fuse.md` (backend reference)
   - `README.md` (if backend list is mentioned)
3. **Governance update.** Change the fixed backend set from `ast | vm | native` to
   `ast | native` with a note that `vm` is deprecated and retained only for the
   deprecation window.
4. **Parity tests.** VM is already removed from the parity test matrix (step 3 of
   VM_ANALYSIS.md). No further test changes needed in this phase.
5. **Benchmark scripts.** `use_case_bench.sh` already uses `--backend vm` only for
   `project_demo` CLI metrics. Change to `--backend native` (or omit, using the default).

### Phase 2: Removal (next breaking minor, `0.5.0`)

1. **Delete `crates/fusec/src/vm/`** (~3,013 LoC).
2. **Remove `Backend::Vm` variant** from `cli.rs` and `main.rs`.
3. **Remove `--backend vm` CLI flag.** Passing it becomes an "unknown backend" error.
4. **Remove `run_vm_ir` fallback** from `main.rs` cached-run path.
5. **Remove VM-related IR cache paths** if no longer needed (Native has its own
   `program.native` artifact; `program.ir` may be retained for Native's consumption
   or removed if Native no longer uses it).
6. **Update `EXTENSIBILITY_BOUNDARIES.md`** to remove the deprecation note and state
   the fixed set as `ast | native`.
7. **CHANGELOG entry** with explicit breaking note and migration guidance.

### Migration path

Users explicitly passing `--backend vm`:
- **`--backend vm` → remove the flag** (default is already Native).
- **`--backend vm` → `--backend ast`** if they specifically need the AST interpreter.
- No source code changes are required. VM and Native produce identical output for all
  programs (enforced by parity tests).

Scripts or CI pipelines referencing `--backend vm`:
- Remove the flag or replace with `--backend native`.

## Alternatives considered

### 1. Keep VM indefinitely as an internal debugging tool

Rejected. The debugging value of VM is marginal — if a bug exists in IR lowering, it
manifests identically in VM and Native (they share the same IR). If a bug is in JIT
codegen, VM cannot reproduce it. AST vs Native comparison is sufficient for diagnosing
semantic issues.

### 2. Remove VM immediately without a deprecation window

Rejected. `VERSIONING_POLICY.md` requires deprecation notes and migration guidance for
contract-facing changes. `--backend vm` is a documented CLI flag, so removal requires
at least one release with a deprecation diagnostic.

### 3. Keep VM but stop maintaining parity

Rejected. A backend that silently drifts from AST/Native semantics is worse than no
backend — it produces wrong results without warning. Either maintain full parity or remove.

## Compatibility and migration

- **Phase 1 is backward compatible.** `--backend vm` continues to work; a warning is
  added but behavior is unchanged.
- **Phase 2 is a breaking change** gated on the next minor version bump (`0.5.0`).
  Migration is trivial: remove `--backend vm` from CLI invocations.
- No source-level migration is required. Programs are backend-agnostic.

## Test plan

### Phase 1

- Add a CLI test verifying `--backend vm` emits the deprecation warning.
- Existing parity tests (AST vs Native) remain the gate. No VM-specific test changes.

### Phase 2

- Remove any remaining VM-specific test files.
- Verify `--backend vm` produces an "unknown backend" error.
- Full release gate (`release_smoke.sh`) must pass.

## Documentation updates

### Phase 1

- `governance/EXTENSIBILITY_BOUNDARIES.md`: update §2 backend set line
- `spec/runtime.md`: mark VM deprecated in backend table
- `guides/fuse.md`: mark VM deprecated
- `CHANGELOG.md`: deprecation entry
- `README.md`: update if backend list is referenced

### Phase 2

- All of the above: remove VM references entirely
- `governance/VERSIONING_POLICY.md`: no change needed (process was followed)

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| External scripts depend on `--backend vm` | Low | Deprecation warning gives one release cycle notice |
| VM removal breaks cached `program.ir` workflows | Low | Native already consumes IR; `program.ir` path remains if needed |
| Debugging regressions harder without VM | Low | AST vs Native comparison is sufficient; VM shares the same IR as Native |

## Rollout plan

1. **`0.4.x` (Phase 1):** Merge deprecation diagnostic and governance/doc updates.
   Completion criteria: `--backend vm` warns, all release gates pass.
2. **`0.5.0` (Phase 2):** Delete VM code, remove CLI flag.
   Completion criteria: `vm/` directory gone, `--backend vm` errors, all release gates pass.

## Decision log

- 2026-02-26: Proposed
