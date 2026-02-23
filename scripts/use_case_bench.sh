#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/.fuse/bench"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/fuse_use_case_bench.XXXXXX")"
REFERENCE_SERVICE_DIR="$ROOT/examples/reference-service"
METRICS_JSON="$OUT_DIR/use_case_metrics.json"
METRICS_MD="$OUT_DIR/use_case_metrics.md"

BENCH_MODE="single"
SAMPLE_COUNT=1
SERVER_PID=""
LAST_PROBE_CODE=0
TMP_DIR=""
LOG_FILE=""
PORT=""
DB_URL=""

declare -a cli_check_samples=()
declare -a cli_run_ok_samples=()
declare -a cli_run_invalid_samples=()
declare -a notes_check_cold_samples=()
declare -a notes_check_warm_samples=()
declare -a notes_migrate_samples=()
declare -a notes_get_list_samples=()
declare -a notes_post_ok_samples=()
declare -a notes_post_invalid_samples=()
declare -a notes_frontend_get_samples=()
declare -a notes_validation_error_overhead_samples=()

usage() {
  cat <<'USAGE'
Usage: scripts/use_case_bench.sh [options]

Options:
  --median-of-3  Record 3 benchmark samples and emit the median for each metric
  -h, --help     Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --median-of-3)
      BENCH_MODE="median_of_3"
      SAMPLE_COUNT=3
      shift
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

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

now_ns() {
  date +%s%N
}

measure_cmd_ms() {
  local __ms_var="$1"
  shift
  local start_ns end_ns log_file
  log_file="$(mktemp "${TMPDIR:-/tmp}/fuse_use_case_cmd.XXXXXX.log")"
  start_ns="$(now_ns)"
  if ! "$@" >"$log_file" 2>&1; then
    cat "$log_file" >&2
    rm -f "$log_file"
    return 1
  fi
  end_ns="$(now_ns)"
  rm -f "$log_file"
  printf -v "$__ms_var" "%s" "$(awk -v ns="$((end_ns - start_ns))" 'BEGIN { printf "%.3f", (ns / 1000000) }')"
}

measure_cmd_ms_with_status() {
  local __ms_var="$1"
  local __status_var="$2"
  shift 2
  local start_ns end_ns status
  start_ns="$(now_ns)"
  set +e
  "$@" >/dev/null 2>&1
  status=$?
  set -e
  end_ns="$(now_ns)"
  printf -v "$__ms_var" "%s" "$(awk -v ns="$((end_ns - start_ns))" 'BEGIN { printf "%.3f", (ns / 1000000) }')"
  printf -v "$__status_var" "%d" "$status"
}

http_request_status_ms() {
  local method="$1"
  local url="$2"
  local body="${3:-}"
  local response status seconds ms

  if [[ -n "$body" ]]; then
    response="$(
      curl -sS -o /dev/null -w '%{http_code} %{time_total}' \
        --connect-timeout 1 \
        --max-time 3 \
        --retry 2 \
        --retry-delay 0 \
        --retry-connrefused \
        -X "$method" \
        -H 'content-type: application/json' \
        --data "$body" \
        "$url"
    )"
  else
    response="$(
      curl -sS -o /dev/null -w '%{http_code} %{time_total}' \
        --connect-timeout 1 \
        --max-time 3 \
        --retry 2 \
        --retry-delay 0 \
        --retry-connrefused \
        -X "$method" \
        "$url"
    )"
  fi

  status="${response%% *}"
  seconds="${response##* }"
  ms="$(awk -v s="$seconds" 'BEGIN { printf "%.3f", (s * 1000) }')"
  echo "$status $ms"
}

wait_for_http() {
  local url="$1"
  local pid="${2:-}"
  local timeout_secs="${3:-12}"
  local deadline=$((SECONDS + timeout_secs))
  while (( SECONDS < deadline )); do
    if [[ -n "$pid" ]] && ! kill -0 "$pid" >/dev/null 2>&1; then
      return 2
    fi
    if curl -s -o /dev/null --connect-timeout 1 --max-time 1 "$url" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

start_reference_service_runtime() {
  local attempts="${1:-2}"
  local timeout_secs="${2:-12}"
  local probe_code
  local attempt

  for ((attempt = 1; attempt <= attempts; attempt++)); do
    PORT="$((39000 + RANDOM % 1000))"
    echo "Using reference-service benchmark port (attempt $attempt/$attempts): $PORT"
    : >"$LOG_FILE"
    env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
      "$ROOT/scripts/fuse" run --manifest-path "$REFERENCE_SERVICE_DIR" >"$LOG_FILE" 2>&1 &
    SERVER_PID="$!"
    probe_code=0
    wait_for_http "http://127.0.0.1:${PORT}/api/notes" "$SERVER_PID" "$timeout_secs" || probe_code=$?
    LAST_PROBE_CODE="$probe_code"
    if [[ "$probe_code" -eq 0 ]]; then
      return 0
    fi

    echo "reference-service readiness attempt ${attempt}/${attempts} failed (probe_code=${probe_code}); retrying..." >&2
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
    SERVER_PID=""
    sleep 0.2
  done

  return 1
}

median_of_three() {
  printf "%s\n%s\n%s\n" "$1" "$2" "$3" | sort -n | sed -n '2p'
}

json_escape() {
  printf '%s' "$1" | tr '\n\r\t' '   ' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

collect_environment_metadata() {
  BENCH_OS_NAME="$(uname -s 2>/dev/null || echo "unknown")"
  BENCH_OS_RELEASE="$(uname -r 2>/dev/null || echo "unknown")"
  BENCH_OS_ARCH="$(uname -m 2>/dev/null || echo "unknown")"
  BENCH_CPU_MODEL="unknown"
  if command -v lscpu >/dev/null 2>&1; then
    BENCH_CPU_MODEL="$(lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p' | head -n 1)"
  elif [[ -f /proc/cpuinfo ]]; then
    BENCH_CPU_MODEL="$(sed -n 's/^model name[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | head -n 1)"
  fi
  if [[ -z "$BENCH_CPU_MODEL" ]]; then
    BENCH_CPU_MODEL="unknown"
  fi

  BENCH_CPU_COUNT="$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)"
  if [[ -z "$BENCH_CPU_COUNT" ]] || ! [[ "$BENCH_CPU_COUNT" =~ ^[0-9]+$ ]]; then
    BENCH_CPU_COUNT=1
  fi

  BENCH_CI=false
  BENCH_CI_PROVIDER="none"
  BENCH_CI_RUNNER_HINT="local"
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    BENCH_CI=true
    BENCH_CI_PROVIDER="github_actions"
    BENCH_CI_RUNNER_HINT="${RUNNER_NAME:-${RUNNER_OS:-unknown}-${RUNNER_ARCH:-unknown}}"
  elif [[ -n "${CI:-}" ]]; then
    BENCH_CI=true
    BENCH_CI_PROVIDER="${CI_PROVIDER:-generic_ci}"
    BENCH_CI_RUNNER_HINT="${CI_RUNNER_DESCRIPTION:-unknown}"
  fi
}

run_single_iteration() {
  local iteration="$1"
  local cli_check_ms cli_run_ok_ms cli_run_invalid_ms cli_run_invalid_status
  local notes_check_cold_ms notes_check_warm_ms notes_migrate_ms
  local status_list status_post_ok status_post_invalid status_root
  local notes_get_list_ms notes_post_ok_ms notes_post_invalid_ms notes_frontend_get_ms
  local notes_validation_error_overhead_ms

  TMP_DIR="$TMP_ROOT/iter_${iteration}"
  mkdir -p "$TMP_DIR"
  LOG_FILE="$TMP_DIR/reference-service.log"
  SERVER_PID=""

  echo "Running CLI workload metrics..."
  measure_cmd_ms cli_check_ms "$ROOT/scripts/fuse" check "$ROOT/examples/project_demo.fuse"
  measure_cmd_ms cli_run_ok_ms env APP_GREETING="Hello" APP_WHO="Bench" \
    "$ROOT/scripts/fuse" run --backend vm "$ROOT/examples/project_demo.fuse"
  measure_cmd_ms_with_status cli_run_invalid_ms cli_run_invalid_status \
    env APP_GREETING="Hello" APP_WHO="Bench" DEMO_FAIL="1" \
    "$ROOT/scripts/fuse" run --backend vm "$ROOT/examples/project_demo.fuse"
  if [[ "$cli_run_invalid_status" -eq 0 ]]; then
    echo "Expected project_demo contract-failure run to fail (DEMO_FAIL=1)." >&2
    exit 1
  fi

  echo "Running reference-service compile/check workload metrics..."
  PORT="$((39000 + RANDOM % 1000))"
  DB_URL="sqlite://$TMP_DIR/reference-service.db"
  echo "Using reference-service benchmark DB: $DB_URL"

  measure_cmd_ms notes_check_cold_ms env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
    "$ROOT/scripts/fuse" check --manifest-path "$REFERENCE_SERVICE_DIR"
  measure_cmd_ms notes_check_warm_ms env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
    "$ROOT/scripts/fuse" check --manifest-path "$REFERENCE_SERVICE_DIR"
  measure_cmd_ms notes_migrate_ms env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
    "$ROOT/scripts/fuse" migrate --manifest-path "$REFERENCE_SERVICE_DIR"

  echo "Running reference-service runtime/request metrics..."
  echo "Waiting for reference-service HTTP readiness (timeout ~12s, with deterministic retry)..."
  if ! start_reference_service_runtime 2 12; then
    echo "reference-service did not become ready for benchmarking (probe_code=${LAST_PROBE_CODE})." >&2
    if [[ -f "$LOG_FILE" ]]; then
      echo "reference-service log (tail):" >&2
      tail -n 80 "$LOG_FILE" >&2 || cat "$LOG_FILE" >&2
    fi
    exit 1
  fi

  # Warm route and DB path before recording request metrics.
  curl -sS -o /dev/null --max-time 3 "http://127.0.0.1:${PORT}/api/notes" || true
  curl -sS -o /dev/null --max-time 3 \
    -X POST \
    -H 'content-type: application/json' \
    --data '{"title":"bench-warmup","content":"warm"}' \
    "http://127.0.0.1:${PORT}/api/notes" || true

  read -r status_list notes_get_list_ms <<<"$(http_request_status_ms GET "http://127.0.0.1:${PORT}/api/notes")"
  if [[ "$status_list" != "200" ]]; then
    echo "GET /api/notes expected 200, got $status_list" >&2
    exit 1
  fi

  read -r status_post_ok notes_post_ok_ms <<<"$(
    http_request_status_ms POST "http://127.0.0.1:${PORT}/api/notes" '{"title":"bench-note","content":"hello"}'
  )"
  if [[ "$status_post_ok" != "200" ]]; then
    echo "POST /api/notes valid payload expected 200, got $status_post_ok" >&2
    exit 1
  fi

  read -r status_post_invalid notes_post_invalid_ms <<<"$(
    http_request_status_ms POST "http://127.0.0.1:${PORT}/api/notes" '{"title":"","content":""}'
  )"
  if [[ "$status_post_invalid" != "400" ]]; then
    echo "POST /api/notes invalid payload expected 400, got $status_post_invalid" >&2
    exit 1
  fi

  read -r status_root notes_frontend_get_ms <<<"$(http_request_status_ms GET "http://127.0.0.1:${PORT}/")"
  if [[ "$status_root" != "200" ]]; then
    echo "GET / expected 200, got $status_root" >&2
    exit 1
  fi

  notes_validation_error_overhead_ms="$(awk -v a="$notes_post_invalid_ms" -v b="$notes_post_ok_ms" 'BEGIN { d = a - b; if (d < 0) d = -d; printf "%.3f", d }')"

  kill "$SERVER_PID" >/dev/null 2>&1 || true
  wait "$SERVER_PID" >/dev/null 2>&1 || true
  SERVER_PID=""

  cli_check_samples+=("$cli_check_ms")
  cli_run_ok_samples+=("$cli_run_ok_ms")
  cli_run_invalid_samples+=("$cli_run_invalid_ms")
  notes_check_cold_samples+=("$notes_check_cold_ms")
  notes_check_warm_samples+=("$notes_check_warm_ms")
  notes_migrate_samples+=("$notes_migrate_ms")
  notes_get_list_samples+=("$notes_get_list_ms")
  notes_post_ok_samples+=("$notes_post_ok_ms")
  notes_post_invalid_samples+=("$notes_post_invalid_ms")
  notes_frontend_get_samples+=("$notes_frontend_get_ms")
  notes_validation_error_overhead_samples+=("$notes_validation_error_overhead_ms")
}

aggregate_final_metrics() {
  if [[ "$SAMPLE_COUNT" -eq 1 ]]; then
    cli_check_ms="${cli_check_samples[0]}"
    cli_run_ok_ms="${cli_run_ok_samples[0]}"
    cli_run_invalid_ms="${cli_run_invalid_samples[0]}"
    notes_check_cold_ms="${notes_check_cold_samples[0]}"
    notes_check_warm_ms="${notes_check_warm_samples[0]}"
    notes_migrate_ms="${notes_migrate_samples[0]}"
    notes_get_list_ms="${notes_get_list_samples[0]}"
    notes_post_ok_ms="${notes_post_ok_samples[0]}"
    notes_post_invalid_ms="${notes_post_invalid_samples[0]}"
    notes_frontend_get_ms="${notes_frontend_get_samples[0]}"
    notes_validation_error_overhead_ms="${notes_validation_error_overhead_samples[0]}"
    return
  fi

  cli_check_ms="$(median_of_three "${cli_check_samples[0]}" "${cli_check_samples[1]}" "${cli_check_samples[2]}")"
  cli_run_ok_ms="$(median_of_three "${cli_run_ok_samples[0]}" "${cli_run_ok_samples[1]}" "${cli_run_ok_samples[2]}")"
  cli_run_invalid_ms="$(median_of_three "${cli_run_invalid_samples[0]}" "${cli_run_invalid_samples[1]}" "${cli_run_invalid_samples[2]}")"
  notes_check_cold_ms="$(median_of_three "${notes_check_cold_samples[0]}" "${notes_check_cold_samples[1]}" "${notes_check_cold_samples[2]}")"
  notes_check_warm_ms="$(median_of_three "${notes_check_warm_samples[0]}" "${notes_check_warm_samples[1]}" "${notes_check_warm_samples[2]}")"
  notes_migrate_ms="$(median_of_three "${notes_migrate_samples[0]}" "${notes_migrate_samples[1]}" "${notes_migrate_samples[2]}")"
  notes_get_list_ms="$(median_of_three "${notes_get_list_samples[0]}" "${notes_get_list_samples[1]}" "${notes_get_list_samples[2]}")"
  notes_post_ok_ms="$(median_of_three "${notes_post_ok_samples[0]}" "${notes_post_ok_samples[1]}" "${notes_post_ok_samples[2]}")"
  notes_post_invalid_ms="$(median_of_three "${notes_post_invalid_samples[0]}" "${notes_post_invalid_samples[1]}" "${notes_post_invalid_samples[2]}")"
  notes_frontend_get_ms="$(median_of_three "${notes_frontend_get_samples[0]}" "${notes_frontend_get_samples[1]}" "${notes_frontend_get_samples[2]}")"
  notes_validation_error_overhead_ms="$(median_of_three "${notes_validation_error_overhead_samples[0]}" "${notes_validation_error_overhead_samples[1]}" "${notes_validation_error_overhead_samples[2]}")"
}

print_row() {
  local workload="$1"
  local metric="$2"
  local value="$3"
  printf "| %s | %s | %s |\n" "$workload" "$metric" "$value"
}

mkdir -p "$OUT_DIR"

echo "Preparing benchmark harness (building fuse/fusec once)..."
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse -p fusec >/dev/null

for ((sample = 1; sample <= SAMPLE_COUNT; sample++)); do
  if [[ "$SAMPLE_COUNT" -gt 1 ]]; then
    echo "Collecting benchmark sample $sample/$SAMPLE_COUNT..."
  fi
  run_single_iteration "$sample"
done

aggregate_final_metrics
collect_environment_metadata
timestamp="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

cat >"$METRICS_JSON" <<EOF
{
  "schema_version": 2,
  "timestamp_utc": "$timestamp",
  "benchmark_context": {
    "mode": "$BENCH_MODE",
    "sample_count": $SAMPLE_COUNT,
    "environment": {
      "os_name": "$(json_escape "$BENCH_OS_NAME")",
      "os_release": "$(json_escape "$BENCH_OS_RELEASE")",
      "arch": "$(json_escape "$BENCH_OS_ARCH")",
      "cpu_model": "$(json_escape "$BENCH_CPU_MODEL")",
      "cpu_count": $BENCH_CPU_COUNT,
      "ci": $BENCH_CI,
      "ci_provider": "$(json_escape "$BENCH_CI_PROVIDER")",
      "ci_runner_hint": "$(json_escape "$BENCH_CI_RUNNER_HINT")"
    }
  },
  "cli_project_demo": {
    "check_ms": $cli_check_ms,
    "run_ok_ms": $cli_run_ok_ms,
    "run_contract_failure_ms": $cli_run_invalid_ms
  },
  "reference_service": {
    "check_cold_ms": $notes_check_cold_ms,
    "check_warm_ms": $notes_check_warm_ms,
    "migrate_ms": $notes_migrate_ms,
    "request_get_notes_ms": $notes_get_list_ms,
    "request_post_valid_ms": $notes_post_ok_ms,
    "request_post_invalid_ms": $notes_post_invalid_ms,
    "request_frontend_root_ms": $notes_frontend_get_ms,
    "request_validation_error_overhead_abs_ms": $notes_validation_error_overhead_ms
  }
}
EOF

{
  echo "# FUSE Use-Case Metrics"
  echo
  echo "Generated: \`$timestamp\`"
  echo "Mode: \`$BENCH_MODE\` (samples: $SAMPLE_COUNT)"
  echo "Environment: \`$BENCH_OS_NAME $BENCH_OS_RELEASE ($BENCH_OS_ARCH)\` / CPU \`$BENCH_CPU_MODEL\` x$BENCH_CPU_COUNT / CI \`$BENCH_CI_PROVIDER\`"
  echo
  echo "| Workload | Metric | Value |"
  echo "| --- | --- | --- |"
  print_row "CLI: project_demo" "check time" "${cli_check_ms} ms"
  print_row "CLI: project_demo" "run (valid)" "${cli_run_ok_ms} ms"
  print_row "CLI: project_demo" "run (contract failure)" "${cli_run_invalid_ms} ms"
  print_row "Package: reference-service" "check (cold)" "${notes_check_cold_ms} ms"
  print_row "Package: reference-service" "check (warm)" "${notes_check_warm_ms} ms"
  print_row "Package: reference-service" "migrate" "${notes_migrate_ms} ms"
  print_row "Runtime: reference-service" "GET /api/notes" "${notes_get_list_ms} ms"
  print_row "Runtime: reference-service" "POST /api/notes (valid body)" "${notes_post_ok_ms} ms"
  print_row "Runtime: reference-service" "POST /api/notes (invalid body, 400)" "${notes_post_invalid_ms} ms"
  print_row "Frontend integration" "GET / (index)" "${notes_frontend_get_ms} ms"
  print_row "Runtime: reference-service" "validation error overhead abs(POST invalid - POST valid)" "${notes_validation_error_overhead_ms} ms"
  echo
  echo "Raw JSON metrics: \`$METRICS_JSON\`"
} >"$METRICS_MD"

cat "$METRICS_MD"
