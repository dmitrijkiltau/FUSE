# AOT Rollback Playbook

Status: Active  
Scope: production incidents on the `v0.7.0` AOT rollout line.

## Document contract

- `Normative`: Yes for incident-response handling during the active AOT rollout line.
- `Front door`: No. Start onboarding from `../README.md`.
- `Owned concerns`: rollback triggers, rollback sequence, and recovery criteria for AOT incidents.
- `Conflict policy`: release contract thresholds defer to `AOT_RELEASE_CONTRACT.md`;
  language/runtime semantics defer to `../spec/fls.md` and `../spec/runtime.md`.

## Intent

Keep AOT as the default production backend while preserving a documented, tested rollback path
to JIT-native execution when an AOT-specific incident occurs.

## Required deploy assets

For each release, keep both:

1. `fuse-aot-<platform>.*` (primary production artifact)
2. `fuse-cli-<platform>.*` (fallback runtime artifact with `fuse` and `fuse-lsp`)

Do not delete fallback CLI artifacts until the release window closes.

## Incident classification

Treat as AOT-specific and eligible for fallback when at least one is true:

1. crash envelopes show `fatal: class=...` with reproducible AOT-only failure behavior
2. AOT startup/health check regression breaches SLO while JIT-native remains healthy
3. platform-specific linker/runtime issue impacts produced AOT binaries

## Rollback sequence

1. Immediate containment:
   - roll back to previous known-good AOT release artifact if available
2. Backend fallback:
   - redeploy using CLI artifact and run package via native backend:
     - `./scripts/fuse run --manifest-path <package-dir> --backend native`
3. Verify recovery:
   - health endpoints return success
   - latency/error rates normalize
   - no new fatal envelopes
4. Preserve diagnostics:
   - keep failing AOT binary
   - capture `FUSE_AOT_BUILD_INFO=1 <binary>` output
   - retain fatal logs with `pid` and build metadata

## Recovery back to AOT

1. reproduce and fix root cause
2. rerun:
   - `./scripts/aot_perf_bench.sh`
   - `./scripts/check_aot_perf_slo.sh`
   - `./scripts/release_smoke.sh`
3. deploy fixed AOT release and monitor

## Notes

1. JIT-native fallback is a temporary incident posture, not a policy change.
2. Any long-term fallback decision requires maintainer sign-off and release-policy update.
