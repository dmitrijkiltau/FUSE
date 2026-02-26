# AOT Release Contract

Status: Accepted
Scope: production AOT backend and release reproducibility contract for the active `v0.4.x` line.

## Document contract

- `Normative`: Yes.
- `Front door`: No. Start onboarding from `../README.md`.
- `Owned concerns`: AOT semantic authority expectations, production release contract, SLO gates,
  deterministic metadata policy, and static-profile constraints.
- `Conflict policy`: language/runtime semantics defer to `../spec/fls.md` and `../spec/runtime.md`;
  incident response steps defer to `AOT_ROLLBACK_PLAYBOOK.md`.

## Semantic authority contract

1. `../spec/fls.md` and `../spec/runtime.md` remain the semantic authority.
2. AOT is an execution strategy over canonical frontend artifacts.
3. Backend-specific reinterpretation of source semantics is a correctness bug.
4. Native/AOT semantic parity is a release gate for contract-facing behavior.

## Operational contract

1. `fuse build --aot` targets production deployment outputs.
2. JIT-native remains valid for fast local iteration and diagnostics.
3. AOT outputs must run without requiring a Rust toolchain on deployment hosts.
4. AOT release artifacts must be produced for:
   - `linux-x64`
   - `macos-arm64`
   - `windows-x64`
5. AOT binaries must embed build metadata (`mode`, `profile`, `target`, `rustc`, `cli`,
   `runtime_cache`, `contract`), expose build metadata (`FUSE_AOT_BUILD_INFO=1`), support startup
   trace diagnostics (`FUSE_AOT_STARTUP_TRACE=1`), and emit stable fatal envelopes
   (`runtime_fatal` / `panic`) including `pid` and build metadata.

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

## Toolchain inputs

1. Rust toolchain policy is sourced from `rust-toolchain.toml` (`stable`, `minimal`, `rustfmt`,
   `clippy`).
2. Release artifact workflow uses Node.js `24` for VSIX packaging parity.
3. Release metadata generation uses `SOURCE_DATE_EPOCH` pinned to the release commit timestamp.

## Deterministic metadata contract

1. Artifact discovery for checksums/metadata is type-scoped and name-sorted.
2. `scripts/generate_release_checksums.sh` emits:
   - deterministic artifact ordering
   - stable `generatedAtUtc` when `SOURCE_DATE_EPOCH` is set
   - optional `sourceDateEpoch` in `release-artifacts.json` for traceability
3. Release workflows must set `SOURCE_DATE_EPOCH` when generating publishable metadata.

## Known non-determinism sources

1. Rust/LLVM codegen changes when the upstream stable toolchain revs.
2. Linker/build-id behavior that differs across host toolchains.
3. Archive timestamp/compression variance when packaging flags or archiver implementations differ.
4. Platform ABI/runtime differences (`linux-x64` vs `macos-arm64` vs `windows-x64`).

## Mitigations

1. Build each target on native CI runners in the official matrix.
2. Keep artifact naming fixed (`fuse-cli-*`, `fuse-aot-*`, `fuse-vscode-*`).
3. Verify archive integrity for CLI/AOT/VSIX payloads before publish.
4. Publish SHA-256 checksums + metadata manifest for every release bundle.
5. Evaluate reproducibility per target class; do not compare checksums across unlike targets.

## Static profile policy

1. Published AOT artifacts use the default `release` profile.
2. Optional static linking is platform-constrained:
   - candidate: `linux-x64` only (toolchain/target dependent)
   - not supported for release contract: `macos-arm64`, `windows-x64`
3. Static profile outputs are non-default and must be documented per release if enabled.

## Ownership and change control

1. Changes to this file require maintainer approval.
2. Any threshold relaxation requires written rationale in the PR.
3. Contract updates must be reflected in:
   - `RELEASE.md` (release gate checklist)
   - relevant RFC status/decision logs.
