use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

fn run_example(backend: &str, example: &str, envs: &[(&str, &str)]) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path(example));
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("failed to run fusec")
}

fn run_example_with_args(backend: &str, example: &str, args: &[&str]) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path(example))
        .arg("--");
    for arg in args {
        cmd.arg(arg);
    }
    cmd.output().expect("failed to run fusec")
}

fn run_example_with_stdin(backend: &str, example: &str, stdin_text: &str) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path(example))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run fusec");
    {
        let mut stdin = child.stdin.take().expect("missing child stdin");
        stdin
            .write_all(stdin_text.as_bytes())
            .expect("failed to write stdin");
    }
    child.wait_with_output().expect("failed to wait for fusec")
}

fn send_http_request_with_retry(port: u16, request: &str) -> (u16, String) {
    let start = Instant::now();
    loop {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                if let Err(err) = stream.write_all(request.as_bytes()) {
                    let last_error = format!("write failed: {err}");
                    if start.elapsed() > Duration::from_secs(2) {
                        panic!(
                            "server did not produce a stable response on 127.0.0.1:{port} (last error: {})",
                            last_error
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                stream.shutdown(std::net::Shutdown::Write).ok();
                let mut buffer = String::new();
                if let Err(err) = stream.read_to_string(&mut buffer) {
                    let last_error = format!("read failed: {err}");
                    if start.elapsed() > Duration::from_secs(2) {
                        panic!(
                            "server did not produce a stable response on 127.0.0.1:{port} (last error: {})",
                            last_error
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                if buffer.trim().is_empty() {
                    let last_error = "empty response";
                    if start.elapsed() > Duration::from_secs(2) {
                        panic!(
                            "server did not produce a stable response on 127.0.0.1:{port} (last error: {})",
                            last_error
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
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
            Err(err) => {
                let last_error = format!("connect failed: {err}");
                if start.elapsed() > Duration::from_secs(2) {
                    panic!(
                        "server did not start on 127.0.0.1:{port} (last error: {})",
                        last_error
                    );
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn run_http_example<F>(backend: &str, make_request: F) -> (u16, String)
where
    F: FnOnce(u16) -> String,
{
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(example_path("http_users.fuse"))
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");
    let request = make_request(port);
    let (status, body) = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    (status, body)
}

fn write_temp_program(name: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}.fuse"));
    fs::write(&path, contents).expect("failed to write temp program");
    path
}

fn run_http_program_request(
    backend: &str,
    program: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    let port = find_free_port();
    let program_path = write_temp_program("fuse_parity_http_status", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path)
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start server");
    let request = if let Some(body) = body {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    } else {
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    };
    let start = Instant::now();
    let (status, response_body) = loop {
        if let Some(_) = child.try_wait().expect("failed to poll child status") {
            let output = child
                .wait_with_output()
                .expect("failed to collect child output");
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("server exited before readiness (backend={backend}): {stderr}");
        }
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                stream
                    .write_all(request.as_bytes())
                    .expect("failed to write request");
                stream.shutdown(std::net::Shutdown::Write).ok();
                let mut response = String::new();
                stream
                    .read_to_string(&mut response)
                    .expect("failed to read response");
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
                break (status, body);
            }
            Err(_) => {
                if start.elapsed() > Duration::from_secs(4) {
                    panic!(
                        "server did not start on 127.0.0.1:{port} within timeout (backend={backend})"
                    );
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let _ = child.wait();
    let _ = fs::remove_file(&program_path);
    (status, response_body)
}

fn assert_http_error_status_case(
    case_name: &str,
    return_ty: &str,
    err_expr: &str,
    expected_status: u16,
    expected_code: &str,
    expected_message: Option<&str>,
) {
    let program = format!(
        r#"
requires network

import {{ BadRequest, Unauthorized, Forbidden, Conflict }} from "std.Error"

config App:
  port: Int = 0

type CustomErr:
  message: String

service Api at "":
  get "/case" -> {return_ty}:
    return null ?! {err_expr}

app "demo":
  serve(App.port)
"#
    );
    let ast = run_http_program_request("ast", &program, "GET", "/case", None);
    let native = run_http_program_request("native", &program, "GET", "/case", None);
    assert_eq!(ast, native, "case={case_name}");
    let (status, body) = ast;
    assert_eq!(status, expected_status, "case={case_name} body={body}");
    assert!(
        body.contains(&format!("\"code\":\"{expected_code}\"")),
        "case={case_name} body={body}"
    );
    if let Some(expected_message) = expected_message {
        assert!(
            body.contains(expected_message),
            "case={case_name} body={body}"
        );
    }
}

fn normalize_error(stderr: &str) -> String {
    let text = stderr.trim();
    let json_text = text.strip_prefix("run error: ").unwrap_or(text);
    if json_text.trim_start().starts_with('{') {
        let parsed = json::decode(json_text).expect("expected JSON error");
        return json::encode(&parsed);
    }
    json_text.trim().to_string()
}

#[test]
fn parity_cli_hello() {
    let envs = [("GREETING", "Hi"), ("NAME", "Codex")];
    let ast = run_example("ast", "cli_hello.fuse", &envs);
    let native = run_example("native", "cli_hello.fuse", &envs);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_interp_demo() {
    let ast = run_example("ast", "interp_demo.fuse", &[]);
    let native = run_example("native", "interp_demo.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_task_api() {
    let ast = run_example("ast", "task_api.fuse", &[]);
    let native = run_example("native", "task_api.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_box_shared() {
    let ast = run_example("ast", "box_shared.fuse", &[]);
    let native = run_example("native", "box_shared.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_assign_field() {
    let ast = run_example("ast", "assign_field.fuse", &[]);
    let native = run_example("native", "assign_field.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_assign_index() {
    let ast = run_example("ast", "assign_index.fuse", &[]);
    let native = run_example("native", "assign_index.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_range_demo() {
    let ast = run_example("ast", "range_demo.fuse", &[]);
    let native = run_example("native", "range_demo.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_float_compare() {
    let ast = run_example("ast", "float_compare.fuse", &[]);
    let native = run_example("native", "float_compare.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_cli_binding() {
    let args = ["--name=Codex", "--excited"];
    let ast = run_example_with_args("ast", "cli_args.fuse", &args);
    let native = run_example_with_args("native", "cli_args.fuse", &args);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_log_output_format() {
    let envs = [("FUSE_COLOR", "never")];
    let ast = run_example("ast", "log_parity.fuse", &envs);
    let native = run_example("native", "log_parity.fuse", &envs);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stderr),
        String::from_utf8_lossy(&native.stderr)
    );
    let log_text = String::from_utf8_lossy(&ast.stderr);
    assert!(
        log_text.contains("[INFO] text parity"),
        "stderr: {log_text}"
    );
    assert!(
        log_text.contains("\"level\":\"info\""),
        "stderr: {log_text}"
    );
    assert!(
        log_text.contains("\"message\":\"json parity\""),
        "stderr: {log_text}"
    );
}

#[test]
fn parity_spawn_error_propagation() {
    let ast = run_example("ast", "spawn_error.fuse", &[]);
    let native = run_example("native", "spawn_error.fuse", &[]);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!native.status.success(), "expected native failure");

    let ast_err = normalize_error(&String::from_utf8_lossy(&ast.stderr));
    let native_err = normalize_error(&String::from_utf8_lossy(&native.stderr));
    assert_eq!(ast_err, native_err);
}

#[test]
fn parity_cli_input_with_piped_stdin() {
    let ast = run_example_with_stdin("ast", "cli_input.fuse", "Codex\n");
    let native = run_example_with_stdin("native", "cli_input.fuse", "Codex\n");

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_cli_input_without_stdin_reports_stable_error() {
    let ast = run_example("ast", "cli_input.fuse", &[]);
    let native = run_example("native", "cli_input.fuse", &[]);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!native.status.success(), "expected native failure");

    let expected = "input requires stdin data in non-interactive mode";
    let ast_err = String::from_utf8_lossy(&ast.stderr);
    let native_err = String::from_utf8_lossy(&native.stderr);
    assert!(ast_err.contains(expected), "ast stderr: {ast_err}");
    assert!(native_err.contains(expected), "native stderr: {native_err}");
}

#[test]
fn parity_enum_match() {
    let ast = run_example("ast", "enum_match.fuse", &[]);
    let native = run_example("native", "enum_match.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_project_demo_error() {
    let envs = [("DEMO_FAIL", "1")];
    let ast = run_example("ast", "project_demo.fuse", &envs);
    let native = run_example("native", "project_demo.fuse", &envs);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!native.status.success(), "expected native failure");

    let ast_err = normalize_error(&String::from_utf8_lossy(&ast.stderr));
    let native_err = normalize_error(&String::from_utf8_lossy(&native.stderr));
    assert_eq!(ast_err, native_err);
}

#[test]
fn parity_db_query_builder() {
    let envs = [("FUSE_DB_URL", "sqlite::memory:")];
    let ast = run_example("ast", "db_query_builder.fuse", &envs);
    let native = run_example("native", "db_query_builder.fuse", &envs);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_http_users_get_not_found() {
    if skip_if_loopback_unavailable("parity_http_users_get_not_found") {
        return;
    }
    let ast = run_http_example("ast", |port| {
        format!("GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    });
    let native = run_http_example("native", |port| {
        format!("GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    });
    assert_eq!(ast, native);
}

#[test]
fn parity_http_users_post_ok() {
    if skip_if_loopback_unavailable("parity_http_users_post_ok") {
        return;
    }
    let body = r#"{"id":"u1","email":"ada@example.com","name":"Ada"}"#;
    let ast = run_http_example("ast", |port| {
        format!(
            "POST /api/users HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    });
    let native = run_http_example("native", |port| {
        format!(
            "POST /api/users HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    });
    assert_eq!(ast, native);
}

#[test]
fn parity_http_error_status_matrix() {
    if skip_if_loopback_unavailable("parity_http_error_status_matrix") {
        return;
    }
    for (case_name, return_ty, err_expr, expected_status, expected_code, expected_message) in [
        (
            "bad_request",
            "String!BadRequest",
            "BadRequest(message=\"bad request\")",
            400,
            "bad_request",
            Some("bad request"),
        ),
        (
            "unauthorized",
            "String!Unauthorized",
            "Unauthorized(message=\"unauthorized\")",
            401,
            "unauthorized",
            Some("unauthorized"),
        ),
        (
            "forbidden",
            "String!Forbidden",
            "Forbidden(message=\"forbidden\")",
            403,
            "forbidden",
            Some("forbidden"),
        ),
        (
            "conflict",
            "String!Conflict",
            "Conflict(message=\"conflict\")",
            409,
            "conflict",
            Some("conflict"),
        ),
        (
            "std_error_override",
            "String!Error",
            "Error(code=\"teapot\", message=\"brew\", status=418)",
            418,
            "teapot",
            Some("brew"),
        ),
        (
            "unknown_error",
            "String!CustomErr",
            "CustomErr(message=\"boom\")",
            500,
            "internal_error",
            Some("internal error"),
        ),
    ] {
        assert_http_error_status_case(
            case_name,
            return_ty,
            err_expr,
            expected_status,
            expected_code,
            expected_message,
        );
    }
}
