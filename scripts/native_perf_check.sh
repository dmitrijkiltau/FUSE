#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cat <<'NOTE'
Running native perf smoke. Optional budgets:
  FUSE_NATIVE_COLD_MS=<ms>  # cold-start budget
  FUSE_NATIVE_WARM_MS=<ms>  # warm average budget (8 runs)

Example:
  FUSE_NATIVE_COLD_MS=800 FUSE_NATIVE_WARM_MS=200 scripts/native_perf_check.sh
NOTE

"$ROOT/scripts/cargo_env.sh" cargo test -p fusec --test native_bench_smoke -- --nocapture
