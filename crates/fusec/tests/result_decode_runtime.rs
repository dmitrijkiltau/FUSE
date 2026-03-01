use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod support;
use support::net::{find_free_port, skip_if_loopback_unavailable};

fn write_temp_file(name: &str, ext: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}.{ext}"));
    fs::write(&path, contents).expect("failed to write temp file");
    path
}

fn send_http_request_with_retry(port: u16, request: &str) -> (u16, String) {
    let start = Instant::now();
    loop {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                if let Err(err) = stream.write_all(request.as_bytes()) {
                    if start.elapsed() > Duration::from_secs(3) {
                        panic!(
                            "server did not produce a stable response on 127.0.0.1:{port} (last error: write failed: {err})"
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                stream.shutdown(std::net::Shutdown::Write).ok();
                let mut response = String::new();
                if let Err(err) = stream.read_to_string(&mut response) {
                    if start.elapsed() > Duration::from_secs(3) {
                        panic!(
                            "server did not produce a stable response on 127.0.0.1:{port} (last error: read failed: {err})"
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                if response.trim().is_empty() {
                    if start.elapsed() > Duration::from_secs(3) {
                        panic!(
                            "server did not produce a stable response on 127.0.0.1:{port} (last error: empty response)"
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                let mut lines = response.split("\r\n");
                let status_line = lines.next().unwrap_or("");
                let status = status_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("500")
                    .parse::<u16>()
                    .unwrap_or(500);
                let body = response
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                return (status, body);
            }
            Err(err) => {
                if start.elapsed() > Duration::from_secs(3) {
                    panic!("server did not start on 127.0.0.1:{port} (last error: {err})");
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn decode_http_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn run_decode_request(backend: &str, payload: &str) -> (u16, String) {
    let program = r#"
requires network

config App:
  port: Int = 0

type User:
  name: String

service Api at "":
  post "/decode" body Result<User, String> -> String:
    match body:
      Ok(user) -> "ok:${user.name}"
      Err(msg) -> "err:${msg}"

app "demo":
  serve(App.port)
"#;
    run_decode_request_with_program(backend, program, payload)
}

fn run_decode_request_with_program(backend: &str, program: &str, payload: &str) -> (u16, String) {
    let _lock = decode_http_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = write_temp_file("fuse_result_decode_runtime", "fuse", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    for attempt in 0..8 {
        let port = find_free_port();
        let mut child = Command::new(exe)
            .arg("--run")
            .arg("--backend")
            .arg(backend)
            .arg(&path)
            .env("APP_PORT", port.to_string())
            .env("FUSE_MAX_REQUESTS", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start server");

        thread::sleep(Duration::from_millis(20));
        if child
            .try_wait()
            .expect("failed to poll child status")
            .is_some()
        {
            let output = child
                .wait_with_output()
                .expect("failed to collect child output");
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if stderr.contains("Address already in use") && attempt < 7 {
                continue;
            }
            let _ = fs::remove_file(&path);
            panic!("server exited before readiness (backend={backend}): {stderr}");
        }

        let request = format!(
            "POST /decode HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            payload.len(),
            payload
        );
        let out = send_http_request_with_retry(port, &request);
        let output = child.wait_with_output().expect("failed to wait for server");
        assert!(
            output.status.success(),
            "server exited with failure (backend={backend}): {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = fs::remove_file(&path);
        return out;
    }
    let _ = fs::remove_file(&path);
    panic!("failed to start server after retries (backend={backend})");
}

#[test]
fn tagged_result_json_decode_supports_ok_all_backends() {
    if skip_if_loopback_unavailable("tagged_result_json_decode_supports_ok_all_backends") {
        return;
    }
    let payload = r#"{"type":"Ok","data":{"name":"Ada"}}"#;
    for backend in ["ast", "native"] {
        let (status, body) = run_decode_request(backend, payload);
        assert_eq!(status, 200, "backend={backend} body={body}");
        assert_eq!(body, r#""ok:Ada""#, "backend={backend}");
    }
}

#[test]
fn tagged_result_json_decode_supports_err_all_backends() {
    if skip_if_loopback_unavailable("tagged_result_json_decode_supports_err_all_backends") {
        return;
    }
    let payload = r#"{"type":"Err","data":"boom"}"#;
    for backend in ["ast", "native"] {
        let (status, body) = run_decode_request(backend, payload);
        assert_eq!(status, 200, "backend={backend} body={body}");
        assert_eq!(body, r#""err:boom""#, "backend={backend}");
    }
}

#[test]
fn tagged_result_json_decode_rejects_missing_or_invalid_tag_all_backends() {
    if skip_if_loopback_unavailable(
        "tagged_result_json_decode_rejects_missing_or_invalid_tag_all_backends",
    ) {
        return;
    }
    for (name, payload, expected) in [
        (
            "missing-tag",
            r#"{"data":{"name":"Ada"}}"#,
            "missing Result tag",
        ),
        (
            "invalid-tag",
            r#"{"type":"Nope","data":{"name":"Ada"}}"#,
            "unknown Result variant",
        ),
    ] {
        for backend in ["ast", "native"] {
            let (status, body) = run_decode_request(backend, payload);
            assert_eq!(status, 400, "case={name} backend={backend} body={body}");
            assert!(
                body.contains(expected),
                "case={name} backend={backend} body={body}"
            );
        }
    }
}

#[test]
fn http_decode_missing_field_uses_default_before_validation_all_backends() {
    if skip_if_loopback_unavailable(
        "http_decode_missing_field_uses_default_before_validation_all_backends",
    ) {
        return;
    }
    let program = r#"
requires network

config App:
  port: Int = 0

type Payload:
  name: String(1..20) = "anon"

service Api at "":
  post "/decode" body Payload -> String:
    return body.name

app "demo":
  serve(App.port)
"#;
    for backend in ["ast", "native"] {
        let (status, body) = run_decode_request_with_program(backend, program, r#"{}"#);
        assert_eq!(status, 200, "backend={backend} body={body}");
        assert_eq!(body, r#""anon""#, "backend={backend} body={body}");
    }
}

#[test]
fn http_decode_unknown_field_rejected_all_backends() {
    if skip_if_loopback_unavailable("http_decode_unknown_field_rejected_all_backends") {
        return;
    }
    let program = r#"
requires network

config App:
  port: Int = 0

type Payload:
  name: String(1..20) = "anon"

service Api at "":
  post "/decode" body Payload -> String:
    return body.name

app "demo":
  serve(App.port)
"#;
    let payload = r#"{"name":"Ada","extra":"boom"}"#;
    for backend in ["ast", "native"] {
        let (status, body) = run_decode_request_with_program(backend, program, payload);
        assert_eq!(status, 400, "backend={backend} body={body}");
        assert!(
            body.contains("\"code\":\"validation_error\""),
            "backend={backend} body={body}"
        );
        assert!(
            body.contains("unknown field"),
            "backend={backend} body={body}"
        );
    }
}
