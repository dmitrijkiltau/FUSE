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
    let vm = run_example("vm", "cli_hello.fuse", &envs);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_interp_demo() {
    let ast = run_example("ast", "interp_demo.fuse", &[]);
    let vm = run_example("vm", "interp_demo.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_vm_native_interp_demo() {
    let vm = run_example("vm", "interp_demo.fuse", &[]);
    let native = run_example("native", "interp_demo.fuse", &[]);

    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_task_api() {
    let ast = run_example("ast", "task_api.fuse", &[]);
    let vm = run_example("vm", "task_api.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_box_shared() {
    let ast = run_example("ast", "box_shared.fuse", &[]);
    let vm = run_example("vm", "box_shared.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_assign_field() {
    let ast = run_example("ast", "assign_field.fuse", &[]);
    let vm = run_example("vm", "assign_field.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_assign_index() {
    let ast = run_example("ast", "assign_index.fuse", &[]);
    let vm = run_example("vm", "assign_index.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_range_demo() {
    let ast = run_example("ast", "range_demo.fuse", &[]);
    let vm = run_example("vm", "range_demo.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_vm_native_range_demo() {
    let vm = run_example("vm", "range_demo.fuse", &[]);
    let native = run_example("native", "range_demo.fuse", &[]);

    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_vm_native_float_compare() {
    let vm = run_example("vm", "float_compare.fuse", &[]);
    let native = run_example("native", "float_compare.fuse", &[]);

    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_cli_binding() {
    let args = ["--name=Codex", "--excited"];
    let ast = run_example_with_args("ast", "cli_args.fuse", &args);
    let vm = run_example_with_args("vm", "cli_args.fuse", &args);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_vm_native_cli_binding() {
    let args = ["--name=Codex", "--excited"];
    let vm = run_example_with_args("vm", "cli_args.fuse", &args);
    let native = run_example_with_args("native", "cli_args.fuse", &args);

    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_log_output_format() {
    let envs = [("FUSE_COLOR", "never")];
    let ast = run_example("ast", "log_parity.fuse", &envs);
    let vm = run_example("vm", "log_parity.fuse", &envs);
    let native = run_example("native", "log_parity.fuse", &envs);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stderr),
        String::from_utf8_lossy(&vm.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&vm.stderr),
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
    let vm = run_example("vm", "spawn_error.fuse", &[]);
    let native = run_example("native", "spawn_error.fuse", &[]);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!vm.status.success(), "expected vm failure");
    assert!(!native.status.success(), "expected native failure");

    let ast_err = normalize_error(&String::from_utf8_lossy(&ast.stderr));
    let vm_err = normalize_error(&String::from_utf8_lossy(&vm.stderr));
    let native_err = normalize_error(&String::from_utf8_lossy(&native.stderr));
    assert_eq!(ast_err, vm_err);
    assert_eq!(vm_err, native_err);
}

#[test]
fn parity_cli_input_with_piped_stdin() {
    let ast = run_example_with_stdin("ast", "cli_input.fuse", "Codex\n");
    let vm = run_example_with_stdin("vm", "cli_input.fuse", "Codex\n");

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_vm_native_cli_input_with_piped_stdin() {
    let vm = run_example_with_stdin("vm", "cli_input.fuse", "Codex\n");
    let native = run_example_with_stdin("native", "cli_input.fuse", "Codex\n");

    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout)
    );
}

#[test]
fn parity_cli_input_without_stdin_reports_stable_error() {
    let ast = run_example("ast", "cli_input.fuse", &[]);
    let vm = run_example("vm", "cli_input.fuse", &[]);
    let native = run_example("native", "cli_input.fuse", &[]);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!vm.status.success(), "expected vm failure");
    assert!(!native.status.success(), "expected native failure");

    let expected = "input requires stdin data in non-interactive mode";
    let ast_err = String::from_utf8_lossy(&ast.stderr);
    let vm_err = String::from_utf8_lossy(&vm.stderr);
    let native_err = String::from_utf8_lossy(&native.stderr);
    assert!(ast_err.contains(expected), "ast stderr: {ast_err}");
    assert!(vm_err.contains(expected), "vm stderr: {vm_err}");
    assert!(native_err.contains(expected), "native stderr: {native_err}");
}

#[test]
fn parity_enum_match() {
    let ast = run_example("ast", "enum_match.fuse", &[]);
    let vm = run_example("vm", "enum_match.fuse", &[]);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_project_demo_error() {
    let envs = [("DEMO_FAIL", "1")];
    let ast = run_example("ast", "project_demo.fuse", &envs);
    let vm = run_example("vm", "project_demo.fuse", &envs);

    assert!(!ast.status.success(), "expected ast failure");
    assert!(!vm.status.success(), "expected vm failure");

    let ast_err = normalize_error(&String::from_utf8_lossy(&ast.stderr));
    let vm_err = normalize_error(&String::from_utf8_lossy(&vm.stderr));
    assert_eq!(ast_err, vm_err);
}

#[test]
fn parity_vm_native_project_demo_error() {
    let envs = [("DEMO_FAIL", "1")];
    let vm = run_example("vm", "project_demo.fuse", &envs);
    let native = run_example("native", "project_demo.fuse", &envs);

    assert!(!vm.status.success(), "expected vm failure");
    assert!(!native.status.success(), "expected native failure");

    let vm_err = normalize_error(&String::from_utf8_lossy(&vm.stderr));
    let native_err = normalize_error(&String::from_utf8_lossy(&native.stderr));
    assert_eq!(vm_err, native_err);
}

#[test]
fn parity_db_query_builder() {
    let envs = [("FUSE_DB_URL", "sqlite::memory:")];
    let ast = run_example("ast", "db_query_builder.fuse", &envs);
    let vm = run_example("vm", "db_query_builder.fuse", &envs);
    let native = run_example("native", "db_query_builder.fuse", &envs);

    assert!(
        ast.status.success(),
        "ast stderr: {}",
        String::from_utf8_lossy(&ast.stderr)
    );
    assert!(
        vm.status.success(),
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert!(
        native.status.success(),
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
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
    let vm = run_http_example("vm", |port| {
        format!("GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    });
    assert_eq!(ast, vm);
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
    let vm = run_http_example("vm", |port| {
        format!(
            "POST /api/users HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    });
    assert_eq!(ast, vm);
}
