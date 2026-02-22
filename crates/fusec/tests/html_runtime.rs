use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
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

fn write_temp_dir(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    path.push(format!("{name}_{stamp}"));
    fs::create_dir_all(&path).expect("failed to create temp dir");
    path
}

fn run_program(backend: &str, source: &str) -> std::process::Output {
    run_program_with_env(backend, source, &[])
}

fn run_program_with_env(
    backend: &str,
    source: &str,
    extra_env: &[(String, String)],
) -> std::process::Output {
    let program_path = write_temp_file("fuse_html_runtime", "fuse", source);
    let exe = env!("CARGO_BIN_EXE_fusec");
    let mut cmd = Command::new(exe);
    cmd.arg("--run")
        .arg("--backend")
        .arg(backend)
        .arg(&program_path);
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("failed to run fusec")
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test port");
    listener.local_addr().expect("local addr").port()
}

fn http_runtime_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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

fn spawn_one_shot_http_server(
    expected_request_line: &'static str,
    response: &'static str,
) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream server");
    let port = listener.local_addr().expect("upstream addr").port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept upstream request");
        let mut buffer = Vec::new();
        let mut temp = [0u8; 1024];
        loop {
            let read = stream.read(&mut temp).expect("read upstream request");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&buffer);
        let first_line = request.lines().next().unwrap_or("");
        assert_eq!(first_line, expected_request_line, "upstream request line");
        stream
            .write_all(response.as_bytes())
            .expect("write upstream response");
    });
    (port, handle)
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
    let _lock = http_runtime_test_lock()
        .lock()
        .expect("http runtime test lock");
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
app "html":
  let view = div():
    h1():
      html.text("Hello")
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
fn html_tag_sugar_renders_across_backends() {
    let program = r#"
app "html":
  let view = div(class="card"):
    "Hello"
  print(html.render(view))
"#;

    let expected = r#"<div class="card">Hello</div>"#;
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
fn html_tag_attr_underscore_maps_to_hyphen() {
    let program = r#"
app "html":
  let view = button(
    type="button"
    aria_label="Close navigation"
    data_view="openapi"
  ):
    "Open"
  print(html.render(view))
"#;

    let expected =
        r#"<button aria-label="Close navigation" data-view="openapi" type="button">Open</button>"#;
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
fn asset_builtin_resolves_hashed_paths_across_backends() {
    let program = r#"
app "asset":
  print(asset("css/app.css"))
  print(asset("/css/app.css?v=1"))
"#;

    let env = vec![(
        "FUSE_ASSET_MAP".to_string(),
        r#"{"css/app.css":"/css/app.3f92ac1f7d.css"}"#.to_string(),
    )];
    for backend in ["ast", "vm", "native"] {
        let output = run_program_with_env(backend, program, &env);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert!(lines.len() >= 2, "{backend} stdout: {stdout}");
        assert_eq!(lines[0], "/css/app.3f92ac1f7d.css", "{backend} line1");
        assert_eq!(lines[1], "/css/app.3f92ac1f7d.css?v=1", "{backend} line2");
    }
}

#[test]
fn vite_proxy_fallback_routes_unknown_paths_across_backends() {
    let program = r#"
config App:
  port: Int = 3000

service Api at "/api":
  get "/ping" -> String:
    return "pong"

app "api":
  serve(App.port)
"#;

    for backend in ["ast", "vm", "native"] {
        let (upstream_port, upstream_thread) = spawn_one_shot_http_server(
            "GET /vite/app.js?x=1 HTTP/1.1",
            "HTTP/1.1 200 OK\r\nContent-Type: application/javascript; charset=utf-8\r\nContent-Length: 19\r\n\r\nconsole.log('vite')\n",
        );
        let port = find_free_port();
        let env = vec![(
            "FUSE_VITE_PROXY_URL".to_string(),
            format!("http://127.0.0.1:{upstream_port}/vite"),
        )];
        let requests = vec![
            "GET /api/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            "GET /app.js?x=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        ];
        let responses = run_http_program_with_env_requests(backend, program, port, &env, &requests);
        assert_eq!(responses[0].status, 200, "{backend} route status");
        assert_eq!(responses[0].body.trim(), "\"pong\"", "{backend} route body");
        assert_eq!(responses[1].status, 200, "{backend} proxy status");
        assert!(
            responses[1].body.contains("console.log('vite')"),
            "{backend} proxy body: {}",
            responses[1].body
        );
        upstream_thread.join().expect("join upstream server");
    }
}

#[test]
fn svg_inline_builtin_loads_raw_svg_across_backends() {
    let svg_root = write_temp_dir("fuse_svg_runtime");
    let icons_dir = svg_root.join("icons");
    fs::create_dir_all(&icons_dir).expect("create icons dir");
    let svg_path = icons_dir.join("check.svg");
    let svg_body = "<svg viewBox=\"0 0 1 1\"><path d=\"M0 0L1 1\"/></svg>";
    fs::write(&svg_path, svg_body).expect("write svg file");

    let program = r#"
app "svg":
  let icon = svg.inline("icons/check")
  print(html.render(icon))
"#;
    let env = vec![(
        "FUSE_SVG_DIR".to_string(),
        svg_root.to_string_lossy().to_string(),
    )];

    for backend in ["ast", "vm", "native"] {
        let output = run_program_with_env(backend, program, &env);
        assert!(
            output.status.success(),
            "{backend} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), svg_body, "{backend} svg output");
    }

    let _ = fs::remove_dir_all(&svg_root);
}

#[test]
fn svg_inline_rejects_path_traversal() {
    let program = r#"
app "svg":
  print(html.render(svg.inline("../secret")))
"#;
    for backend in ["ast", "vm", "native"] {
        let output = run_program(backend, program);
        assert!(
            !output.status.success(),
            "{backend} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("path traversal is not allowed"),
            "{backend} stderr: {stderr}"
        );
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
        assert_eq!(
            responses[1].body.trim(),
            openapi_json,
            "{backend} json body"
        );
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
        let responses =
            run_http_program_with_env_requests(backend, program, port, &[], &[&request]);
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
