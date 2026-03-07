use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::Duration;

use fuse_rt::json as rt_json;

use crate::concurrency_metrics::ConcurrencySnapshot;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const REQUEST_ID_FALLBACK_HEADER: &str = "x-correlation-id";
pub const RESPONSE_REQUEST_ID_HEADER: &str = "X-Request-Id";

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_SHUTDOWN_SIGNAL: AtomicI32 = AtomicI32::new(0);
static SHUTDOWN_SIGNAL_INIT: Once = Once::new();

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanicDetails {
    pub kind: &'static str,
    pub message: String,
}

pub fn classify_panic_payload(payload: &(dyn Any + Send)) -> PanicDetails {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return PanicDetails {
            kind: "panic_static_str",
            message: (*message).to_string(),
        };
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return PanicDetails {
            kind: "panic_string",
            message: message.clone(),
        };
    }
    PanicDetails {
        kind: "panic_non_string",
        message: "panic".to_string(),
    }
}

pub fn format_panic_message(details: &PanicDetails) -> String {
    if details.message.is_empty() {
        format!("panic_kind={}", details.kind)
    } else {
        format!("panic_kind={} {}", details.kind, details.message)
    }
}

pub fn resolve_request_id(headers: &HashMap<String, String>) -> String {
    request_id_from_header(headers, REQUEST_ID_HEADER)
        .or_else(|| request_id_from_header(headers, REQUEST_ID_FALLBACK_HEADER))
        .unwrap_or_else(next_request_id)
}

pub fn emit_http_observability(
    runtime: &str,
    request_id: &str,
    method: &str,
    path: &str,
    status: u16,
    duration: Duration,
    response_bytes: usize,
) {
    let duration_ms = duration.as_millis() as f64;
    if structured_request_logging_enabled() {
        let mut obj = BTreeMap::new();
        obj.insert(
            "duration_ms".to_string(),
            rt_json::JsonValue::Number(duration_ms),
        );
        obj.insert(
            "event".to_string(),
            rt_json::JsonValue::String("http.request".to_string()),
        );
        obj.insert(
            "method".to_string(),
            rt_json::JsonValue::String(method.to_string()),
        );
        obj.insert(
            "path".to_string(),
            rt_json::JsonValue::String(path.to_string()),
        );
        obj.insert(
            "request_id".to_string(),
            rt_json::JsonValue::String(request_id.to_string()),
        );
        obj.insert(
            "response_bytes".to_string(),
            rt_json::JsonValue::Number(response_bytes as f64),
        );
        obj.insert(
            "runtime".to_string(),
            rt_json::JsonValue::String(runtime.to_string()),
        );
        obj.insert(
            "status".to_string(),
            rt_json::JsonValue::Number(status as f64),
        );
        eprintln!("{}", rt_json::encode(&rt_json::JsonValue::Object(obj)));
    }

    if metrics_hook_mode() == MetricsHookMode::Stderr {
        let mut obj = BTreeMap::new();
        obj.insert(
            "duration_ms".to_string(),
            rt_json::JsonValue::Number(duration_ms),
        );
        obj.insert(
            "metric".to_string(),
            rt_json::JsonValue::String("http.server.request".to_string()),
        );
        obj.insert(
            "method".to_string(),
            rt_json::JsonValue::String(method.to_string()),
        );
        obj.insert(
            "path".to_string(),
            rt_json::JsonValue::String(path.to_string()),
        );
        obj.insert(
            "request_id".to_string(),
            rt_json::JsonValue::String(request_id.to_string()),
        );
        obj.insert(
            "runtime".to_string(),
            rt_json::JsonValue::String(runtime.to_string()),
        );
        obj.insert(
            "status".to_string(),
            rt_json::JsonValue::Number(status as f64),
        );
        eprintln!(
            "metrics: {}",
            rt_json::encode(&rt_json::JsonValue::Object(obj))
        );
    }
}

pub fn emit_http_client_observability(
    runtime: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    duration: Duration,
    response_bytes: usize,
    error_code: Option<&str>,
) {
    let duration_ms = duration.as_millis() as f64;
    let outcome = if error_code.is_some() { "error" } else { "success" };

    if structured_request_logging_enabled() {
        let mut obj = BTreeMap::new();
        obj.insert(
            "duration_ms".to_string(),
            rt_json::JsonValue::Number(duration_ms),
        );
        obj.insert(
            "event".to_string(),
            rt_json::JsonValue::String("http.client.request".to_string()),
        );
        obj.insert(
            "method".to_string(),
            rt_json::JsonValue::String(method.to_string()),
        );
        obj.insert(
            "outcome".to_string(),
            rt_json::JsonValue::String(outcome.to_string()),
        );
        obj.insert(
            "response_bytes".to_string(),
            rt_json::JsonValue::Number(response_bytes as f64),
        );
        obj.insert(
            "runtime".to_string(),
            rt_json::JsonValue::String(runtime.to_string()),
        );
        obj.insert(
            "status".to_string(),
            match status {
                Some(status) => rt_json::JsonValue::Number(status as f64),
                None => rt_json::JsonValue::Null,
            },
        );
        obj.insert(
            "url".to_string(),
            rt_json::JsonValue::String(url.to_string()),
        );
        obj.insert(
            "error_code".to_string(),
            match error_code {
                Some(code) => rt_json::JsonValue::String(code.to_string()),
                None => rt_json::JsonValue::Null,
            },
        );
        eprintln!("{}", rt_json::encode(&rt_json::JsonValue::Object(obj)));
    }

    if metrics_hook_mode() == MetricsHookMode::Stderr {
        let mut obj = BTreeMap::new();
        obj.insert(
            "duration_ms".to_string(),
            rt_json::JsonValue::Number(duration_ms),
        );
        obj.insert(
            "metric".to_string(),
            rt_json::JsonValue::String("http.client.request".to_string()),
        );
        obj.insert(
            "method".to_string(),
            rt_json::JsonValue::String(method.to_string()),
        );
        obj.insert(
            "outcome".to_string(),
            rt_json::JsonValue::String(outcome.to_string()),
        );
        obj.insert(
            "response_bytes".to_string(),
            rt_json::JsonValue::Number(response_bytes as f64),
        );
        obj.insert(
            "runtime".to_string(),
            rt_json::JsonValue::String(runtime.to_string()),
        );
        obj.insert(
            "status".to_string(),
            match status {
                Some(status) => rt_json::JsonValue::Number(status as f64),
                None => rt_json::JsonValue::Null,
            },
        );
        obj.insert(
            "url".to_string(),
            rt_json::JsonValue::String(url.to_string()),
        );
        obj.insert(
            "error_code".to_string(),
            match error_code {
                Some(code) => rt_json::JsonValue::String(code.to_string()),
                None => rt_json::JsonValue::Null,
            },
        );
        eprintln!(
            "metrics: {}",
            rt_json::encode(&rt_json::JsonValue::Object(obj))
        );
    }
}

pub fn parse_http_response_status_and_body_len(response: &str) -> (u16, usize) {
    let mut sections = response.splitn(2, "\r\n\r\n");
    let head = sections.next().unwrap_or("");
    let body = sections.next().unwrap_or("");
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("500")
        .parse::<u16>()
        .unwrap_or(500);
    (status, body.len())
}

pub fn inject_request_id_header(response: String, request_id: &str) -> String {
    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return response;
    };
    let mut lines = head.split("\r\n");
    let Some(status_line) = lines.next() else {
        return response;
    };
    let mut out = String::new();
    out.push_str(status_line);
    out.push_str("\r\n");
    out.push_str(RESPONSE_REQUEST_ID_HEADER);
    out.push_str(": ");
    out.push_str(request_id);
    out.push_str("\r\n");
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, _)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case(REQUEST_ID_HEADER) {
                continue;
            }
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

pub fn begin_graceful_shutdown_session() {
    #[cfg(unix)]
    {
        SHUTDOWN_SIGNAL_INIT.call_once(|| unsafe {
            install_unix_shutdown_signal_handler(2);
            install_unix_shutdown_signal_handler(15);
        });
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        LAST_SHUTDOWN_SIGNAL.store(0, Ordering::SeqCst);
    }
}

pub fn graceful_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

pub fn take_shutdown_signal_name() -> Option<&'static str> {
    #[cfg(unix)]
    {
        match LAST_SHUTDOWN_SIGNAL.swap(0, Ordering::SeqCst) {
            2 => Some("SIGINT"),
            15 => Some("SIGTERM"),
            _ => None,
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}

#[cfg(unix)]
extern "C" fn handle_unix_shutdown_signal(sig: i32) {
    LAST_SHUTDOWN_SIGNAL.store(sig, Ordering::SeqCst);
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
unsafe fn install_unix_shutdown_signal_handler(sig: i32) {
    let _ = unsafe { signal(sig, handle_unix_shutdown_signal as *const () as usize) };
}

fn request_id_from_header(headers: &HashMap<String, String>, key: &str) -> Option<String> {
    headers.get(key).and_then(|raw| sanitize_request_id(raw))
}

fn sanitize_request_id(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/' | '@'))
    {
        Some(value.to_string())
    } else {
        None
    }
}

fn next_request_id() -> String {
    let next = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("req-{next:016x}")
}

/// Emit a `concurrency.snapshot` metric line after a `--run` invocation completes.
///
/// Only emits when `total_spawned > 0` (i.e., the program actually used `spawn`).
///
/// - When `diagnostics_json` is true, emits a structured NDJSON event line to stderr
///   (same channel used by `--diagnostics json` diagnostic output).
/// - When `FUSE_METRICS_HOOK=stderr`, emits `metrics: {...}` to stderr regardless.
pub fn emit_concurrency_metrics(snapshot: &ConcurrencySnapshot, diagnostics_json: bool) {
    if snapshot.total_spawned == 0 {
        return;
    }

    let build_hist = || {
        let mut h = BTreeMap::new();
        h.insert(
            "ge_1s".to_string(),
            rt_json::JsonValue::Number(snapshot.latency_hist[4] as f64),
        );
        h.insert(
            "lt_100ms".to_string(),
            rt_json::JsonValue::Number(snapshot.latency_hist[2] as f64),
        );
        h.insert(
            "lt_10ms".to_string(),
            rt_json::JsonValue::Number(snapshot.latency_hist[1] as f64),
        );
        h.insert(
            "lt_1ms".to_string(),
            rt_json::JsonValue::Number(snapshot.latency_hist[0] as f64),
        );
        h.insert(
            "lt_1s".to_string(),
            rt_json::JsonValue::Number(snapshot.latency_hist[3] as f64),
        );
        rt_json::JsonValue::Object(h)
    };

    if diagnostics_json {
        let mut obj = BTreeMap::new();
        obj.insert(
            "active_tasks".to_string(),
            rt_json::JsonValue::Number(snapshot.active_tasks as f64),
        );
        obj.insert(
            "avg_latency_us".to_string(),
            rt_json::JsonValue::Number(snapshot.avg_latency_us),
        );
        obj.insert(
            "event".to_string(),
            rt_json::JsonValue::String("concurrency.snapshot".to_string()),
        );
        obj.insert("latency_hist".to_string(), build_hist());
        obj.insert(
            "queue_depth".to_string(),
            rt_json::JsonValue::Number(snapshot.queue_depth as f64),
        );
        obj.insert(
            "total_completed".to_string(),
            rt_json::JsonValue::Number(snapshot.total_completed as f64),
        );
        obj.insert(
            "total_spawned".to_string(),
            rt_json::JsonValue::Number(snapshot.total_spawned as f64),
        );
        obj.insert(
            "worker_count".to_string(),
            rt_json::JsonValue::Number(snapshot.worker_count as f64),
        );
        eprintln!("{}", rt_json::encode(&rt_json::JsonValue::Object(obj)));
    }

    if metrics_hook_mode() == MetricsHookMode::Stderr {
        let mut obj = BTreeMap::new();
        obj.insert(
            "active_tasks".to_string(),
            rt_json::JsonValue::Number(snapshot.active_tasks as f64),
        );
        obj.insert(
            "avg_latency_us".to_string(),
            rt_json::JsonValue::Number(snapshot.avg_latency_us),
        );
        obj.insert("latency_hist".to_string(), build_hist());
        obj.insert(
            "metric".to_string(),
            rt_json::JsonValue::String("concurrency.snapshot".to_string()),
        );
        obj.insert(
            "queue_depth".to_string(),
            rt_json::JsonValue::Number(snapshot.queue_depth as f64),
        );
        obj.insert(
            "total_completed".to_string(),
            rt_json::JsonValue::Number(snapshot.total_completed as f64),
        );
        obj.insert(
            "total_spawned".to_string(),
            rt_json::JsonValue::Number(snapshot.total_spawned as f64),
        );
        obj.insert(
            "worker_count".to_string(),
            rt_json::JsonValue::Number(snapshot.worker_count as f64),
        );
        eprintln!(
            "metrics: {}",
            rt_json::encode(&rt_json::JsonValue::Object(obj))
        );
    }
}

fn structured_request_logging_enabled() -> bool {
    env_true("FUSE_REQUEST_LOG", &["structured", "json"])
}

fn env_true(key: &str, extra_truthy: &[&str]) -> bool {
    let Ok(raw) = std::env::var(key) else {
        return false;
    };
    let value = raw.trim().to_ascii_lowercase();
    if value == "1" || value == "true" {
        return true;
    }
    extra_truthy.iter().any(|candidate| value == *candidate)
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum MetricsHookMode {
    Off,
    Stderr,
}

fn metrics_hook_mode() -> MetricsHookMode {
    let Ok(raw) = std::env::var("FUSE_METRICS_HOOK") else {
        return MetricsHookMode::Off;
    };
    if raw.trim().eq_ignore_ascii_case("stderr") {
        MetricsHookMode::Stderr
    } else {
        MetricsHookMode::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_panic_payload_uses_deterministic_kind_for_static_str() {
        let payload: Box<dyn Any + Send> = Box::new("boom");
        let details = classify_panic_payload(payload.as_ref());
        assert_eq!(details.kind, "panic_static_str");
        assert_eq!(details.message, "boom");
        assert_eq!(
            format_panic_message(&details),
            "panic_kind=panic_static_str boom"
        );
    }

    #[test]
    fn classify_panic_payload_uses_deterministic_kind_for_string() {
        let payload: Box<dyn Any + Send> = Box::new(String::from("boom"));
        let details = classify_panic_payload(payload.as_ref());
        assert_eq!(details.kind, "panic_string");
        assert_eq!(details.message, "boom");
        assert_eq!(
            format_panic_message(&details),
            "panic_kind=panic_string boom"
        );
    }

    #[test]
    fn classify_panic_payload_uses_deterministic_kind_for_non_string_payload() {
        let payload: Box<dyn Any + Send> = Box::new(42u64);
        let details = classify_panic_payload(payload.as_ref());
        assert_eq!(details.kind, "panic_non_string");
        assert_eq!(details.message, "panic");
        assert_eq!(
            format_panic_message(&details),
            "panic_kind=panic_non_string panic"
        );
    }

    #[test]
    fn resolve_request_id_prefers_explicit_request_id_header() {
        let mut headers = HashMap::new();
        headers.insert("x-request-id".to_string(), "req-123".to_string());
        headers.insert("x-correlation-id".to_string(), "corr-456".to_string());
        assert_eq!(resolve_request_id(&headers), "req-123");
    }

    #[test]
    fn resolve_request_id_falls_back_to_correlation_id() {
        let mut headers = HashMap::new();
        headers.insert("x-correlation-id".to_string(), "corr-456".to_string());
        assert_eq!(resolve_request_id(&headers), "corr-456");
    }

    #[test]
    fn resolve_request_id_generates_runtime_id_when_missing_or_invalid() {
        let mut headers = HashMap::new();
        headers.insert("x-request-id".to_string(), "bad id".to_string());
        let resolved = resolve_request_id(&headers);
        assert!(resolved.starts_with("req-"), "resolved: {resolved}");
    }

    #[test]
    fn inject_request_id_header_replaces_existing_request_id_header() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nx-request-id: old\r\nContent-Length: 2\r\n\r\nok".to_string();
        let patched = inject_request_id_header(response, "req-abc");
        assert!(patched.contains("\r\nX-Request-Id: req-abc\r\n"));
        assert!(!patched.contains("x-request-id: old"));
        let (status, body_len) = parse_http_response_status_and_body_len(&patched);
        assert_eq!(status, 200);
        assert_eq!(body_len, 2);
    }
}
