#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_SRC="$ROOT/examples/project_demo.fuse"
OUT_DIR="$ROOT/.fuse/bench"
METRICS_JSON="$OUT_DIR/aot_perf_metrics.json"
METRICS_MD="$OUT_DIR/aot_perf_metrics.md"
mkdir -p "$ROOT/tmp"
TMP_ROOT="$(mktemp -d "$ROOT/tmp/fuse_aot_perf_bench.XXXXXX")"

SAMPLES=7
BURST_RUNS=15

declare -a jit_startup_samples=()
declare -a aot_startup_samples=()
declare -a jit_rps_samples=()
declare -a aot_rps_samples=()

usage() {
  cat <<'USAGE'
Usage: scripts/aot_perf_bench.sh [options]

Options:
  --samples <n>      Cold-start samples per backend (default: 7)
  --burst-runs <n>   Sequential runs per throughput sample (default: 15)
  --fixture <path>   Fixture source file (default: examples/project_demo.fuse)
  --out-dir <path>   Output directory (default: .fuse/bench)
  -h, --help         Show this help
USAGE
}

abspath() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

cleanup() {
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

now_ns() {
  date +%s%N
}

percentile_from_samples() {
  local pct="$1"
  shift
  local -a values=("$@")
  if [[ "${#values[@]}" -eq 0 ]]; then
    printf "0.000"
    return 0
  fi
  local -a sorted=()
  while IFS= read -r value; do
    sorted+=("$value")
  done < <(printf '%s\n' "${values[@]}" | sort -n)
  local count="${#sorted[@]}"
  local rank
  rank="$(awk -v n="$count" -v p="$pct" 'BEGIN {
    if (n <= 1) { print 1; exit; }
    r = (p / 100.0) * n;
    idx = int(r);
    if (r > idx) idx += 1;
    if (idx < 1) idx = 1;
    if (idx > n) idx = n;
    print idx;
  }')"
  local idx=$((rank - 1))
  printf '%s' "${sorted[$idx]}"
}

ms_to_rps() {
  local runs="$1"
  local ms="$2"
  awk -v n="$runs" -v elapsed_ms="$ms" 'BEGIN {
    if (elapsed_ms <= 0) {
      printf "0.000";
    } else {
      printf "%.3f", (n * 1000.0) / elapsed_ms;
    }
  }'
}

pct_drop() {
  local base="$1"
  local candidate="$2"
  awk -v base="$base" -v candidate="$candidate" 'BEGIN {
    if (base <= 0) {
      printf "0.000";
    } else {
      printf "%.3f", ((base - candidate) / base) * 100.0;
    }
  }'
}

pct_gain() {
  local base="$1"
  local candidate="$2"
  awk -v base="$base" -v candidate="$candidate" 'BEGIN {
    if (base <= 0) {
      printf "0.000";
    } else {
      printf "%.3f", ((candidate - base) / base) * 100.0;
    }
  }'
}

json_escape() {
  printf '%s' "$1" | tr '\n\r\t' '   ' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

json_number_array() {
  local -a values=("$@")
  if [[ "${#values[@]}" -eq 0 ]]; then
    printf "[]"
    return 0
  fi
  local first=1
  printf "["
  local value
  for value in "${values[@]}"; do
    if [[ "$first" -eq 0 ]]; then
      printf ", "
    fi
    printf "%s" "$value"
    first=0
  done
  printf "]"
}

collect_environment_metadata() {
  BENCH_OS_NAME="$(uname -s 2>/dev/null || echo "unknown")"
  BENCH_OS_RELEASE="$(uname -r 2>/dev/null || echo "unknown")"
  BENCH_OS_ARCH="$(uname -m 2>/dev/null || echo "unknown")"
  BENCH_CPU_MODEL="unknown"
  if command -v lscpu >/dev/null 2>&1; then
    BENCH_CPU_MODEL="$(lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p' | head -n1)"
  elif [[ -f /proc/cpuinfo ]]; then
    BENCH_CPU_MODEL="$(sed -n 's/^model name[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | head -n1)"
  fi
  if [[ -z "$BENCH_CPU_MODEL" ]]; then
    BENCH_CPU_MODEL="unknown"
  fi
  BENCH_CPU_COUNT="$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)"
  if [[ -z "$BENCH_CPU_COUNT" ]] || ! [[ "$BENCH_CPU_COUNT" =~ ^[0-9]+$ ]]; then
    BENCH_CPU_COUNT=1
  fi
}

measure_cmd_ms() {
  local __out_var="$1"
  shift
  local start_ns end_ns log_file
  log_file="$(mktemp "$TMP_ROOT/cmd.XXXXXX.log")"
  start_ns="$(now_ns)"
  if ! "$@" >"$log_file" 2>&1; then
    cat "$log_file" >&2
    rm -f "$log_file"
    return 1
  fi
  end_ns="$(now_ns)"
  rm -f "$log_file"
  printf -v "$__out_var" "%s" "$(awk -v ns="$((end_ns - start_ns))" 'BEGIN { printf "%.3f", (ns / 1000000) }')"
}

measure_burst_rps() {
  local __out_var="$1"
  local runs="$2"
  shift 2
  local start_ns end_ns elapsed_ms
  local log_file
  log_file="$(mktemp "$TMP_ROOT/burst.XXXXXX.log")"

  # Warm command once before burst collection.
  if ! "$@" >"$log_file" 2>&1; then
    cat "$log_file" >&2
    rm -f "$log_file"
    return 1
  fi

  start_ns="$(now_ns)"
  for ((run = 1; run <= runs; run++)); do
    if ! "$@" >"$log_file" 2>&1; then
      cat "$log_file" >&2
      rm -f "$log_file"
      return 1
    fi
  done
  end_ns="$(now_ns)"
  rm -f "$log_file"
  elapsed_ms="$(awk -v ns="$((end_ns - start_ns))" 'BEGIN { printf "%.3f", (ns / 1000000) }')"
  printf -v "$__out_var" "%s" "$(ms_to_rps "$runs" "$elapsed_ms")"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --samples)
      SAMPLES="${2:-}"
      if [[ -z "$SAMPLES" ]]; then
        echo "--samples requires a value" >&2
        exit 1
      fi
      if ! [[ "$SAMPLES" =~ ^[0-9]+$ ]] || [[ "$SAMPLES" -lt 3 ]]; then
        echo "--samples must be an integer >= 3" >&2
        exit 1
      fi
      shift 2
      ;;
    --burst-runs)
      BURST_RUNS="${2:-}"
      if [[ -z "$BURST_RUNS" ]]; then
        echo "--burst-runs requires a value" >&2
        exit 1
      fi
      if ! [[ "$BURST_RUNS" =~ ^[0-9]+$ ]] || [[ "$BURST_RUNS" -lt 1 ]]; then
        echo "--burst-runs must be a positive integer" >&2
        exit 1
      fi
      shift 2
      ;;
    --fixture)
      FIXTURE_SRC="${2:-}"
      if [[ -z "$FIXTURE_SRC" ]]; then
        echo "--fixture requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      if [[ -z "$OUT_DIR" ]]; then
        echo "--out-dir requires a value" >&2
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

FIXTURE_SRC="$(abspath "$FIXTURE_SRC")"
if [[ ! -f "$FIXTURE_SRC" ]]; then
  echo "fixture source not found: $FIXTURE_SRC" >&2
  exit 1
fi
OUT_DIR="$(abspath "$OUT_DIR")"
METRICS_JSON="$OUT_DIR/aot_perf_metrics.json"
METRICS_MD="$OUT_DIR/aot_perf_metrics.md"

FIXTURE_DIR="$TMP_ROOT/project-demo-fixture"
mkdir -p "$FIXTURE_DIR"
cp "$FIXTURE_SRC" "$FIXTURE_DIR/main.fuse"
cat >"$FIXTURE_DIR/fuse.toml" <<'EOF'
[package]
entry = "main.fuse"
app = "demo"
backend = "native"
EOF

echo "Preparing AOT performance benchmark harness..."
echo "Fixture source: $FIXTURE_SRC"
echo "Samples per backend: $SAMPLES"
echo "Throughput burst runs/sample: $BURST_RUNS"

mkdir -p "$OUT_DIR"

echo "Building AOT binary fixture (release)..."
"$ROOT/scripts/fuse" build --manifest-path "$FIXTURE_DIR" --aot --release >/dev/null
AOT_BIN="$FIXTURE_DIR/.fuse/build/program.aot"
if [[ ! -f "$AOT_BIN" && -f "${AOT_BIN}.exe" ]]; then
  AOT_BIN="${AOT_BIN}.exe"
fi
if [[ ! -f "$AOT_BIN" ]]; then
  echo "missing AOT binary: $AOT_BIN" >&2
  exit 1
fi

if ! AOT_BUILD_INFO="$(FUSE_AOT_BUILD_INFO=1 "$AOT_BIN" 2>/dev/null)"; then
  echo "failed to read AOT build metadata from $AOT_BIN" >&2
  exit 1
fi

AOT_RUN_BIN="$TMP_ROOT/aot-project-demo"
if [[ "$AOT_BIN" == *.exe ]]; then
  AOT_RUN_BIN="${AOT_RUN_BIN}.exe"
fi
cp "$AOT_BIN" "$AOT_RUN_BIN"
chmod +x "$AOT_RUN_BIN"

echo "Running cold-start + throughput samples..."
for ((sample = 1; sample <= SAMPLES; sample++)); do
  echo "Sample $sample/$SAMPLES"
  rm -rf "$FIXTURE_DIR/.fuse/build"

  jit_startup_ms=""
  measure_cmd_ms \
    jit_startup_ms \
    env APP_GREETING="Hello" APP_WHO="Bench" \
    "$ROOT/scripts/fuse" run --manifest-path "$FIXTURE_DIR" --backend native
  jit_startup_samples+=("$jit_startup_ms")

  aot_startup_ms=""
  measure_cmd_ms \
    aot_startup_ms \
    env APP_GREETING="Hello" APP_WHO="Bench" \
    "$AOT_RUN_BIN"
  aot_startup_samples+=("$aot_startup_ms")

  jit_rps=""
  measure_burst_rps \
    jit_rps \
    "$BURST_RUNS" \
    env APP_GREETING="Hello" APP_WHO="Bench" \
    "$ROOT/scripts/fuse" run --manifest-path "$FIXTURE_DIR" --backend native
  jit_rps_samples+=("$jit_rps")

  aot_rps=""
  measure_burst_rps \
    aot_rps \
    "$BURST_RUNS" \
    env APP_GREETING="Hello" APP_WHO="Bench" \
    "$AOT_RUN_BIN"
  aot_rps_samples+=("$aot_rps")
done

jit_startup_p50="$(percentile_from_samples 50 "${jit_startup_samples[@]}")"
jit_startup_p95="$(percentile_from_samples 95 "${jit_startup_samples[@]}")"
jit_startup_p99="$(percentile_from_samples 99 "${jit_startup_samples[@]}")"
aot_startup_p50="$(percentile_from_samples 50 "${aot_startup_samples[@]}")"
aot_startup_p95="$(percentile_from_samples 95 "${aot_startup_samples[@]}")"
aot_startup_p99="$(percentile_from_samples 99 "${aot_startup_samples[@]}")"

jit_rps_p50="$(percentile_from_samples 50 "${jit_rps_samples[@]}")"
jit_rps_p95="$(percentile_from_samples 95 "${jit_rps_samples[@]}")"
jit_rps_p99="$(percentile_from_samples 99 "${jit_rps_samples[@]}")"
aot_rps_p50="$(percentile_from_samples 50 "${aot_rps_samples[@]}")"
aot_rps_p95="$(percentile_from_samples 95 "${aot_rps_samples[@]}")"
aot_rps_p99="$(percentile_from_samples 99 "${aot_rps_samples[@]}")"

startup_improvement_p50_pct="$(pct_drop "$jit_startup_p50" "$aot_startup_p50")"
startup_improvement_p95_pct="$(pct_drop "$jit_startup_p95" "$aot_startup_p95")"
startup_improvement_p99_pct="$(pct_drop "$jit_startup_p99" "$aot_startup_p99")"
throughput_improvement_p50_pct="$(pct_gain "$jit_rps_p50" "$aot_rps_p50")"
throughput_improvement_p95_pct="$(pct_gain "$jit_rps_p95" "$aot_rps_p95")"
throughput_improvement_p99_pct="$(pct_gain "$jit_rps_p99" "$aot_rps_p99")"

collect_environment_metadata
timestamp="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
git_rev="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")"

cat >"$METRICS_JSON" <<EOF
{
  "schema_version": 1,
  "timestamp_utc": "$timestamp",
  "benchmark_context": {
    "samples": $SAMPLES,
    "burst_runs_per_sample": $BURST_RUNS,
    "git_rev": "$(json_escape "$git_rev")",
    "fixture_source": "$(json_escape "$FIXTURE_SRC")",
    "fixture_manifest": "$(json_escape "$FIXTURE_DIR/fuse.toml")",
    "environment": {
      "os_name": "$(json_escape "$BENCH_OS_NAME")",
      "os_release": "$(json_escape "$BENCH_OS_RELEASE")",
      "arch": "$(json_escape "$BENCH_OS_ARCH")",
      "cpu_model": "$(json_escape "$BENCH_CPU_MODEL")",
      "cpu_count": $BENCH_CPU_COUNT
    },
    "aot_build_info": "$(json_escape "$AOT_BUILD_INFO")"
  },
  "cold_start_ms": {
    "jit_native": {
      "samples": $(json_number_array "${jit_startup_samples[@]}"),
      "p50": $jit_startup_p50,
      "p95": $jit_startup_p95,
      "p99": $jit_startup_p99
    },
    "aot": {
      "samples": $(json_number_array "${aot_startup_samples[@]}"),
      "p50": $aot_startup_p50,
      "p95": $aot_startup_p95,
      "p99": $aot_startup_p99
    },
    "improvement_pct": {
      "p50": $startup_improvement_p50_pct,
      "p95": $startup_improvement_p95_pct,
      "p99": $startup_improvement_p99_pct
    }
  },
  "steady_state_cli_runs_per_sec": {
    "jit_native": {
      "samples": $(json_number_array "${jit_rps_samples[@]}"),
      "p50": $jit_rps_p50,
      "p95": $jit_rps_p95,
      "p99": $jit_rps_p99
    },
    "aot": {
      "samples": $(json_number_array "${aot_rps_samples[@]}"),
      "p50": $aot_rps_p50,
      "p95": $aot_rps_p95,
      "p99": $aot_rps_p99
    },
    "improvement_pct": {
      "p50": $throughput_improvement_p50_pct,
      "p95": $throughput_improvement_p95_pct,
      "p99": $throughput_improvement_p99_pct
    }
  }
}
EOF

cat >"$METRICS_MD" <<EOF
# FUSE AOT Performance Metrics

Generated: \`$timestamp\`  
Git revision: \`$git_rev\`  
Fixture source: \`$FIXTURE_SRC\`  
Samples/backend: \`$SAMPLES\`  
Burst runs/sample: \`$BURST_RUNS\`

## Cold Start (ms, process start to successful completion)

| Backend | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| JIT-native | ${jit_startup_p50} ms | ${jit_startup_p95} ms | ${jit_startup_p99} ms |
| AOT | ${aot_startup_p50} ms | ${aot_startup_p95} ms | ${aot_startup_p99} ms |

Improvement vs JIT-native:
- p50: ${startup_improvement_p50_pct}%
- p95: ${startup_improvement_p95_pct}%
- p99: ${startup_improvement_p99_pct}%

## Steady-State Throughput (CLI runs/s)

| Backend | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| JIT-native | ${jit_rps_p50} | ${jit_rps_p95} | ${jit_rps_p99} |
| AOT | ${aot_rps_p50} | ${aot_rps_p95} | ${aot_rps_p99} |

Throughput delta vs JIT-native:
- p50: ${throughput_improvement_p50_pct}%
- p95: ${throughput_improvement_p95_pct}%
- p99: ${throughput_improvement_p99_pct}%

## AOT Build Metadata

\`\`\`txt
$AOT_BUILD_INFO
\`\`\`

Raw JSON metrics: \`$METRICS_JSON\`
EOF

cat "$METRICS_MD"
