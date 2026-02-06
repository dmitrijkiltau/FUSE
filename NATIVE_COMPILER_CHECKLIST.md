# Native compiler/codegen checklist

Status legend: [x] done, [ ] pending, [~] partial

## Stage 0 — Foundation (done)

- [x] IR artifact cache (`.fuse/build/program.ir`)
- [x] Native image cache (`.fuse/build/program.native`)
- [x] `--backend native` plumbing in `fusec` and `fuse`
- [x] Native fast-path JIT for a minimal Int/Bool subset
- [x] Native benchmark smoke test
- [x] Single-pass build artifact generation

## Stage 1 — Numeric completeness (done)

- [x] Int arithmetic + comparisons + control flow
- [x] Int div/mod
- [x] Float arithmetic + comparisons + control flow (JIT fast path)
- [x] Numeric casts/coercions (int → float in arithmetic + comparisons)
- [x] Refined numeric types in native fast path (return types covered)
- [x] Parity tests for numeric examples under native

## Stage 2 — Value representation + heap types (done)

- [x] Define native value ABI (tagged layout + handle rules)
- [x] Native ABI wired into JIT boundary (numeric fast path)
- [x] Allocate/GC strategy for heap values (arena + mark/sweep over pinned roots)
- [x] Strings (native heap + round-trip encoding + JIT literals + interpolation)
- [x] JIT supports heap-tagged params/returns + string literals
- [x] Lists (native heap + round-trip encoding + JIT MakeList)
- [x] Maps (native heap + round-trip encoding + JIT MakeMap)
- [x] Structs (heap representation + JIT MakeStruct/GetField for heap fields)
- [x] Enums (heap representation + JIT MakeEnum)
- [x] Boxed/shared values (heap representation + JIT MakeBox)
- [x] Task values (heap representation + ABI round-trip)

## Stage 3 — Option/Result + error model (done)

- [x] Option representation + `null` handling (native ABI + JIT null constants/JumpIfNull)
- [x] Result representation (`T!E`, `?!`) (native ABI + `?!` in JIT)
- [x] Error propagation + runtime error mapping (JIT runtime errors + `?!` errors)
- [x] Golden error output parity under native

## Stage 4 — Builtins + runtime interop

- [ ] `log`, `env`, `assert`
- [ ] `db.exec/query/one`
- [ ] JSON encode/decode
- [ ] Validation hooks (refinements, Email/Id)
- [ ] HTTP routing + handler calls
- [ ] Config loading
- [ ] Task APIs (`task.*`)

## Stage 5 — De‑VM

- [ ] Native-only execution for supported feature set (no VM fallback)
- [ ] Explicit unsupported-feature errors for native
- [ ] Full example parity suite on native

## Stage 6 — AOT artifacts (optional)

- [ ] Object emission (cranelift-object)
- [ ] Link step + standalone native binary
- [ ] Persistent native cache versioning
- [ ] Cold-start perf regression checks
