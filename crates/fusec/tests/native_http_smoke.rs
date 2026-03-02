use std::process::{Command, Stdio};

mod support;
use support::http::send_http_request_status_body_with_retry;
use support::net::{find_free_port, skip_if_loopback_unavailable};

fn example_path(name: &str) -> String {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("examples");
    path.push(name);
    path.to_string_lossy().to_string()
}

fn run_http_example<F>(make_request: F) -> (u16, String)
where
    F: FnOnce(u16) -> String,
{
    let port = find_free_port();
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("native")
        .arg(example_path("http_users.fuse"))
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");
    let request = make_request(port);
    let (status, body) = send_http_request_status_body_with_retry(port, &request);
    let _ = child.wait();
    (status, body)
}

#[test]
fn native_http_users_post_ok() {
    if skip_if_loopback_unavailable("native_http_users_post_ok") {
        return;
    }
    let body = r#"{"id":"u1","email":"ada@example.com","name":"Ada"}"#;
    let (status, response_body) = run_http_example(|port| {
        format!(
            "POST /api/users HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    });
    assert_eq!(status, 200);
    assert_eq!(
        response_body,
        r#"{"email":"ada@example.com","id":"u1","name":"Ada"}"#
    );
}

#[test]
fn native_http_users_get_not_found() {
    if skip_if_loopback_unavailable("native_http_users_get_not_found") {
        return;
    }
    let (status, response_body) = run_http_example(|port| {
        format!("GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    });
    assert_eq!(status, 404);
    assert_eq!(
        response_body,
        r#"{"error":{"code":"not_found","message":"not found"}}"#
    );
}
