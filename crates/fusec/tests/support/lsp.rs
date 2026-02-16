#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use fuse_rt::json::{self, JsonValue};

pub fn temp_project_dir(prefix: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("{prefix}_{nanos}"));
    dir
}

pub fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

pub fn write_project_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, text).expect("write file");
}

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    next_id: u64,
}

impl LspClient {
    pub fn spawn_with_root(root_uri: &str) -> Self {
        let exe = find_fuse_lsp_bin().expect("could not locate fuse-lsp binary");
        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn fuse-lsp");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };

        let mut init_params = BTreeMap::new();
        init_params.insert("rootUri".to_string(), JsonValue::String(root_uri.to_string()));
        let _ = client.request("initialize", JsonValue::Object(init_params));
        client.notify("initialized", JsonValue::Object(BTreeMap::new()));
        client
    }

    pub fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn request(&mut self, method: &str, params: JsonValue) -> JsonValue {
        let id = self.next_request_id();
        self.send_request_with_id(id, method, params);
        self.wait_response(id)
    }

    pub fn send_request_with_id(&mut self, id: u64, method: &str, params: JsonValue) {
        let mut msg = BTreeMap::new();
        msg.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
        msg.insert("id".to_string(), JsonValue::Number(id as f64));
        msg.insert("method".to_string(), JsonValue::String(method.to_string()));
        msg.insert("params".to_string(), params);
        write_lsp_message(&mut self.stdin, &JsonValue::Object(msg));
    }

    pub fn notify(&mut self, method: &str, params: JsonValue) {
        let mut msg = BTreeMap::new();
        msg.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
        msg.insert("method".to_string(), JsonValue::String(method.to_string()));
        msg.insert("params".to_string(), params);
        write_lsp_message(&mut self.stdin, &JsonValue::Object(msg));
    }

    pub fn open_document(&mut self, uri: &str, text: &str, version: u64) {
        let mut text_doc = BTreeMap::new();
        text_doc.insert("uri".to_string(), JsonValue::String(uri.to_string()));
        text_doc.insert(
            "languageId".to_string(),
            JsonValue::String("fuse".to_string()),
        );
        text_doc.insert("version".to_string(), JsonValue::Number(version as f64));
        text_doc.insert("text".to_string(), JsonValue::String(text.to_string()));
        let mut params = BTreeMap::new();
        params.insert("textDocument".to_string(), JsonValue::Object(text_doc));
        self.notify("textDocument/didOpen", JsonValue::Object(params));
    }

    pub fn change_document(&mut self, uri: &str, text: &str, version: u64) {
        let mut text_doc = BTreeMap::new();
        text_doc.insert("uri".to_string(), JsonValue::String(uri.to_string()));
        text_doc.insert("version".to_string(), JsonValue::Number(version as f64));
        let mut change = BTreeMap::new();
        change.insert("text".to_string(), JsonValue::String(text.to_string()));
        let mut params = BTreeMap::new();
        params.insert("textDocument".to_string(), JsonValue::Object(text_doc));
        params.insert(
            "contentChanges".to_string(),
            JsonValue::Array(vec![JsonValue::Object(change)]),
        );
        self.notify("textDocument/didChange", JsonValue::Object(params));
    }

    pub fn close_document(&mut self, uri: &str) {
        let mut text_doc = BTreeMap::new();
        text_doc.insert("uri".to_string(), JsonValue::String(uri.to_string()));
        let mut params = BTreeMap::new();
        params.insert("textDocument".to_string(), JsonValue::Object(text_doc));
        self.notify("textDocument/didClose", JsonValue::Object(params));
    }

    pub fn wait_response(&mut self, id: u64) -> JsonValue {
        loop {
            let Some(msg) = read_lsp_message(&mut self.stdout) else {
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

    pub fn wait_diagnostics(&mut self, uri: &str) -> Vec<JsonValue> {
        loop {
            let Some(msg) = read_lsp_message(&mut self.stdout) else {
                panic!("missing diagnostics for {uri}");
            };
            let JsonValue::Object(obj) = msg else {
                continue;
            };
            let Some(JsonValue::String(method)) = obj.get("method") else {
                continue;
            };
            if method != "textDocument/publishDiagnostics" {
                continue;
            }
            let Some(JsonValue::Object(params)) = obj.get("params") else {
                continue;
            };
            let Some(JsonValue::String(got_uri)) = params.get("uri") else {
                continue;
            };
            if got_uri != uri {
                continue;
            }
            if let Some(JsonValue::Array(diags)) = params.get("diagnostics") {
                return diags.clone();
            }
            return Vec::new();
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.request("shutdown", JsonValue::Object(BTreeMap::new()));
        self.notify("exit", JsonValue::Object(BTreeMap::new()));
        let status = self.child.wait().expect("wait lsp");
        assert!(status.success(), "fuse-lsp exited with {status}");
    }
}

pub fn semantic_rows(result: &JsonValue) -> Vec<(usize, usize, usize, usize)> {
    let JsonValue::Object(obj) = result else {
        return Vec::new();
    };
    let Some(JsonValue::Array(data)) = obj.get("data") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut line = 0usize;
    let mut col = 0usize;
    let mut idx = 0usize;
    while idx + 4 < data.len() {
        let delta_line = match data[idx] {
            JsonValue::Number(v) => v as usize,
            _ => break,
        };
        let delta_col = match data[idx + 1] {
            JsonValue::Number(v) => v as usize,
            _ => break,
        };
        let len = match data[idx + 2] {
            JsonValue::Number(v) => v as usize,
            _ => break,
        };
        let token_type = match data[idx + 3] {
            JsonValue::Number(v) => v as usize,
            _ => break,
        };
        if delta_line > 0 {
            line += delta_line;
            col = delta_col;
        } else {
            col += delta_col;
        }
        out.push((line, col, len, token_type));
        idx += 5;
    }
    out
}

pub fn token_type_at(
    rows: &[(usize, usize, usize, usize)],
    line: usize,
    col: usize,
) -> Option<usize> {
    rows.iter()
        .find(|(row_line, row_col, row_len, _)| {
            *row_line == line && col >= *row_col && col < row_col.saturating_add(*row_len)
        })
        .map(|(_, _, _, token_type)| *token_type)
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
    candidates.push(
        workspace_dir
            .join("tmp")
            .join("fuse-target")
            .join("debug")
            .join("fuse-lsp"),
    );
    candidates.push(workspace_dir.join("target").join("debug").join("fuse-lsp"));
    candidates.into_iter().find(|path| path.exists())
}
