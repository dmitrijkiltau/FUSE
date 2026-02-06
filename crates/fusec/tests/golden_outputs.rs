use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fuse_rt::json;

fn example_path(name: &str) -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path.to_string_lossy().to_string()
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind test port");
    listener.local_addr().unwrap().port()
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

    let request = format!(
        "GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
    );
    let (status, response_body) = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    assert_eq!(status, 404);
    assert_eq!(
        response_body,
        r#"{"error":{"code":"not_found","message":"not found"}}"#
    );
}
