use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fuse_rt::json;
mod support;
use support::net::{find_free_port, skip_if_loopback_unavailable};

fn example_path(name: &str) -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path.to_string_lossy().to_string()
}

fn send_http_request_with_retry(port: u16, request: &str) -> (u16, String) {
    let start = Instant::now();
    loop {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                stream
                    .write_all(request.as_bytes())
                    .expect("failed to write request");
                stream.shutdown(std::net::Shutdown::Write).ok();
                let mut buffer = String::new();
                stream
                    .read_to_string(&mut buffer)
                    .expect("failed to read response");
                let mut lines = buffer.split("\r\n");
                let status_line = lines.next().unwrap_or("");
                let status = status_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("500")
                    .parse::<u16>()
                    .unwrap_or(500);
                let body = buffer
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                return (status, body);
            }
            Err(_) => {
                if start.elapsed() > Duration::from_secs(2) {
                    panic!("server did not start on 127.0.0.1:{port}");
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

#[test]
fn golden_cli_hello_ast_stdout() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(example_path("cli_hello.fuse"))
        .env("GREETING", "Hi")
        .env("NAME", "Codex")
        .output()
        .expect("failed to run fusec");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Hi, Codex!");
}

#[test]
fn golden_project_demo_error_json() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(example_path("project_demo.fuse"))
        .env("DEMO_FAIL", "1")
        .output()
        .expect("failed to run fusec");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_text = stderr
        .trim()
        .strip_prefix("run error: ")
        .unwrap_or(stderr.trim());
    let json_value = json::decode(json_text).expect("expected JSON error");
    let rendered = json::encode(&json_value);
    let expected = r#"{"error":{"code":"validation_error","fields":[{"code":"invalid_value","message":"length 0 out of range 1..80","path":"User.name"}],"message":"validation failed"}}"#;
    assert_eq!(rendered, expected);
}

#[test]
fn golden_project_demo_error_json_native() {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("native")
        .arg(example_path("project_demo.fuse"))
        .env("DEMO_FAIL", "1")
        .output()
        .expect("failed to run fusec");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_text = stderr
        .trim()
        .strip_prefix("run error: ")
        .unwrap_or(stderr.trim());
    let json_value = json::decode(json_text).expect("expected JSON error");
    let rendered = json::encode(&json_value);
    let expected = r#"{"error":{"code":"validation_error","fields":[{"code":"invalid_value","message":"length 0 out of range 1..80","path":"User.name"}],"message":"validation failed"}}"#;
    assert_eq!(rendered, expected);
}

#[test]
fn golden_http_users_post_ok() {
    if skip_if_loopback_unavailable("golden_http_users_post_ok") {
        return;
    }
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(example_path("http_users.fuse"))
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");

    let body = r#"{"id":"u1","email":"ada@example.com","name":"Ada"}"#;
    let request = format!(
        "POST /api/users HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let (status, response_body) = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    assert_eq!(status, 200);
    assert_eq!(
        response_body,
        r#"{"email":"ada@example.com","id":"u1","name":"Ada"}"#
    );
}

#[test]
fn golden_http_users_get_not_found() {
    if skip_if_loopback_unavailable("golden_http_users_get_not_found") {
        return;
    }
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(example_path("http_users.fuse"))
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");

    let request = format!("GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let (status, response_body) = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    assert_eq!(status, 404);
    assert_eq!(
        response_body,
        r#"{"error":{"code":"not_found","message":"not found"}}"#
    );
}

#[test]
fn golden_http_body_structured_and_bytes_validation() {
    if skip_if_loopback_unavailable("golden_http_body_structured_and_bytes_validation") {
        return;
    }
    let port = find_free_port();
    let src = r#"
requires network

config App:
  port: Int = 0

type Payload:
  names: List<String>
  labels: Map<String, Int>
  who: User
  blob: Bytes

type User:
  name: String

service Api at "":
  post "/decode" body Payload -> Payload:
    return body

app "demo":
  serve(App.port)
"#;
    let mut path = std::env::temp_dir();
    path.push(format!("fuse_http_decode_{}.fuse", port));
    std::fs::write(&path, src).expect("write temp program");

    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&path)
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "2")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");

    let ok_body = r#"{"blob":"YQ==","labels":{"x":1},"names":["Ada"],"who":{"name":"Ada"}}"#;
    let ok_req = format!(
        "POST /decode HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        ok_body.len(),
        ok_body
    );
    let (ok_status, ok_resp) = send_http_request_with_retry(port, &ok_req);
    assert_eq!(ok_status, 200);
    assert_eq!(ok_resp, ok_body);

    let bad_body = r#"{"names":["Ada"],"labels":{"x":1},"who":{"name":"Ada"},"blob":"not_base64"}"#;
    let bad_req = format!(
        "POST /decode HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        bad_body.len(),
        bad_body
    );
    let (bad_status, bad_resp) = send_http_request_with_retry(port, &bad_req);
    let _ = child.wait();
    assert_eq!(bad_status, 400);
    assert_eq!(
        bad_resp,
        r#"{"error":{"code":"validation_error","fields":[{"code":"invalid_value","message":"invalid Bytes (base64): invalid base64 length","path":"body.blob"}],"message":"validation failed"}}"#
    );
}
