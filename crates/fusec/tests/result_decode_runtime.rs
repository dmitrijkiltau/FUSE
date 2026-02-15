use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind free port");
    listener.local_addr().expect("missing local addr").port()
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

fn run_decode_request(backend: &str, payload: &str) -> (u16, String) {
    let port = find_free_port();
    let program = r#"
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
    let path = write_temp_file("fuse_result_decode_runtime", "fuse", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&path)
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");

    let request = format!(
        "POST /decode HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    );
    let out = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    out
}

#[test]
fn tagged_result_json_decode_supports_ok_all_backends() {
    let payload = r#"{"type":"Ok","data":{"name":"Ada"}}"#;
    for backend in ["ast", "vm", "native"] {
        let (status, body) = run_decode_request(backend, payload);
        assert_eq!(status, 200, "backend={backend} body={body}");
        assert_eq!(body, r#""ok:Ada""#, "backend={backend}");
    }
}

#[test]
fn tagged_result_json_decode_supports_err_all_backends() {
    let payload = r#"{"type":"Err","data":"boom"}"#;
    for backend in ["ast", "vm", "native"] {
        let (status, body) = run_decode_request(backend, payload);
        assert_eq!(status, 200, "backend={backend} body={body}");
        assert_eq!(body, r#""err:boom""#, "backend={backend}");
    }
}

#[test]
fn tagged_result_json_decode_rejects_missing_or_invalid_tag_all_backends() {
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
        for backend in ["ast", "vm", "native"] {
            let (status, body) = run_decode_request(backend, payload);
            assert_eq!(status, 400, "case={name} backend={backend} body={body}");
            assert!(
                body.contains(expected),
                "case={name} backend={backend} body={body}"
            );
        }
    }
}
