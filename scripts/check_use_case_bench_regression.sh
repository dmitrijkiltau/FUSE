#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CURRENT_JSON="$ROOT/.fuse/bench/use_case_metrics.json"
BASELINE_JSON="$ROOT/benchmarks/use_case_baseline.json"

usage() {
  cat <<'EOF'
Usage: scripts/check_use_case_bench_regression.sh [options]

Options:
  --current <path>   Path to current benchmark metrics JSON
  --baseline <path>  Path to baseline benchmark JSON
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --current)
      CURRENT_JSON="${2:-}"
      if [[ -z "$CURRENT_JSON" ]]; then
        echo "--current requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --baseline)
      BASELINE_JSON="${2:-}"
      if [[ -z "$BASELINE_JSON" ]]; then
        echo "--baseline requires a value" >&2
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

if [[ ! -f "$CURRENT_JSON" ]]; then
  echo "missing current benchmark metrics file: $CURRENT_JSON" >&2
  exit 1
fi

if [[ ! -f "$BASELINE_JSON" ]]; then
  echo "missing baseline benchmark file: $BASELINE_JSON" >&2
  exit 1
fi

CURRENT_JSON="$CURRENT_JSON" BASELINE_JSON="$BASELINE_JSON" node <<'NODE'
const fs = require("fs");

function readJson(path) {
  try {
    return JSON.parse(fs.readFileSync(path, "utf8"));
  } catch (err) {
    console.error(`failed to parse ${path}: ${err.message}`);
    process.exit(1);
  }
}

function flattenMetrics(src, prefix = "", out = {}) {
  for (const [key, value] of Object.entries(src)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value === null || value === undefined) {
      continue;
    }
    if (typeof value === "number") {
      out[path] = value;
      continue;
    }
    if (Array.isArray(value)) {
      continue;
    }
    if (typeof value === "object") {
      flattenMetrics(value, path, out);
    }
  }
  return out;
}

function isIsoUtcTimestamp(value) {
  if (typeof value !== "string") {
    return false;
  }
  return /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(value);
}

const currentPath = process.env.CURRENT_JSON;
const baselinePath = process.env.BASELINE_JSON;

const currentRaw = readJson(currentPath);
const baselineRaw = readJson(baselinePath);

if (!baselineRaw.metrics || typeof baselineRaw.metrics !== "object") {
  console.error(`invalid baseline format in ${baselinePath}: missing object field "metrics"`);
  process.exit(1);
}

const baselineSchemaVersion = Number(baselineRaw.schema_version);
if (!Number.isInteger(baselineSchemaVersion) || baselineSchemaVersion < 1) {
  console.error(`invalid baseline format in ${baselinePath}: schema_version must be an integer >= 1`);
  process.exit(1);
}

if (!isIsoUtcTimestamp(baselineRaw.generated_utc)) {
  console.error(
    `invalid baseline format in ${baselinePath}: generated_utc must be an ISO-8601 UTC timestamp (YYYY-MM-DDTHH:MM:SSZ)`
  );
  process.exit(1);
}

if (typeof baselineRaw.refresh_rationale !== "string" || baselineRaw.refresh_rationale.trim().length < 12) {
  console.error(
    `invalid baseline format in ${baselinePath}: refresh_rationale must be a non-empty rationale note (>= 12 chars)`
  );
  process.exit(1);
}

function benchmarkProfile(current) {
  const env = current?.benchmark_context?.environment ?? {};
  const osRelease = String(env.os_release ?? "");
  const isCi = Boolean(env.ci);
  if (!isCi && /microsoft-standard-wsl2/i.test(osRelease)) {
    return "local_wsl2";
  }
  return "default";
}

function profileMetricLimitFloors(profile) {
  if (profile !== "local_wsl2") {
    return {};
  }
  return {
    // WSL2 host loopback/network stack often adds ~20-25ms request latency.
    // Keep CI/default thresholds unchanged; apply local floor only for these host-class paths.
    "reference_service.request_get_notes_ms": 30.0,
    "reference_service.request_post_invalid_ms": 30.0,
    "reference_service.request_frontend_root_ms": 30.0
  };
}

const current = flattenMetrics(currentRaw);
const profile = benchmarkProfile(currentRaw);
const metricLimitFloors = profileMetricLimitFloors(profile);
let failures = 0;
const lines = [];
lines.push(`benchmark regression profile: ${profile}`);

for (const [metric, rule] of Object.entries(baselineRaw.metrics)) {
  if (!rule || typeof rule !== "object") {
    console.error(`invalid baseline rule for ${metric}: expected object`);
    process.exit(1);
  }
  const baselineMs = Number(rule.baseline_ms);
  const maxPct = Number(rule.max_regression_pct ?? 0);
  const maxMs = Number(rule.max_regression_ms ?? 0);
  if (!Number.isFinite(baselineMs) || baselineMs < 0) {
    console.error(`invalid baseline_ms for ${metric}`);
    process.exit(1);
  }
  if (!Number.isFinite(maxPct) || maxPct < 0) {
    console.error(`invalid max_regression_pct for ${metric}`);
    process.exit(1);
  }
  if (!Number.isFinite(maxMs) || maxMs < 0) {
    console.error(`invalid max_regression_ms for ${metric}`);
    process.exit(1);
  }

  if (!(metric in current)) {
    failures += 1;
    lines.push(`FAIL ${metric}: missing from current metrics`);
    continue;
  }

  const observed = Number(current[metric]);
  if (!Number.isFinite(observed) || observed < 0) {
    failures += 1;
    lines.push(`FAIL ${metric}: invalid observed value ${current[metric]}`);
    continue;
  }

  const allowance = Math.max(baselineMs * maxPct, maxMs);
  const limit = baselineMs + allowance;
  const floor = Number(metricLimitFloors[metric] ?? Number.NaN);
  const effectiveLimit = Number.isFinite(floor) ? Math.max(limit, floor) : limit;
  const status = observed <= effectiveLimit ? "PASS" : "FAIL";
  const line =
    `${status} ${metric}: observed=${observed.toFixed(3)}ms ` +
    `baseline=${baselineMs.toFixed(3)}ms limit=${effectiveLimit.toFixed(3)}ms` +
    (Number.isFinite(floor) ? ` (profile_floor=${floor.toFixed(3)}ms)` : "");
  lines.push(line);
  if (status === "FAIL") {
    failures += 1;
  }
}

for (const line of lines) {
  console.log(line);
}

if (failures > 0) {
  console.error(`benchmark regression check failed with ${failures} issue(s)`);
  process.exit(1);
}

console.log("benchmark regression checks passed");
NODE
