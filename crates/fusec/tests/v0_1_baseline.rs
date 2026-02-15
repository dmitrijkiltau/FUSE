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

fn temp_db_url() -> String {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.push(format!("fuse_v01_pool_{stamp}.sqlite"));
    format!("sqlite://{}", path.display())
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

#[test]
fn milestone0_refinement_regex_and_predicate_should_typecheck() {
    let program = r#"
fn is_slug(value: String) -> Bool:
  return value != ""

type Input:
  slug: String(regex("^[a-z0-9_-]+$"), predicate(is_slug))
"#;
    let path = write_temp_file("fuse_m0_refine", "fuse", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--check")
        .arg(&path)
        .output()
        .expect("failed to run fusec --check");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn milestone0_http_result_body_decode_tagged_ok_should_work() {
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
    let path = write_temp_file("fuse_m0_result_decode", "fuse", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut child = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&path)
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start server");

    let body = r#"{"type":"Ok","data":{"name":"Ada"}}"#;
    let request = format!(
        "POST /decode HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let (status, response_body) = send_http_request_with_retry(port, &request);
    let _ = child.wait();
    assert_eq!(status, 200);
    assert_eq!(response_body, r#""ok:Ada""#);
}

#[test]
fn milestone0_invalid_db_pool_size_should_error() {
    let program = r#"
fn main():
  db.query("select 1")

app "demo":
  main()
"#;
    let path = write_temp_file("fuse_m0_pool_size", "fuse", program);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let output = Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg("ast")
        .arg(&path)
        .env("FUSE_DB_URL", temp_db_url())
        .env("FUSE_DB_POOL_SIZE", "0")
        .output()
        .expect("failed to run fusec");
    assert!(
        !output.status.success(),
        "expected failure for invalid pool size"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("FUSE_DB_POOL_SIZE"),
        "stderr should mention invalid pool size, got: {stderr}"
    );
}
