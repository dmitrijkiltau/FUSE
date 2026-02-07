use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    let (status, body) = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    (status, body)
}

#[test]
fn native_http_users_post_ok() {
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
    let (status, response_body) = run_http_example(|port| {
        format!("GET /api/users/42 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
    });
    assert_eq!(status, 404);
    assert_eq!(
        response_body,
        r#"{"error":{"code":"not_found","message":"not found"}}"#
    );
}
