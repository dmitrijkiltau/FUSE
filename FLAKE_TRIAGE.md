# Flake Triage Checklist

This checklist defines how to handle intermittent test/CI failures on release-blocking paths.

## Scope

Apply this process when failures are non-deterministic across reruns for:

- `scripts/authority_parity.sh`
- `scripts/lsp_suite.sh`
- benchmark gate path (`scripts/use_case_bench.sh` + `scripts/check_use_case_bench_regression.sh`)
- `scripts/release_smoke.sh`

## Ownership

1. Assign one owner within 1 business day.
2. Owner is responsible for root-cause tracking, mitigation, and closure.
3. Do not leave flaky failures unassigned.

## Reproduction Steps

1. Record failing workflow link, commit SHA, and exact failing step.
2. Re-run the failing command locally with the same environment variables when possible.
3. Run `scripts/reliability_repeat.sh --iterations 2` to check repeat stability.
4. Capture logs/artifacts needed to reproduce (stderr output, benchmark JSON, test output).

## Classification

Classify each flake as one of:

1. `infra` (runner/network/io/resource contention)
2. `harness` (timeout/retry/readiness logic)
3. `product` (real semantic/runtime bug)

## Mitigation Policy

1. Prefer deterministic fixes first:
   - bounded retry with explicit timeout
   - readiness probes
   - stable port/resource allocation
2. Avoid masking bugs:
   - retries must be limited and explicit
   - do not silently ignore failures
3. Add or update tests so the fix is enforced in CI.

## Temporary Waiver Policy

A temporary waiver is allowed only when all are true:

1. an owner and tracking issue exist,
2. root-cause hypothesis is documented,
3. a fix date (or milestone) is set,
4. the waiver has a hard expiry date.

Waivers must not be indefinite.

## Closure Criteria

A flake issue is closed only when:

1. fix is merged,
2. repeat-run gate has been green on main for multiple runs,
3. related docs/tests are updated.
