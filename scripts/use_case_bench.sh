#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/.fuse/bench"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fuse_use_case_bench.XXXXXX")"
NOTES_DIR="$ROOT/examples/notes-api"
LOG_FILE="$TMP_DIR/notes-api.log"
METRICS_JSON="$OUT_DIR/use_case_metrics.json"
METRICS_MD="$OUT_DIR/use_case_metrics.md"
SERVER_PID=""

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP_DIR"
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
  printf -v "$__ms_var" "%d" $(((end_ns - start_ns) / 1000000))
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
  printf -v "$__ms_var" "%d" $(((end_ns - start_ns) / 1000000))
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
        -X "$method" \
        -H 'content-type: application/json' \
        --data "$body" \
        "$url"
    )"
  else
    response="$(
      curl -sS -o /dev/null -w '%{http_code} %{time_total}' \
        -X "$method" \
        "$url"
    )"
  fi

  status="${response%% *}"
  seconds="${response##* }"
  ms="$(awk -v s="$seconds" 'BEGIN { printf "%d", (s * 1000) }')"
  echo "$status $ms"
}

wait_for_http() {
  local url="$1"
  local pid="${2:-}"
  local timeout_secs="${3:-12}"
  local deadline=$((SECONDS + timeout_secs))
  local i
  i=0
  while (( SECONDS < deadline )); do
    ((i += 1))
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

print_row() {
  local workload="$1"
  local metric="$2"
  local value="$3"
  printf "| %s | %s | %s |\n" "$workload" "$metric" "$value"
}

mkdir -p "$OUT_DIR"

echo "Preparing benchmark harness (building fuse/fusec once)..."
"$ROOT/scripts/cargo_env.sh" cargo build -p fuse -p fusec >/dev/null

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

echo "Running notes-api compile/check workload metrics..."
PORT="$((39000 + RANDOM % 1000))"
DB_URL="sqlite://$TMP_DIR/notes.db"
echo "Using notes-api benchmark port: $PORT"

measure_cmd_ms notes_check_cold_ms env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
  "$ROOT/scripts/fuse" check --manifest-path "$NOTES_DIR"
measure_cmd_ms notes_check_warm_ms env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
  "$ROOT/scripts/fuse" check --manifest-path "$NOTES_DIR"
measure_cmd_ms notes_migrate_ms env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
  "$ROOT/scripts/fuse" migrate --manifest-path "$NOTES_DIR"

echo "Running notes-api runtime/request metrics..."
env APP_PORT="$PORT" PORT="$PORT" FUSE_DB_URL="$DB_URL" \
  "$ROOT/scripts/fuse" run --manifest-path "$NOTES_DIR" >"$LOG_FILE" 2>&1 &
SERVER_PID="$!"
echo "Waiting for notes-api HTTP readiness (timeout ~12s)..."
probe_code=0
wait_for_http "http://127.0.0.1:${PORT}/api/notes" "$SERVER_PID" 12 || probe_code=$?
if [[ "$probe_code" -ne 0 ]]; then
  echo "notes-api did not become ready for benchmarking (probe_code=${probe_code})." >&2
  if [[ -f "$LOG_FILE" ]]; then
    echo "notes-api log (tail):" >&2
    tail -n 80 "$LOG_FILE" >&2 || cat "$LOG_FILE" >&2
  fi
  exit 1
fi

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

if (( notes_post_invalid_ms >= notes_post_ok_ms )); then
  notes_contract_delta_ms=$((notes_post_invalid_ms - notes_post_ok_ms))
else
  notes_contract_delta_ms=$((notes_post_ok_ms - notes_post_invalid_ms))
fi

kill "$SERVER_PID" >/dev/null 2>&1 || true
wait "$SERVER_PID" >/dev/null 2>&1 || true
SERVER_PID=""

timestamp="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

cat >"$METRICS_JSON" <<EOF
{
  "timestamp_utc": "$timestamp",
  "cli_project_demo": {
    "check_ms": $cli_check_ms,
    "run_ok_ms": $cli_run_ok_ms,
    "run_contract_failure_ms": $cli_run_invalid_ms
  },
  "notes_api": {
    "check_cold_ms": $notes_check_cold_ms,
    "check_warm_ms": $notes_check_warm_ms,
    "migrate_ms": $notes_migrate_ms,
    "request_get_notes_ms": $notes_get_list_ms,
    "request_post_valid_ms": $notes_post_ok_ms,
    "request_post_invalid_ms": $notes_post_invalid_ms,
    "request_frontend_root_ms": $notes_frontend_get_ms,
    "contract_validation_delta_ms": $notes_contract_delta_ms
  }
}
EOF

{
  echo "# FUSE Use-Case Metrics"
  echo
  echo "Generated: \`$timestamp\`"
  echo
  echo "| Workload | Metric | Value |"
  echo "| --- | --- | --- |"
  print_row "CLI: project_demo" "check time" "${cli_check_ms} ms"
  print_row "CLI: project_demo" "run (valid)" "${cli_run_ok_ms} ms"
  print_row "CLI: project_demo" "run (contract failure)" "${cli_run_invalid_ms} ms"
  print_row "Package: notes-api" "check (cold)" "${notes_check_cold_ms} ms"
  print_row "Package: notes-api" "check (warm)" "${notes_check_warm_ms} ms"
  print_row "Package: notes-api" "migrate" "${notes_migrate_ms} ms"
  print_row "Runtime: notes-api" "GET /api/notes" "${notes_get_list_ms} ms"
  print_row "Runtime: notes-api" "POST /api/notes (valid body)" "${notes_post_ok_ms} ms"
  print_row "Runtime: notes-api" "POST /api/notes (invalid body, 400)" "${notes_post_invalid_ms} ms"
  print_row "Frontend integration" "GET / (index)" "${notes_frontend_get_ms} ms"
  print_row "Contract cost signal" "|invalid-valid| delta" "${notes_contract_delta_ms} ms"
  echo
  echo "Raw JSON metrics: \`$METRICS_JSON\`"
} >"$METRICS_MD"

cat "$METRICS_MD"
