# Real-World Benchmarks and Use Cases

This document tracks benchmark workloads that represent how FUSE is expected to be used in practice.

Run the benchmark harness:

```bash
scripts/use_case_bench.sh
```

Outputs:

- `.fuse/bench/use_case_metrics.md`
- `.fuse/bench/use_case_metrics.json`

## Workload matrix

| Use case | Target project | Why this workload exists | Metrics collected |
| --- | --- | --- | --- |
| CLI tool with config + validation | `examples/project_demo.fuse` | Covers env-backed config, refined types, and runtime contract failures in a command-style app | `check` latency, run latency (valid), run latency (contract failure path) |
| Non-toy backend API package | `examples/notes-api` | Covers package check/migrate/run flow with HTTP routes, type-checked request/response boundaries, and DB usage | cold/warm check latency, migrate latency, API request latencies (when loopback is available) |
| Frontend client integration | `examples/notes-api` (`GET /` + API calls) | Covers serving static UI alongside API boundary validation behavior in one service | root document latency, valid/invalid JSON request latency, contract validation delta (when loopback is available) |

## Metric definitions

- Compile/check times:
  - first `check`: cold package semantic compile/check path
  - second `check`: warm package semantic compile/check path
- Boundary check times:
  - `POST /api/notes` with valid and invalid JSON bodies
  - invalid payload is expected to fail with `400`
- Contract enforcement cost:
  - absolute delta between invalid and valid `POST /api/notes` latency
  - used as a practical signal for validation/error-mapping overhead

## Notes

- Results are environment-dependent; compare trends across runs on the same machine.
- The harness intentionally checks status codes so failures are semantic regressions, not only performance noise.
- Runtime HTTP metrics are required for this benchmark; the script fails if the service cannot be started or reached on loopback.
- This benchmark set is complementary to semantic parity gates (`scripts/semantic_suite.sh`, `scripts/authority_parity.sh`).
