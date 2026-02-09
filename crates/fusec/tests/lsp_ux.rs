use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use fuse_rt::json::{self, JsonValue};

fn temp_project_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("fuse_lsp_ux_test_{nanos}"));
    dir
}

fn path_to_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

fn write_lsp_message(stdin: &mut ChildStdin, body: &JsonValue) {
    let payload = json::encode(body);
    write!(stdin, "Content-Length: {}\r\n\r\n{payload}", payload.len()).expect("write lsp");
    stdin.flush().expect("flush lsp");
}

fn read_lsp_message(stdout: &mut ChildStdout) -> Option<JsonValue> {
    let mut header = Vec::new();
    let mut buf = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        let n = stdout.read(&mut buf).ok()?;
        if n == 0 {
            if header.is_empty() {
                return None;
            }
            break;
        }
        header.extend_from_slice(&buf[..n]);
    }
    let header_text = String::from_utf8_lossy(&header);
    let mut content_length = None;
    for line in header_text.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let len = content_length?;
    let mut body = vec![0u8; len];
    stdout.read_exact(&mut body).ok()?;
    json::decode(&String::from_utf8_lossy(&body)).ok()
}

fn send_request(stdin: &mut ChildStdin, id: u64, method: &str, params: JsonValue) {
    let mut msg = BTreeMap::new();
    msg.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    msg.insert("id".to_string(), JsonValue::Number(id as f64));
    msg.insert("method".to_string(), JsonValue::String(method.to_string()));
    msg.insert("params".to_string(), params);
    write_lsp_message(stdin, &JsonValue::Object(msg));
}

fn send_notification(stdin: &mut ChildStdin, method: &str, params: JsonValue) {
    let mut msg = BTreeMap::new();
    msg.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    msg.insert("method".to_string(), JsonValue::String(method.to_string()));
    msg.insert("params".to_string(), params);
    write_lsp_message(stdin, &JsonValue::Object(msg));
}

fn wait_response(stdout: &mut ChildStdout, id: u64) -> JsonValue {
    loop {
        let Some(msg) = read_lsp_message(stdout) else {
            panic!("missing response for id {id}");
        };
        let JsonValue::Object(obj) = msg else {
            continue;
        };
        let Some(JsonValue::Number(got)) = obj.get("id") else {
            continue;
        };
        if *got as u64 != id {
            continue;
        }
        return obj.get("result").cloned().unwrap_or(JsonValue::Null);
    }
}

fn wait_error(stdout: &mut ChildStdout, id: u64) -> JsonValue {
    loop {
        let Some(msg) = read_lsp_message(stdout) else {
            panic!("missing error response for id {id}");
        };
        let JsonValue::Object(obj) = msg else {
            continue;
        };
        let Some(JsonValue::Number(got)) = obj.get("id") else {
            continue;
        };
        if *got as u64 != id {
            continue;
        }
        return obj.get("error").cloned().unwrap_or(JsonValue::Null);
    }
}

fn spawn_lsp() -> (Child, ChildStdin, ChildStdout) {
    let exe = find_fuse_lsp_bin().expect("could not locate fuse-lsp binary");
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn fuse-lsp");
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    (child, stdin, stdout)
}

fn find_fuse_lsp_bin() -> Option<PathBuf> {
    for key in ["CARGO_BIN_EXE_fuse-lsp", "CARGO_BIN_EXE_fuse_lsp"] {
        if let Ok(path) = std::env::var(key) {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }
    }

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_dir = crate_dir.parent()?.parent()?;
    let mut candidates = Vec::new();
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        candidates.push(PathBuf::from(target_dir).join("debug").join("fuse-lsp"));
    }
    candidates.push(workspace_dir.join("tmp").join("fuse-target").join("debug").join("fuse-lsp"));
    candidates.push(workspace_dir.join("target").join("debug").join("fuse-lsp"));
    candidates.into_iter().find(|path| path.exists())
}

#[test]
fn lsp_hover_semantic_tokens_and_inlay_hints() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    )
    .expect("write fuse.toml");

    let util_src = r#"## Says hello repeatedly.
fn greet(name: String, times: Int) -> String:
  return "${name} x ${times}"
"#;
    let main_src = r#"import { greet } from "./util"

fn main():
  let out = greet("Ada", 2)
  print(out)
"#;
    let util_path = dir.join("util.fuse");
    let main_path = dir.join("main.fuse");
    fs::write(&util_path, util_src).expect("write util.fuse");
    fs::write(&main_path, main_src).expect("write main.fuse");

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let main_uri = path_to_uri(&main_path);

    let (mut child, mut stdin, mut stdout) = spawn_lsp();

    let mut init_params = BTreeMap::new();
    init_params.insert("rootUri".to_string(), JsonValue::String(root_uri));
    send_request(&mut stdin, 1, "initialize", JsonValue::Object(init_params));
    let _ = wait_response(&mut stdout, 1);
    send_notification(
        &mut stdin,
        "initialized",
        JsonValue::Object(BTreeMap::new()),
    );

    let mut util_doc = BTreeMap::new();
    util_doc.insert("uri".to_string(), JsonValue::String(util_uri.clone()));
    util_doc.insert("languageId".to_string(), JsonValue::String("fuse".to_string()));
    util_doc.insert("version".to_string(), JsonValue::Number(1.0));
    util_doc.insert("text".to_string(), JsonValue::String(util_src.to_string()));
    let mut util_open_params = BTreeMap::new();
    util_open_params.insert("textDocument".to_string(), JsonValue::Object(util_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(util_open_params),
    );

    let mut main_doc = BTreeMap::new();
    main_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    main_doc.insert("languageId".to_string(), JsonValue::String("fuse".to_string()));
    main_doc.insert("version".to_string(), JsonValue::Number(1.0));
    main_doc.insert("text".to_string(), JsonValue::String(main_src.to_string()));
    let mut main_open_params = BTreeMap::new();
    main_open_params.insert("textDocument".to_string(), JsonValue::Object(main_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(main_open_params),
    );

    let mut hover_doc = BTreeMap::new();
    hover_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut hover_pos = BTreeMap::new();
    hover_pos.insert("line".to_string(), JsonValue::Number(3.0));
    hover_pos.insert("character".to_string(), JsonValue::Number(14.0));
    let mut hover_params = BTreeMap::new();
    hover_params.insert("textDocument".to_string(), JsonValue::Object(hover_doc));
    hover_params.insert("position".to_string(), JsonValue::Object(hover_pos));
    send_request(
        &mut stdin,
        2,
        "textDocument/hover",
        JsonValue::Object(hover_params),
    );
    let hover = wait_response(&mut stdout, 2);
    let hover_text = json::encode(&hover);
    assert!(
        hover_text.contains("Says hello repeatedly."),
        "hover missing docstring: {hover_text}"
    );
    assert!(hover_text.contains("\"range\""), "hover missing range: {hover_text}");

    let mut inlay_doc = BTreeMap::new();
    inlay_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(0.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(50.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));
    let mut inlay_params = BTreeMap::new();
    inlay_params.insert("textDocument".to_string(), JsonValue::Object(inlay_doc));
    inlay_params.insert("range".to_string(), JsonValue::Object(range));
    send_request(
        &mut stdin,
        3,
        "textDocument/inlayHint",
        JsonValue::Object(inlay_params),
    );
    let inlays = wait_response(&mut stdout, 3);
    let inlay_text = json::encode(&inlays);
    assert!(
        inlay_text.contains("name: ") && inlay_text.contains("times: "),
        "inlay hints missing parameter labels: {inlay_text}"
    );

    let mut sem_doc = BTreeMap::new();
    sem_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut sem_params = BTreeMap::new();
    sem_params.insert("textDocument".to_string(), JsonValue::Object(sem_doc));
    send_request(
        &mut stdin,
        4,
        "textDocument/semanticTokens/full",
        JsonValue::Object(sem_params),
    );
    let sem = wait_response(&mut stdout, 4);
    let sem_text = json::encode(&sem);
    assert!(
        sem_text.contains("\"data\"") && !sem_text.contains("\"data\":[]"),
        "semantic tokens unexpectedly empty: {sem_text}"
    );

    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(2.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(4.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));
    let mut sem_range_doc = BTreeMap::new();
    sem_range_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut sem_range_params = BTreeMap::new();
    sem_range_params.insert("textDocument".to_string(), JsonValue::Object(sem_range_doc));
    sem_range_params.insert("range".to_string(), JsonValue::Object(range));
    send_request(
        &mut stdin,
        6,
        "textDocument/semanticTokens/range",
        JsonValue::Object(sem_range_params),
    );
    let sem_range = wait_response(&mut stdout, 6);
    let sem_range_text = json::encode(&sem_range);
    assert!(
        sem_range_text.contains("\"data\"") && !sem_range_text.contains("\"data\":[]"),
        "semantic tokens range unexpectedly empty: {sem_range_text}"
    );

    let mut cancel_params = BTreeMap::new();
    cancel_params.insert("id".to_string(), JsonValue::Number(7.0));
    send_notification(
        &mut stdin,
        "$/cancelRequest",
        JsonValue::Object(cancel_params),
    );
    let mut cancel_hover_doc = BTreeMap::new();
    cancel_hover_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut cancel_hover_pos = BTreeMap::new();
    cancel_hover_pos.insert("line".to_string(), JsonValue::Number(3.0));
    cancel_hover_pos.insert("character".to_string(), JsonValue::Number(14.0));
    let mut cancel_hover_params = BTreeMap::new();
    cancel_hover_params.insert("textDocument".to_string(), JsonValue::Object(cancel_hover_doc));
    cancel_hover_params.insert("position".to_string(), JsonValue::Object(cancel_hover_pos));
    send_request(
        &mut stdin,
        7,
        "textDocument/hover",
        JsonValue::Object(cancel_hover_params),
    );
    let err = wait_error(&mut stdout, 7);
    let err_text = json::encode(&err);
    assert!(
        err_text.contains("\"code\":-32800"),
        "expected cancellation error: {err_text}"
    );

    send_request(
        &mut stdin,
        8,
        "shutdown",
        JsonValue::Object(BTreeMap::new()),
    );
    let _ = wait_response(&mut stdout, 8);
    send_notification(&mut stdin, "exit", JsonValue::Object(BTreeMap::new()));
    let status = child.wait().expect("wait lsp");
    assert!(status.success(), "fuse-lsp exited with {status}");

    let _ = fs::remove_dir_all(&dir);
}
