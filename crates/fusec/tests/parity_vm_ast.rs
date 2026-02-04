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

fn run_example(
    backend: &str,
    example: &str,
    envs: &[(&str, &str)],
) -> std::process::Output {
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
    let parsed = json::decode(json_text).expect("expected JSON error");
    json::encode(&parsed)
}

#[test]
fn parity_cli_hello() {
    let envs = [("GREETING", "Hi"), ("NAME", "Codex")];
    let ast = run_example("ast", "cli_hello.fuse", &envs);
    let vm = run_example("vm", "cli_hello.fuse", &envs);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_interp_demo() {
    let ast = run_example("ast", "interp_demo.fuse", &[]);
    let vm = run_example("vm", "interp_demo.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_spawn_await_box() {
    let ast = run_example("ast", "spawn_await_box.fuse", &[]);
    let vm = run_example("vm", "spawn_await_box.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_task_api() {
    let ast = run_example("ast", "task_api.fuse", &[]);
    let vm = run_example("vm", "task_api.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_box_shared() {
    let ast = run_example("ast", "box_shared.fuse", &[]);
    let vm = run_example("vm", "box_shared.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_assign_field() {
    let ast = run_example("ast", "assign_field.fuse", &[]);
    let vm = run_example("vm", "assign_field.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_assign_index() {
    let ast = run_example("ast", "assign_index.fuse", &[]);
    let vm = run_example("vm", "assign_index.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_range_demo() {
    let ast = run_example("ast", "range_demo.fuse", &[]);
    let vm = run_example("vm", "range_demo.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_cli_binding() {
    let args = ["--name=Codex", "--excited"];
    let ast = run_example_with_args("ast", "cli_args.fuse", &args);
    let vm = run_example_with_args("vm", "cli_args.fuse", &args);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

    assert_eq!(
        String::from_utf8_lossy(&ast.stdout),
        String::from_utf8_lossy(&vm.stdout)
    );
}

#[test]
fn parity_enum_match() {
    let ast = run_example("ast", "enum_match.fuse", &[]);
    let vm = run_example("vm", "enum_match.fuse", &[]);

    assert!(ast.status.success(), "ast stderr: {}", String::from_utf8_lossy(&ast.stderr));
    assert!(vm.status.success(), "vm stderr: {}", String::from_utf8_lossy(&vm.stderr));

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
fn parity_http_users_get_not_found() {
    let ast = run_http_example("ast", |port| {
        format!(
            "GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        )
    });
    let vm = run_http_example("vm", |port| {
        format!(
            "GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        )
    });
    assert_eq!(ast, vm);
}

#[test]
fn parity_http_users_post_ok() {
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
