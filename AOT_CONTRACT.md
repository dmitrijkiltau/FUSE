# AOT Contract (Milestone 0)

Status: Accepted  
Scope: production AOT backend contract for the `v0.4.0` release line.

## Semantic authority contract

1. `fls.md` and `runtime.md` remain the semantic authority.
2. AOT is an execution strategy over canonical frontend artifacts.
3. Backend-specific reinterpretation of source semantics is a correctness bug.
4. VM/native/AOT semantic parity is a release gate for contract-facing behavior.

## Operational contract

1. `fuse build --aot` targets production deployment outputs.
2. JIT-native remains valid for fast local iteration and diagnostics.
3. AOT outputs must run without requiring a Rust toolchain on deployment hosts.
4. AOT release artifacts must be produced for:
   - `linux-x64`
   - `macos-arm64`
   - `windows-x64`

## SLO targets for v0.4.0

These targets are enforced as release-go/no-go criteria for the AOT rollout.

| Metric | Target | Measurement scope |
| --- | --- | --- |
| Cold start latency improvement vs JIT-native | >= 30% improvement at p50 and >= 20% at p95 | same host class, same app workload, same runtime env |
| AOT executable size | <= 25 MB (stripped) for reference `hello-http` class service | per target, release profile |
| AOT build time in CI | <= 10 minutes p95 per target job | clean CI runner, release build path |
| Semantic parity failures | 0 known unresolved contract-facing divergences | semantic/parity/release gates |

## Measurement protocol

1. Use fixed benchmark fixtures from the repo for every measurement run.
2. Record host class, target triple, git revision, and command line.
3. Compare against the same workload on the same host class.
4. Publish results in PR/release notes when targets change.
5. Treat target changes as contract updates requiring explicit maintainer approval.

## Ownership and change control

1. Changes to this file require maintainer approval.
2. Any threshold relaxation requires written rationale in the PR.
3. Contract updates must be reflected in:
   - `AOT_PRODUCTION_PLAN.md`
   - `RELEASE.md` (release gate checklist)
   - relevant RFC status/decision logs.
