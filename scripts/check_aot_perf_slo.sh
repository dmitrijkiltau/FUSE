#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
METRICS_JSON="$ROOT/.fuse/bench/aot_perf_metrics.json"
MIN_P50=30
MIN_P95=20

usage() {
  cat <<'EOF'
Usage: scripts/check_aot_perf_slo.sh [options]

Options:
  --metrics <path>  Path to AOT performance metrics JSON
  --min-p50 <pct>   Minimum required p50 cold-start improvement percentage (default: 30)
  --min-p95 <pct>   Minimum required p95 cold-start improvement percentage (default: 20)
  -h, --help        Show this help
EOF
}

abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --metrics)
      METRICS_JSON="${2:-}"
      if [[ -z "$METRICS_JSON" ]]; then
        echo "--metrics requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --min-p50)
      MIN_P50="${2:-}"
      if [[ -z "$MIN_P50" ]]; then
        echo "--min-p50 requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --min-p95)
      MIN_P95="${2:-}"
      if [[ -z "$MIN_P95" ]]; then
        echo "--min-p95 requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

METRICS_JSON="$(abspath "$METRICS_JSON")"

if [[ ! -f "$METRICS_JSON" ]]; then
  echo "missing AOT performance metrics file: $METRICS_JSON" >&2
  exit 1
fi

if ! [[ "$MIN_P50" =~ ^-?[0-9]+([.][0-9]+)?$ ]]; then
  echo "--min-p50 must be numeric" >&2
  exit 1
fi
if ! [[ "$MIN_P95" =~ ^-?[0-9]+([.][0-9]+)?$ ]]; then
  echo "--min-p95 must be numeric" >&2
  exit 1
fi

METRICS_JSON="$METRICS_JSON" MIN_P50="$MIN_P50" MIN_P95="$MIN_P95" node <<'NODE'
const fs = require("fs");

function readJson(path) {
  try {
    return JSON.parse(fs.readFileSync(path, "utf8"));
  } catch (err) {
    console.error(`failed to parse ${path}: ${err.message}`);
    process.exit(1);
  }
}

function readNumber(root, path) {
  let cursor = root;
  for (const part of path.split(".")) {
    if (!cursor || typeof cursor !== "object" || !(part in cursor)) {
      throw new Error(`missing field: ${path}`);
    }
    cursor = cursor[part];
  }
  const value = Number(cursor);
  if (!Number.isFinite(value)) {
    throw new Error(`invalid numeric field: ${path}=${cursor}`);
  }
  return value;
}

const metricsPath = process.env.METRICS_JSON;
const minP50 = Number(process.env.MIN_P50);
const minP95 = Number(process.env.MIN_P95);

const metrics = readJson(metricsPath);
if (Number(metrics.schema_version) !== 1) {
  console.error(`unsupported schema_version in ${metricsPath}: expected 1`);
  process.exit(1);
}

let startupP50;
let startupP95;
let startupP99;
let throughputP50;
let jitP50;
let aotP50;

try {
  startupP50 = readNumber(metrics, "cold_start_ms.improvement_pct.p50");
  startupP95 = readNumber(metrics, "cold_start_ms.improvement_pct.p95");
  startupP99 = readNumber(metrics, "cold_start_ms.improvement_pct.p99");
  throughputP50 = readNumber(metrics, "steady_state_cli_runs_per_sec.improvement_pct.p50");
  jitP50 = readNumber(metrics, "cold_start_ms.jit_native.p50");
  aotP50 = readNumber(metrics, "cold_start_ms.aot.p50");
} catch (err) {
  console.error(`invalid AOT performance metrics: ${err.message}`);
  process.exit(1);
}

console.log(
  `AOT cold-start improvement: p50=${startupP50.toFixed(3)}% p95=${startupP95.toFixed(3)}% p99=${startupP99.toFixed(3)}%`
);
console.log(
  `AOT throughput delta (CLI runs/s, p50): ${throughputP50.toFixed(3)}%`
);
console.log(
  `Cold-start p50 absolute: jit_native=${jitP50.toFixed(3)}ms aot=${aotP50.toFixed(3)}ms`
);

let failures = 0;
if (startupP50 < minP50) {
  failures += 1;
  console.error(
    `FAIL startup p50 improvement ${startupP50.toFixed(3)}% < required ${minP50.toFixed(3)}%`
  );
}
if (startupP95 < minP95) {
  failures += 1;
  console.error(
    `FAIL startup p95 improvement ${startupP95.toFixed(3)}% < required ${minP95.toFixed(3)}%`
  );
}

if (failures > 0) {
  console.error(`AOT performance SLO check failed with ${failures} issue(s)`);
  process.exit(1);
}

console.log("AOT performance SLO checks passed");
NODE
