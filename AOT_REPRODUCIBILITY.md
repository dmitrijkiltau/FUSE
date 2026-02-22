# AOT Reproducibility Policy

Status: Active  
Scope: `fuse build --aot` release artifacts (`v0.4.0` rollout line)

## Toolchain inputs

1. Rust toolchain policy is sourced from `rust-toolchain.toml` (`stable`, `minimal`, `rustfmt`, `clippy`).
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

