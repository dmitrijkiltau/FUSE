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
    run_http_program_with_env(backend, source, port, &[])
}

fn run_http_program_with_env(
    backend: &str,
    source: &str,
    port: u16,
    extra_env: &[(String, String)],
) -> HttpResponse {
    let mut responses = run_http_program_with_env_requests(
        backend,
        source,
        port,
        extra_env,
        &["GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"],
    );
    responses.remove(0)
}

fn run_http_program_with_env_requests(
    backend: &str,
    source: &str,
    port: u16,
    extra_env: &[(String, String)],
    requests: &[&str],
) -> Vec<HttpResponse> {
    let program_path = write_temp_file("fuse_html_http", "fuse", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path)
        .env("APP_PORT", port.to_string())
        .env("FUSE_MAX_REQUESTS", requests.len().to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let child = cmd.spawn().expect("failed to start server");
    let mut responses = Vec::new();
    for request in requests {
        let request = if request.contains("Host:") {
            request.replace("localhost", &format!("127.0.0.1:{port}"))
        } else {
            request.to_string()
        };
        responses.push(send_http_request_with_retry(port, &request));
    }
    let output = child.wait_with_output().expect("failed to wait for server");
    assert!(
        output.status.success(),
        "{backend} stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    responses
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
fn html_block_dsl_renders_across_backends() {
    let program = r#"
fn div(attrs: Map<String, String>, children: List<Html>) -> Html:
  return html.node("div", attrs, children)

fn h1(attrs: Map<String, String>, children: List<Html>) -> Html:
  return html.node("h1", attrs, children)

fn text(value: String) -> Html:
  return html.text(value)

app "html":
  let view = div():
    h1():
      text("Hello")
  print(html.render(view))
"#;

    let expected = r#"<div><h1>Hello</h1></div>"#;
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

#[test]
fn html_http_injects_live_reload_script_when_enabled() {
    let program = r#"
config App:
  port: Int = 3000

service Docs at "/":
  get "/" -> Html:
    return html.node("h1", {}, [html.text("Hi")])

app "docs":
  serve(App.port)
"#;

    let ws_url = "ws://127.0.0.1:35555/__reload".to_string();
    let extra_env = vec![("FUSE_DEV_RELOAD_WS_URL".to_string(), ws_url.clone())];
    for backend in ["ast", "vm", "native"] {
        let port = find_free_port();
        let response = run_http_program_with_env(backend, program, port, &extra_env);
        assert!(
            response.body.contains("data-fuse-live-reload"),
            "{backend} body: {}",
            response.body
        );
        assert!(
            response.body.contains(&ws_url),
            "{backend} body: {}",
            response.body
        );
    }
}

#[test]
fn openapi_ui_routes_are_served_when_enabled() {
    let program = r#"
config App:
  port: Int = 3000

service Docs at "/api":
  get "/ping" -> String:
    return "ok"

app "docs":
  serve(App.port)
"#;
    let mut openapi_path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    openapi_path.push(format!("fuse_openapi_ui_{stamp}.json"));
    let openapi_json = r#"{"openapi":"3.0.0","info":{"title":"Doc","version":"1"},"paths":{"/api/ping":{"get":{"summary":"ping"}}}}"#;
    fs::write(&openapi_path, openapi_json).expect("write openapi json");

    let env = vec![
        (
            "FUSE_OPENAPI_JSON_PATH".to_string(),
            openapi_path.to_string_lossy().to_string(),
        ),
        ("FUSE_OPENAPI_UI_PATH".to_string(), "/docs".to_string()),
    ];
    let requests = vec![
        "GET /docs HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        "GET /docs/openapi.json HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    ];

    for backend in ["ast", "vm", "native"] {
        let port = find_free_port();
        let responses = run_http_program_with_env_requests(backend, program, port, &env, &requests);
        assert_eq!(responses[0].status, 200, "{backend} docs status");
        assert!(
            responses[0].body.contains("FUSE OpenAPI"),
            "{backend} docs body: {}",
            responses[0].body
        );
        assert_eq!(responses[1].status, 200, "{backend} json status");
        assert_eq!(responses[1].body.trim(), openapi_json, "{backend} json body");
    }

    let _ = fs::remove_file(openapi_path);
}

#[test]
fn html_fragment_post_route_supports_server_driven_swaps() {
    let program = r#"
config App:
  port: Int = 3000

type NoteInput:
  title: String(1..80)

service Notes at "/api":
  post "/notes" body NoteInput -> Html:
    return html.node("li", {"class": "note-row"}, [html.text(body.title)])

app "notes":
  serve(App.port)
"#;
    let payload = r#"{"title":"Ship it"}"#;
    let request = format!(
        "POST /api/notes HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );

    for backend in ["ast", "vm", "native"] {
        let port = find_free_port();
        let responses = run_http_program_with_env_requests(backend, program, port, &[], &[&request]);
        let response = &responses[0];
        assert_eq!(response.status, 200, "{backend} status");
        let content_type = response
            .headers
            .get("content-type")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            content_type, "text/html; charset=utf-8",
            "{backend} content-type"
        );
        assert_eq!(
            response.body.trim(),
            r#"<li class="note-row">Ship it</li>"#,
            "{backend} body"
        );
    }
}
