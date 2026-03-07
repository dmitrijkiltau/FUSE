use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fuse_rt::json;
mod support;
use support::http::{
    DelayedHttpExchange, ScriptedHttpExchange, send_http_request_status_body_with_retry,
    spawn_scripted_https_server,
    spawn_delayed_http_server,
    spawn_handshake_only_https_server, spawn_scripted_http_server,
};
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

fn parity_http_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn run_http_example<F>(backend: &str, make_request: F) -> (u16, String)
where
    F: Fn(u16) -> String,
{
    let _lock = parity_http_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let exe = env!("CARGO_BIN_EXE_fusec");
    for attempt in 0..8 {
        let port = find_free_port();
        let mut child = Command::new(exe)
            .arg("--run")
            .arg("--backend")
            .arg(backend)
            .arg(example_path("http_users.fuse"))
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
            panic!("server exited before readiness (backend={backend}): {stderr}");
        }
        let request = make_request(port);
        let (status, body) = send_http_request_status_body_with_retry(port, &request);
        let output = child.wait_with_output().expect("failed to wait for server");
        assert!(
            output.status.success(),
            "server exited with failure (backend={backend}): {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return (status, body);
    }
    panic!("failed to start server after retries (backend={backend})");
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

fn run_temp_program(backend: &str, source: &str, envs: &[(&str, &str)]) -> std::process::Output {
    let program_path = write_temp_program("fuse_parity_temp", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("failed to run fusec");
    let _ = fs::remove_file(&program_path);
    output
}

fn write_temp_pem(name: &str, contents: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("{name}_{stamp}.pem"));
    fs::write(&path, contents).expect("failed to write temp pem");
    path
}

fn run_http_program_request(
    backend: &str,
    program: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    let _lock = parity_http_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let program_path = write_temp_program("fuse_parity_http_status", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut last_error = String::from("unknown");
    'attempts: for attempt in 0..8 {
        let port = find_free_port();
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
        loop {
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
                    last_error = stderr;
                    continue 'attempts;
                }
                let _ = fs::remove_file(&program_path);
                panic!("server exited before readiness (backend={backend}): {stderr}");
            }
            match TcpStream::connect(format!("127.0.0.1:{port}")) {
                Ok(mut stream) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    if let Err(err) = stream.write_all(request.as_bytes()) {
                        last_error = format!("write failed: {err}");
                        if start.elapsed() > Duration::from_secs(4) {
                            let _ = fs::remove_file(&program_path);
                            panic!(
                                "server did not produce a stable response on 127.0.0.1:{port} (backend={backend}, last error: {last_error})"
                            );
                        }
                        thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                    stream.shutdown(std::net::Shutdown::Write).ok();
                    let mut response = String::new();
                    if let Err(err) = stream.read_to_string(&mut response) {
                        last_error = format!("read failed: {err}");
                        if start.elapsed() > Duration::from_secs(4) {
                            let _ = fs::remove_file(&program_path);
                            panic!(
                                "server did not produce a stable response on 127.0.0.1:{port} (backend={backend}, last error: {last_error})"
                            );
                        }
                        thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                    if response.trim().is_empty() {
                        last_error = "empty response".to_string();
                        if start.elapsed() > Duration::from_secs(4) {
                            let _ = fs::remove_file(&program_path);
                            panic!(
                                "server did not produce a stable response on 127.0.0.1:{port} (backend={backend}, last error: {last_error})"
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
                    let output = child.wait_with_output().expect("failed to wait for server");
                    assert!(
                        output.status.success(),
                        "server exited with failure (backend={backend}): {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let _ = fs::remove_file(&program_path);
                    return (status, body);
                }
                Err(err) => {
                    last_error = format!("connect failed: {err}");
                    if start.elapsed() > Duration::from_secs(4) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = fs::remove_file(&program_path);
    panic!(
        "server did not start within timeout after retries (backend={backend}, last error: {last_error})"
    );
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

fn normalize_tls_output_port(text: &str) -> String {
    let prefix = "TLS:tls_error:get https://127.0.0.1:";
    let Some(rest) = text.strip_prefix(prefix) else {
        return text.to_string();
    };
    let Some((_, suffix)) = rest.split_once('/') else {
        return text.to_string();
    };
    format!("{prefix}<port>/{suffix}")
}

fn normalize_http_client_loopback_port(text: &str) -> String {
    let normalized_http = normalize_loopback_port_for_scheme(text, "http");
    normalize_loopback_port_for_scheme(&normalized_http, "https")
}

fn normalize_loopback_port_for_scheme(text: &str, scheme: &str) -> String {
    let needle = format!("{scheme}://127.0.0.1:");
    let Some(start) = text.find(&needle) else {
        return text.to_string();
    };
    let port_start = start + needle.len();
    let port_end = text[port_start..]
        .find('/')
        .map(|offset| port_start + offset)
        .unwrap_or(text.len());
    let mut normalized = String::with_capacity(text.len());
    normalized.push_str(&text[..port_start]);
    normalized.push_str("<port>");
    normalized.push_str(&text[port_end..]);
    normalized
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
fn parity_time_and_crypto_runtime_slice() {
    let program = r#"
requires time
requires crypto

app "demo":
  let now = time.now()
  assert(now > 0, "time.now should return positive unix ms")
  time.sleep(1)

  let payload = crypto.random_bytes(8)
  let key = crypto.random_bytes(4)
  let sha256 = crypto.hash("sha256", payload)
  let sha512 = crypto.hash("sha512", payload)
  let hmac512 = crypto.hmac("sha512", key, payload)
  let random = crypto.random_bytes(16)

  assert(crypto.constant_time_eq(sha256, sha256), "hash self-equality failed")
  assert(!crypto.constant_time_eq(sha256, sha512), "different digest lengths should differ")
  assert(!crypto.constant_time_eq(sha256, hmac512), "different digest lengths should differ")
  assert(crypto.constant_time_eq(random, random), "random self-equality failed")
  assert(!crypto.constant_time_eq(random, payload), "different lengths should differ")

  print("ok")
"#;
    let ast = run_temp_program("ast", program, &[]);
    let native = run_temp_program("native", program, &[]);

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
    assert_eq!(String::from_utf8_lossy(&ast.stdout), "ok\n");
}

#[test]
fn parity_crypto_hash_invalid_algorithm_error() {
    let program = r#"
requires crypto

app "demo":
  let payload = crypto.random_bytes(8)
  crypto.hash("md5", payload)
"#;
    let ast = run_temp_program("ast", program, &[]);
    let native = run_temp_program("native", program, &[]);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!native.status.success(), "expected native failure");

    let ast_err = normalize_error(&String::from_utf8_lossy(&ast.stderr));
    let native_err = normalize_error(&String::from_utf8_lossy(&native.stderr));
    assert_eq!(ast_err, native_err);
    assert!(
        ast_err.contains("crypto.hash unsupported algorithm md5"),
        "stderr: {ast_err}"
    );
}

#[test]
fn parity_time_format_parse_roundtrip() {
    let program = r#"
requires time

app "demo":
  let epoch = 1704067200123
  let formatted = time.format(epoch, "%Y-%m-%d %H:%M:%S")
  print(formatted)
  let parsed = time.parse(formatted, "%Y-%m-%d %H:%M:%S")
  match parsed:
    Ok(v) -> print(v)
    Err(e) -> print(e.message)
"#;
    let ast = run_temp_program("ast", program, &[]);
    let native = run_temp_program("native", program, &[]);

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
    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        "2024-01-01 00:00:00\n1704067200000\n"
    );
}

#[test]
fn parity_time_parse_invalid_returns_error_result() {
    let program = r#"
requires time

app "demo":
  let parsed = time.parse("not-a-date", "%Y-%m-%d")
  match parsed:
    Ok(v) -> print("unexpected")
    Err(e) -> print(e.message)
"#;
    let ast = run_temp_program("ast", program, &[]);
    let native = run_temp_program("native", program, &[]);

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
    assert!(
        String::from_utf8_lossy(&ast.stdout).contains("time.parse failed for format"),
        "stdout: {}",
        String::from_utf8_lossy(&ast.stdout)
    );
}

#[test]
fn parity_http_client_roundtrip_across_backends() {
    if skip_if_loopback_unavailable("parity_http_client_roundtrip_across_backends") {
        return;
    }
    let program = r#"
requires network

app "demo":
  let get_result = http.get(env("UPSTREAM_GET") ?? "", {"x-trace": "abc"}, 1000)
  match get_result:
    Ok(resp):
      print("GET:${resp.status}:${resp.body}")
    Err(err):
      print("GETERR:${err.code}:${err.status ?? 0}:${err.body ?? ""}")

  let post_result = http.post(
    env("UPSTREAM_POST") ?? "",
    "{\"name\":\"Ada\"}",
    {"content-type": "application/json", "x-extra": "1"},
    1000
  )
  match post_result:
    Ok(resp):
      print("POST:${resp.status}:${resp.body}")
    Err(err):
      print("POSTERR:${err.code}:${err.status ?? 0}:${err.body ?? ""}")
"#;
    let mut outputs = Vec::new();
    for backend in ["ast", "native"] {
        let (port, server) = spawn_scripted_http_server(vec![
            ScriptedHttpExchange {
                request_line: "GET /ok HTTP/1.1".to_string(),
                request_contains: vec!["x-trace: abc".to_string()],
                response: "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Upstream: yes\r\nContent-Length: 2\r\n\r\nok".to_string(),
            },
            ScriptedHttpExchange {
                request_line: "POST /submit HTTP/1.1".to_string(),
                request_contains: vec![
                    "content-type: application/json".to_string(),
                    "x-extra: 1".to_string(),
                    "{\"name\":\"Ada\"}".to_string(),
                ],
                response: "HTTP/1.1 201 Created\r\nContent-Length: 7\r\n\r\ncreated".to_string(),
            },
        ]);
        let upstream_get = format!("http://127.0.0.1:{port}/ok");
        let upstream_post = format!("http://127.0.0.1:{port}/submit");
        let output = run_temp_program(
            backend,
            program,
            &[("UPSTREAM_GET", upstream_get.as_str()), ("UPSTREAM_POST", upstream_post.as_str())],
        );
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        server.join().expect("join scripted upstream server");
        outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
    }

    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], "GET:200:ok\nPOST:201:created\n");
}

#[test]
fn parity_http_client_error_results_across_backends() {
    if skip_if_loopback_unavailable("parity_http_client_error_results_across_backends") {
        return;
    }
    let program = r#"
requires network

fn missing_line() -> String:
    let missing = http.get(env("UPSTREAM_MISSING") ?? "", {}, 1000)
    match missing:
        Ok(resp):
            return "unexpected"
        Err(err):
            let missing_body = err.body ?? ""
            return "STATUS:${err.code}:${err.status ?? 0}:${missing_body}"

fn tls_line() -> String:
    let tls = http.get(env("UPSTREAM_TLS") ?? "")
    match tls:
        Ok(resp):
            return "unexpected"
        Err(err):
            return "TLS:${err.code}"

app "demo":
    print(missing_line())
    print(tls_line())
"#;
    let mut outputs = Vec::new();
    for backend in ["ast", "native"] {
        let (port, server) = spawn_scripted_http_server(vec![ScriptedHttpExchange {
            request_line: "GET /missing HTTP/1.1".to_string(),
            request_contains: Vec::new(),
            response: "HTTP/1.1 404 Not Found\r\nContent-Length: 7\r\n\r\nmissing".to_string(),
        }]);
        let (tls_port, _cert_pem, tls_server) = spawn_handshake_only_https_server();
        let upstream_missing = format!("http://127.0.0.1:{port}/missing");
        let upstream_tls = format!("https://127.0.0.1:{tls_port}/tls");
        let output = run_temp_program(
            backend,
            program,
            &[
                ("UPSTREAM_MISSING", upstream_missing.as_str()),
                ("UPSTREAM_TLS", upstream_tls.as_str()),
            ],
        );
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        server.join().expect("join scripted upstream server");
        tls_server.join().expect("join scripted upstream tls server");
        outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
    }

    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], "STATUS:http_status:404:missing\nTLS:tls_error\n");
}

#[test]
fn parity_http_client_https_success_across_backends() {
    if skip_if_loopback_unavailable("parity_http_client_https_success_across_backends") {
        return;
    }
    let program = r#"
requires network

app "demo":
  let result = http.get(env("UPSTREAM_TLS") ?? "", {"x-test": "yes"}, 1000)
  match result:
    Ok(resp):
      print("HTTPS:${resp.status}:${resp.headers["x-reply"] ?? ""}:${resp.body}")
    Err(err):
      print("ERR:${err.code}")
"#;

    let mut outputs = Vec::new();
    for backend in ["ast", "native"] {
        let (tls_port, cert_pem, tls_server) =
            spawn_scripted_https_server(vec![ScriptedHttpExchange {
                request_line: "GET /secure HTTP/1.1".to_string(),
                request_contains: vec!["x-test: yes".to_string()],
                response: "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-Reply: ok\r\n\r\nhi"
                    .to_string(),
            }]);
        let cert_path = write_temp_pem("fuse_parity_https_root", &cert_pem);
        let upstream_tls = format!("https://127.0.0.1:{tls_port}/secure");
        let cert_path_text = cert_path.to_string_lossy().to_string();
        let output = run_temp_program(
            backend,
            program,
            &[
                ("UPSTREAM_TLS", upstream_tls.as_str()),
                ("FUSE_EXTRA_CA_CERT_FILE", cert_path_text.as_str()),
            ],
        );
        let _ = fs::remove_file(&cert_path);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        tls_server.join().expect("join scripted upstream tls server");
        outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
    }

    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], "HTTPS:200:ok:hi\n");
}

#[test]
fn parity_http_client_tls_diagnostics_match_across_backends() {
    if skip_if_loopback_unavailable("parity_http_client_tls_diagnostics_match_across_backends") {
        return;
    }
    let program = r#"
requires network

app "demo":
  let result = http.get(env("UPSTREAM_TLS") ?? "")
  match result:
    Ok(resp):
      print("unexpected")
    Err(err):
      print("TLS:${err.code}:${err.message}")
"#;

    let mut outputs = Vec::new();
    for backend in ["ast", "native"] {
        let (tls_port, _cert_pem, tls_server) = spawn_handshake_only_https_server();
        let upstream_tls = format!("https://127.0.0.1:{tls_port}/tls");
        let output = run_temp_program(backend, program, &[("UPSTREAM_TLS", upstream_tls.as_str())]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        tls_server.join().expect("join scripted upstream tls server");
        outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
    }

    assert_eq!(
        normalize_tls_output_port(&outputs[0]),
        normalize_tls_output_port(&outputs[1])
    );
    assert!(outputs[0].starts_with("TLS:tls_error:get https://127.0.0.1:"), "stdout: {}", outputs[0]);
    assert!(outputs[0].contains("TLS handshake failed:"), "stdout: {}", outputs[0]);
}

#[test]
fn parity_http_client_reserved_header_rejection() {
    let program = r#"
requires network

app "demo":
  let result = http.get("http://127.0.0.1:1/blocked", {"Host": "evil"}, 1000)
  match result:
    Ok(resp):
      print("unexpected")
    Err(err):
      print("${err.code}:${err.message}")
"#;

    let ast = run_temp_program("ast", program, &[]);
    let native = run_temp_program("native", program, &[]);
    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(native.status.success(), "native stderr: {}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&ast.stdout), String::from_utf8_lossy(&native.stdout));
    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        "invalid_request:http.* manages header host automatically\n"
    );
}

#[test]
fn parity_http_client_timeout_diagnostics_match_across_backends() {
    if skip_if_loopback_unavailable("parity_http_client_timeout_diagnostics_match_across_backends") {
        return;
    }
    let program = r#"
requires network

app "demo":
  let result = http.get(env("UPSTREAM_SLOW") ?? "", {}, 50)
  match result:
    Ok(resp):
      print("unexpected")
    Err(err):
      print("TIMEOUT:${err.code}:${err.message}")
"#;

    let mut outputs = Vec::new();
    for backend in ["ast", "native"] {
        let (slow_port, slow_server) = spawn_delayed_http_server(DelayedHttpExchange {
            request_line: "GET /slow HTTP/1.1".to_string(),
            request_contains: Vec::new(),
            response: "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nslow".to_string(),
            delay: Duration::from_millis(150),
        });
        let upstream_slow = format!("http://127.0.0.1:{slow_port}/slow");
        let output = run_temp_program(backend, program, &[("UPSTREAM_SLOW", upstream_slow.as_str())]);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        slow_server.join().expect("join slow upstream server");
        outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
    }

    assert_eq!(
        normalize_http_client_loopback_port(&outputs[0]),
        normalize_http_client_loopback_port(&outputs[1])
    );
    assert!(outputs[0].contains("TIMEOUT:timeout:"), "stdout: {}", outputs[0]);
    assert!(outputs[0].contains("timed out during read after 50ms"), "stdout: {}", outputs[0]);
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
