use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn write_temp_file(name: &str, ext: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    path.push(format!("{name}_{stamp}.{ext}"));
    fs::write(&path, contents).expect("failed to write temp file");
    path
}

fn run_program(backend: &str, source: &str) -> std::process::Output {
    let program_path = write_temp_file("fuse_html_runtime", "fuse", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    Command::new(exe)
        .arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path)
        .output()
        .expect("failed to run fusec")
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test port");
    listener.local_addr().expect("local addr").port()
}

struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

fn send_http_request_with_retry(port: u16, request: &str) -> HttpResponse {
    let start = Instant::now();
    loop {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                stream
                    .write_all(request.as_bytes())
                    .expect("failed to write request");
                let _ = stream.shutdown(std::net::Shutdown::Write);

                let mut buffer = String::new();
                stream
                    .read_to_string(&mut buffer)
                    .expect("failed to read response");

                let mut parts = buffer.splitn(2, "\r\n\r\n");
                let head = parts.next().unwrap_or("");
                let body = parts.next().unwrap_or("").to_string();
                let mut lines = head.split("\r\n");
                let status_line = lines.next().unwrap_or("");
                let status = status_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("500")
                    .parse::<u16>()
                    .unwrap_or(500);
                let mut headers = HashMap::new();
                for line in lines {
                    if let Some((key, value)) = line.split_once(':') {
                        headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                return HttpResponse {
                    status,
                    headers,
                    body,
                };
            }
            Err(_) => {
                if start.elapsed() > Duration::from_secs(3) {
                    panic!("server did not start on 127.0.0.1:{port}");
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn run_http_program(backend: &str, source: &str, port: u16) -> HttpResponse {
    let program_path = write_temp_file("fuse_html_http", "fuse", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let child = Command::new(exe)
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

    let request = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    let response = send_http_request_with_retry(port, &request);
    let output = child.wait_with_output().expect("failed to wait for server");
    assert!(
        output.status.success(),
        "{backend} stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    response
}

#[test]
fn html_builtins_render_across_backends() {
    let program = r#"
app "html":
  let view = html.node("div", {"id": "x", "class": "card"}, [
    html.node("h1", {}, [html.text("Hello")]),
    html.raw("<p>raw</p>")
  ])
  print(html.render(view))
"#;

    let expected = r#"<div class="card" id="x"><h1>Hello</h1><p>raw</p></div>"#;
    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), expected, "{backend} stdout");
    }
}

#[test]
fn html_http_response_sets_text_html_content_type() {
    let program = r#"
config App:
  port: Int = 3000

service Docs at "/":
  get "/" -> Html:
    return html.node("h1", {}, [html.text("Hi")])

app "docs":
  serve(App.port)
"#;

    for backend in ["ast", "vm", "native"] {
        let port = find_free_port();
        let response = run_http_program(backend, program, port);
        assert_eq!(
            response.status, 200,
            "{backend} status, body: {}",
            response.body
        );
        let content_type = response
            .headers
            .get("content-type")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            content_type, "text/html; charset=utf-8",
            "{backend} content-type"
        );
        assert_eq!(response.body.trim(), "<h1>Hi</h1>", "{backend} body");
    }
}
