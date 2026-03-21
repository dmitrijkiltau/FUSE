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

fn semantic_rows(result: &JsonValue) -> Vec<(usize, usize, usize, usize)> {
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

fn token_type_at(rows: &[(usize, usize, usize, usize)], line: usize, col: usize) -> Option<usize> {
    rows.iter()
        .find(|(row_line, row_col, row_len, _)| {
            *row_line == line && col >= *row_col && col < row_col.saturating_add(*row_len)
        })
        .map(|(_, _, _, token_type)| *token_type)
}

fn line_col_of(text: &str, needle: &str) -> (usize, usize) {
    let idx = text.find(needle).expect("needle");
    let line = text[..idx].bytes().filter(|b| *b == b'\n').count();
    let line_start = text[..idx].rfind('\n').map_or(0, |pos| pos + 1);
    let col = text[line_start..idx].chars().count();
    (line, col)
}

fn wait_diagnostics(stdout: &mut ChildStdout, uri: &str) -> Vec<JsonValue> {
    loop {
        let Some(msg) = read_lsp_message(stdout) else {
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

#[test]
fn lsp_hover_semantic_tokens_and_inlay_hints() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    )
    .expect("write fuse.toml");

    let util_src = r#"type Person:
  name: String

## Says hello repeatedly.
fn greet(user: Person, times: Int) -> String:
  return "${user.name} x ${times}"
"#;
    let main_src = r#"requires db

import { Person, greet } from "./util"

fn main():
  let user: Person = Person(name="Ada")
  let out = greet(user, 2)
  let rows = db
    .from("notes")
    .select(["id"])
    .all()
  let _typed: List<Map<String, String>> = rows
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
    util_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
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
    main_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    main_doc.insert("version".to_string(), JsonValue::Number(1.0));
    main_doc.insert("text".to_string(), JsonValue::String(main_src.to_string()));
    let mut main_open_params = BTreeMap::new();
    main_open_params.insert("textDocument".to_string(), JsonValue::Object(main_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(main_open_params),
    );

    let util_diags = wait_diagnostics(&mut stdout, &util_uri);
    assert!(
        util_diags.is_empty(),
        "expected no util diagnostics, got {}",
        json::encode(&JsonValue::Array(util_diags))
    );
    let main_diags = wait_diagnostics(&mut stdout, &main_uri);
    assert!(
        main_diags.is_empty(),
        "expected no main diagnostics, got {}",
        json::encode(&JsonValue::Array(main_diags))
    );

    let mut hover_doc = BTreeMap::new();
    hover_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let (call_line, call_greet_col) = line_col_of(main_src, "greet(user, 2)");
    let mut hover_pos = BTreeMap::new();
    hover_pos.insert("line".to_string(), JsonValue::Number(call_line as f64));
    hover_pos.insert(
        "character".to_string(),
        JsonValue::Number((call_greet_col + 1) as f64),
    );
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
    assert!(
        hover_text.contains("\"range\""),
        "hover missing range: {hover_text}"
    );

    let mut sig_doc = BTreeMap::new();
    sig_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut sig_pos = BTreeMap::new();
    sig_pos.insert("line".to_string(), JsonValue::Number(call_line as f64));
    sig_pos.insert(
        "character".to_string(),
        JsonValue::Number((call_greet_col + "greet(user, ".len()) as f64),
    );
    let mut sig_params = BTreeMap::new();
    sig_params.insert("textDocument".to_string(), JsonValue::Object(sig_doc));
    sig_params.insert("position".to_string(), JsonValue::Object(sig_pos));
    send_request(
        &mut stdin,
        11,
        "textDocument/signatureHelp",
        JsonValue::Object(sig_params),
    );
    let sig_help = wait_response(&mut stdout, 11);
    let sig_help_text = json::encode(&sig_help);
    assert!(
        sig_help_text.contains("fn greet(user: Person, times: Int) -> String"),
        "raw signatureHelp should include imported function signature: {sig_help_text}"
    );
    assert!(
        sig_help_text.contains("\"activeParameter\":1"),
        "raw signatureHelp should report the active parameter for imported calls: {sig_help_text}"
    );

    let mut completion_doc = BTreeMap::new();
    completion_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut completion_pos = BTreeMap::new();
    completion_pos.insert("line".to_string(), JsonValue::Number(call_line as f64));
    completion_pos.insert(
        "character".to_string(),
        JsonValue::Number((call_greet_col + 2) as f64),
    );
    let mut completion_params = BTreeMap::new();
    completion_params.insert(
        "textDocument".to_string(),
        JsonValue::Object(completion_doc),
    );
    completion_params.insert("position".to_string(), JsonValue::Object(completion_pos));
    send_request(
        &mut stdin,
        5,
        "textDocument/completion",
        JsonValue::Object(completion_params),
    );
    let completion = wait_response(&mut stdout, 5);
    let completion_text = json::encode(&completion);
    assert!(
        completion_text.contains("\"label\":\"greet\""),
        "completion missing greet symbol: {completion_text}"
    );

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
        inlay_text.contains("user: ") && inlay_text.contains("times: "),
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
    let rows = semantic_rows(&sem);
    let (import_person_line, import_person_col) = line_col_of(main_src, "Person, greet");
    let (annotate_person_line, annotate_person_col) = line_col_of(main_src, "user: Person");
    let (import_greet_line, import_greet_col) = line_col_of(main_src, "greet } from");
    let (call_greet_line, call_greet_col) = line_col_of(main_src, "greet(user, 2)");
    let (from_line, from_col) = line_col_of(main_src, ".from(\"notes\")");
    let (select_line, select_col) = line_col_of(main_src, ".select([\"id\"])");
    let (typed_line, typed_col) = line_col_of(main_src, "List<Map<String, String>>");
    let list_col = typed_col;
    let map_col = typed_col + "List<".len();
    let string_col = typed_col + "List<Map<".len();
    let import_person_ty = token_type_at(&rows, import_person_line, import_person_col)
        .expect("token for import Person");
    let annotate_person_ty = token_type_at(&rows, annotate_person_line, annotate_person_col + 6)
        .expect("token for annotation Person");
    let import_greet_ty =
        token_type_at(&rows, import_greet_line, import_greet_col).expect("token for import greet");
    let call_greet_ty =
        token_type_at(&rows, call_greet_line, call_greet_col).expect("token for call greet");
    let from_ty = token_type_at(&rows, from_line, from_col + 1).expect("token for from");
    let select_ty = token_type_at(&rows, select_line, select_col + 1).expect("token for select");
    let list_ty = token_type_at(&rows, typed_line, list_col).expect("token for List");
    let map_ty = token_type_at(&rows, typed_line, map_col).expect("token for Map");
    let string_ty = token_type_at(&rows, typed_line, string_col).expect("token for String");
    assert_eq!(
        import_person_ty, annotate_person_ty,
        "imported type token mismatch"
    );
    assert_eq!(
        import_greet_ty, call_greet_ty,
        "imported function token mismatch"
    );
    assert_eq!(from_ty, select_ty, "db method token mismatch");
    assert_eq!(list_ty, import_person_ty, "builtin List should be typed");
    assert_eq!(map_ty, import_person_ty, "builtin Map should be typed");
    assert_eq!(
        string_ty, import_person_ty,
        "builtin String should be typed"
    );

    let (main_decl_line, _) = line_col_of(main_src, "fn main():");
    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(main_decl_line as f64));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert(
        "line".to_string(),
        JsonValue::Number((main_decl_line + 2) as f64),
    );
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

    let util_fn_line = util_src.lines().nth(4).expect("util fn line");
    let util_greet_col = util_fn_line.find("greet").expect("util greet");
    let mut rename_doc = BTreeMap::new();
    rename_doc.insert("uri".to_string(), JsonValue::String(util_uri.clone()));
    let mut rename_pos = BTreeMap::new();
    rename_pos.insert("line".to_string(), JsonValue::Number(4.0));
    rename_pos.insert(
        "character".to_string(),
        JsonValue::Number((util_greet_col + 1) as f64),
    );
    let mut rename_params = BTreeMap::new();
    rename_params.insert("textDocument".to_string(), JsonValue::Object(rename_doc));
    rename_params.insert("position".to_string(), JsonValue::Object(rename_pos));
    rename_params.insert(
        "newName".to_string(),
        JsonValue::String("greetAgain".to_string()),
    );
    send_request(
        &mut stdin,
        9,
        "textDocument/rename",
        JsonValue::Object(rename_params),
    );
    let rename = wait_response(&mut stdout, 9);
    let rename_text = json::encode(&rename);
    assert!(
        rename_text.contains("greetAgain"),
        "rename did not include requested name: {rename_text}"
    );
    assert!(
        rename_text.contains(&util_uri),
        "rename missing util edits: {rename_text}"
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
    cancel_hover_pos.insert("line".to_string(), JsonValue::Number(call_line as f64));
    cancel_hover_pos.insert(
        "character".to_string(),
        JsonValue::Number((call_greet_col + 1) as f64),
    );
    let mut cancel_hover_params = BTreeMap::new();
    cancel_hover_params.insert(
        "textDocument".to_string(),
        JsonValue::Object(cancel_hover_doc),
    );
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

#[test]
fn lsp_semantic_tokens_distinguish_signature_types_params_and_locals() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    )
    .expect("write fuse.toml");

    let main_src = r#"type PrivateNoteRow:
  title: String

fn render_private_note_card(note: PrivateNoteRow) -> Html:
  let title = note.title
  return div():
    html.text(title)

fn main():
  let note = PrivateNoteRow(title="hello")
  render_private_note_card(note)
"#;
    let main_path = dir.join("main.fuse");
    fs::write(&main_path, main_src).expect("write main.fuse");

    let root_uri = path_to_uri(&dir);
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

    let mut main_doc = BTreeMap::new();
    main_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    main_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    main_doc.insert("version".to_string(), JsonValue::Number(1.0));
    main_doc.insert("text".to_string(), JsonValue::String(main_src.to_string()));
    let mut main_open_params = BTreeMap::new();
    main_open_params.insert("textDocument".to_string(), JsonValue::Object(main_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(main_open_params),
    );

    let main_diags = wait_diagnostics(&mut stdout, &main_uri);
    assert!(
        main_diags.is_empty(),
        "expected no main diagnostics, got {}",
        json::encode(&JsonValue::Array(main_diags))
    );

    let mut sem_doc = BTreeMap::new();
    sem_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut sem_params = BTreeMap::new();
    sem_params.insert("textDocument".to_string(), JsonValue::Object(sem_doc));
    send_request(
        &mut stdin,
        2,
        "textDocument/semanticTokens/full",
        JsonValue::Object(sem_params),
    );
    let sem = wait_response(&mut stdout, 2);
    let sem_text = json::encode(&sem);
    assert!(
        sem_text.contains("\"data\"") && !sem_text.contains("\"data\":[]"),
        "semantic tokens unexpectedly empty: {sem_text}"
    );

    let rows = semantic_rows(&sem);
    let (type_decl_line, type_decl_col) = line_col_of(main_src, "PrivateNoteRow:");
    let (type_ref_line, type_ref_col) = line_col_of(main_src, "note: PrivateNoteRow");
    let type_ref_col = type_ref_col + "note: ".len();
    let (fn_decl_line, fn_decl_col) = line_col_of(main_src, "render_private_note_card(note");
    let (fn_call_line, fn_call_col) = line_col_of(main_src, "render_private_note_card(note)");
    let (param_line, param_col) = line_col_of(main_src, "note: PrivateNoteRow");
    let (local_line, local_col) = line_col_of(main_src, "title = note.title");

    let type_decl_ty =
        token_type_at(&rows, type_decl_line, type_decl_col).expect("token for type decl");
    let type_ref_ty =
        token_type_at(&rows, type_ref_line, type_ref_col).expect("token for type ref");
    let fn_decl_ty =
        token_type_at(&rows, fn_decl_line, fn_decl_col).expect("token for function decl");
    let fn_call_ty =
        token_type_at(&rows, fn_call_line, fn_call_col).expect("token for function call");
    let param_ty = token_type_at(&rows, param_line, param_col).expect("token for parameter");
    let local_ty = token_type_at(&rows, local_line, local_col).expect("token for local");

    assert_eq!(type_decl_ty, type_ref_ty, "custom type ref token mismatch");
    assert_eq!(fn_decl_ty, fn_call_ty, "function token mismatch");
    assert_ne!(param_ty, local_ty, "parameter and local should differ");
    assert_ne!(param_ty, type_ref_ty, "parameter should not look like a type");
    assert_ne!(fn_decl_ty, param_ty, "function should not look like a parameter");

    send_request(
        &mut stdin,
        3,
        "shutdown",
        JsonValue::Object(BTreeMap::new()),
    );
    let _ = wait_response(&mut stdout, 3);
    send_notification(&mut stdin, "exit", JsonValue::Object(BTreeMap::new()));
    let status = child.wait().expect("wait lsp");
    assert!(status.success(), "fuse-lsp exited with {status}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lsp_semantic_tokens_highlight_string_interpolations() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        "[package]\nentry = \"main.fuse\"\napp = \"Demo\"\n",
    )
    .expect("write fuse.toml");

    let main_src = r#"config App:
  port: Int = 3000

fn greeting(name: String) -> String:
  return "Hello, ${name}! http://localhost:${App.port}"

app "Demo":
  print(greeting("Fuse"))
"#;
    let main_path = dir.join("main.fuse");
    fs::write(&main_path, main_src).expect("write main.fuse");

    let root_uri = path_to_uri(&dir);
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

    let mut main_doc = BTreeMap::new();
    main_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    main_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    main_doc.insert("version".to_string(), JsonValue::Number(1.0));
    main_doc.insert("text".to_string(), JsonValue::String(main_src.to_string()));
    let mut main_open_params = BTreeMap::new();
    main_open_params.insert("textDocument".to_string(), JsonValue::Object(main_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(main_open_params),
    );

    let main_diags = wait_diagnostics(&mut stdout, &main_uri);
    assert!(
        main_diags.is_empty(),
        "expected no main diagnostics, got {}",
        json::encode(&JsonValue::Array(main_diags))
    );

    let mut sem_doc = BTreeMap::new();
    sem_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut sem_params = BTreeMap::new();
    sem_params.insert("textDocument".to_string(), JsonValue::Object(sem_doc));
    send_request(
        &mut stdin,
        2,
        "textDocument/semanticTokens/full",
        JsonValue::Object(sem_params),
    );
    let sem = wait_response(&mut stdout, 2);
    let sem_text = json::encode(&sem);
    assert!(
        sem_text.contains("\"data\"") && !sem_text.contains("\"data\":[]"),
        "semantic tokens unexpectedly empty: {sem_text}"
    );

    let rows = semantic_rows(&sem);
    let (config_decl_line, config_decl_col) = line_col_of(main_src, "config App:");
    let config_decl_col = config_decl_col + "config ".len();
    let (field_decl_line, field_decl_col) = line_col_of(main_src, "port: Int");
    let (param_decl_line, param_decl_col) = line_col_of(main_src, "name: String");
    let (interp_name_line, interp_name_col) = line_col_of(main_src, "${name}");
    let interp_name_col = interp_name_col + 2;
    let (interp_app_line, interp_app_col) = line_col_of(main_src, "App.port}");
    let interp_port_col = interp_app_col + "App.".len();
    let (string_text_line, string_text_col) = line_col_of(main_src, "Hello, ");

    let config_decl_ty =
        token_type_at(&rows, config_decl_line, config_decl_col).expect("token for config App");
    let field_decl_ty =
        token_type_at(&rows, field_decl_line, field_decl_col).expect("token for field port");
    let param_decl_ty =
        token_type_at(&rows, param_decl_line, param_decl_col).expect("token for param name");
    let interp_name_ty =
        token_type_at(&rows, interp_name_line, interp_name_col).expect("token for ${name}");
    let interp_app_ty =
        token_type_at(&rows, interp_app_line, interp_app_col).expect("token for App in string");
    let interp_port_ty = token_type_at(&rows, interp_app_line, interp_port_col)
        .expect("token for port in string");
    let string_text_ty = token_type_at(&rows, string_text_line, string_text_col)
        .expect("token for raw string text");

    assert_eq!(param_decl_ty, interp_name_ty, "interpolated name token mismatch");
    assert_eq!(config_decl_ty, interp_app_ty, "interpolated App token mismatch");
    assert_eq!(field_decl_ty, interp_port_ty, "interpolated port token mismatch");
    assert_ne!(string_text_ty, interp_name_ty, "string text should differ from name");
    assert_ne!(interp_app_ty, interp_port_ty, "App and port should differ");

    send_request(
        &mut stdin,
        3,
        "shutdown",
        JsonValue::Object(BTreeMap::new()),
    );
    let _ = wait_response(&mut stdout, 3);
    send_notification(&mut stdin, "exit", JsonValue::Object(BTreeMap::new()));
    let status = child.wait().expect("wait lsp");
    assert!(status.success(), "fuse-lsp exited with {status}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lsp_multi_package_definition_and_references_smoke() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        "[package]\nentry = \"src/main.fuse\"\napp = \"Demo\"\n\n[dependencies]\nAuth = { path = \"./deps/auth\" }\n",
    )
    .expect("write fuse.toml");

        let main_src = r#"import Core from "root:lib/core"
import Auth from "dep:Auth/lib"

type CoreThing:
    value: Int

fn main():
    let a = Core.plus_one(1)
    let b = Auth.plus_one(a)
    print(b)
"#;
    let core_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;
    let dep_src = r#"fn plus_one(value: Int) -> Int:
  return value + 1
"#;

    let main_path = dir.join("src").join("main.fuse");
    let core_path = dir.join("lib").join("core.fuse");
    let dep_path = dir.join("deps").join("auth").join("lib.fuse");
    fs::create_dir_all(main_path.parent().expect("main parent")).expect("create src dir");
    fs::create_dir_all(core_path.parent().expect("core parent")).expect("create lib dir");
    fs::create_dir_all(dep_path.parent().expect("dep parent")).expect("create dep dir");
    fs::write(&main_path, main_src).expect("write main.fuse");
    fs::write(&core_path, core_src).expect("write core.fuse");
    fs::write(&dep_path, dep_src).expect("write dep lib.fuse");

    let root_uri = path_to_uri(&dir);
    let main_uri = path_to_uri(&main_path);
    let core_uri = path_to_uri(&core_path);
    let dep_uri = path_to_uri(&dep_path);

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

    let mut core_doc = BTreeMap::new();
    core_doc.insert("uri".to_string(), JsonValue::String(core_uri.clone()));
    core_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    core_doc.insert("version".to_string(), JsonValue::Number(1.0));
    core_doc.insert("text".to_string(), JsonValue::String(core_src.to_string()));
    let mut core_open_params = BTreeMap::new();
    core_open_params.insert("textDocument".to_string(), JsonValue::Object(core_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(core_open_params),
    );

    let mut dep_doc = BTreeMap::new();
    dep_doc.insert("uri".to_string(), JsonValue::String(dep_uri.clone()));
    dep_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    dep_doc.insert("version".to_string(), JsonValue::Number(1.0));
    dep_doc.insert("text".to_string(), JsonValue::String(dep_src.to_string()));
    let mut dep_open_params = BTreeMap::new();
    dep_open_params.insert("textDocument".to_string(), JsonValue::Object(dep_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(dep_open_params),
    );

    let mut main_doc = BTreeMap::new();
    main_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    main_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    main_doc.insert("version".to_string(), JsonValue::Number(1.0));
    main_doc.insert("text".to_string(), JsonValue::String(main_src.to_string()));
    let mut main_open_params = BTreeMap::new();
    main_open_params.insert("textDocument".to_string(), JsonValue::Object(main_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(main_open_params),
    );

    let core_diags = wait_diagnostics(&mut stdout, &core_uri);
    assert!(
        core_diags.is_empty(),
        "expected no core diagnostics, got {}",
        json::encode(&JsonValue::Array(core_diags))
    );
    let dep_diags = wait_diagnostics(&mut stdout, &dep_uri);
    assert!(
        dep_diags.is_empty(),
        "expected no dep diagnostics, got {}",
        json::encode(&JsonValue::Array(dep_diags))
    );
    let main_diags = wait_diagnostics(&mut stdout, &main_uri);
    assert!(
        main_diags.is_empty(),
        "expected no main diagnostics, got {}",
        json::encode(&JsonValue::Array(main_diags))
    );

    let (core_call_line, core_call_col) = line_col_of(main_src, "Core.plus_one(1)");
    let mut alias_def_doc = BTreeMap::new();
    alias_def_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut alias_def_pos = BTreeMap::new();
    alias_def_pos.insert("line".to_string(), JsonValue::Number(core_call_line as f64));
    alias_def_pos.insert(
        "character".to_string(),
        JsonValue::Number((core_call_col + 1) as f64),
    );
    let mut alias_def_params = BTreeMap::new();
    alias_def_params.insert("textDocument".to_string(), JsonValue::Object(alias_def_doc));
    alias_def_params.insert("position".to_string(), JsonValue::Object(alias_def_pos));
    send_request(
        &mut stdin,
        2,
        "textDocument/definition",
        JsonValue::Object(alias_def_params),
    );
    let alias_definition = wait_response(&mut stdout, 2);
    let alias_definition_text = json::encode(&alias_definition);
    assert!(
        alias_definition_text.contains(&core_uri),
        "definition on root module alias receiver should resolve to root module file: {alias_definition_text}"
    );

    let mut hover_doc = BTreeMap::new();
    hover_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut hover_pos = BTreeMap::new();
    hover_pos.insert("line".to_string(), JsonValue::Number(core_call_line as f64));
    hover_pos.insert(
        "character".to_string(),
        JsonValue::Number((core_call_col + 1) as f64),
    );
    let mut hover_params = BTreeMap::new();
    hover_params.insert("textDocument".to_string(), JsonValue::Object(hover_doc));
    hover_params.insert("position".to_string(), JsonValue::Object(hover_pos));
    send_request(
        &mut stdin,
        3,
        "textDocument/hover",
        JsonValue::Object(hover_params),
    );
    let alias_hover = wait_response(&mut stdout, 3);
    let alias_hover_text = json::encode(&alias_hover);
    assert!(
        alias_hover_text.contains("Module") && alias_hover_text.contains("Core"),
        "hover on root module alias receiver should describe the module binding: {alias_hover_text}"
    );
    assert!(
        alias_hover_text.contains(&core_uri) && alias_hover_text.contains("plus_one"),
        "hover on root module alias receiver should include the target module and exports: {alias_hover_text}"
    );

    let mut alias_sig_doc = BTreeMap::new();
    alias_sig_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut alias_sig_pos = BTreeMap::new();
    alias_sig_pos.insert("line".to_string(), JsonValue::Number(core_call_line as f64));
    alias_sig_pos.insert(
        "character".to_string(),
        JsonValue::Number((core_call_col + "Core.plus_one(".len()) as f64),
    );
    let mut alias_sig_params = BTreeMap::new();
    alias_sig_params.insert("textDocument".to_string(), JsonValue::Object(alias_sig_doc));
    alias_sig_params.insert("position".to_string(), JsonValue::Object(alias_sig_pos));
    send_request(
        &mut stdin,
        13,
        "textDocument/signatureHelp",
        JsonValue::Object(alias_sig_params),
    );
    let alias_sig_help = wait_response(&mut stdout, 13);
    let alias_sig_help_text = json::encode(&alias_sig_help);
    assert!(
        alias_sig_help_text.contains("fn plus_one(value: Int) -> Int"),
        "raw signatureHelp should include module-alias member signatures: {alias_sig_help_text}"
    );
    assert!(
        alias_sig_help_text.contains("\"activeParameter\":0"),
        "raw signatureHelp should report the active parameter for module-alias member calls: {alias_sig_help_text}"
    );

    let mut completion_doc = BTreeMap::new();
    completion_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut completion_pos = BTreeMap::new();
    completion_pos.insert("line".to_string(), JsonValue::Number(core_call_line as f64));
    completion_pos.insert(
        "character".to_string(),
        JsonValue::Number((core_call_col + "Core.pl".len()) as f64),
    );
    let mut completion_params = BTreeMap::new();
    completion_params.insert(
        "textDocument".to_string(),
        JsonValue::Object(completion_doc),
    );
    completion_params.insert("position".to_string(), JsonValue::Object(completion_pos));
    send_request(
        &mut stdin,
        4,
        "textDocument/completion",
        JsonValue::Object(completion_params),
    );
    let alias_completion = wait_response(&mut stdout, 4);
    let alias_completion_text = json::encode(&alias_completion);
    assert!(
        alias_completion_text.contains("\"label\":\"plus_one\""),
        "completion on root module alias receiver should include exported members: {alias_completion_text}"
    );

    let mut symbol_params = BTreeMap::new();
    symbol_params.insert("query".to_string(), JsonValue::String("Core".to_string()));
    send_request(
        &mut stdin,
        5,
        "workspace/symbol",
        JsonValue::Object(symbol_params),
    );
    let symbols = wait_response(&mut stdout, 5);
    let symbols_text = json::encode(&symbols);
    assert!(
        symbols_text.contains("\"name\":\"Core\"") && symbols_text.contains(&main_uri),
        "workspace symbols should include imported module aliases: {symbols_text}"
    );
    let core_symbol_pos = symbols_text.find("\"name\":\"Core\"").expect("Core symbol");
    let core_thing_symbol_pos = symbols_text
        .find("\"name\":\"CoreThing\"")
        .expect("CoreThing symbol");
    assert!(
        core_symbol_pos < core_thing_symbol_pos,
        "workspace symbol exact alias match should rank before substring matches: {symbols_text}"
    );

    let mut prepare_doc = BTreeMap::new();
    prepare_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut prepare_pos = BTreeMap::new();
    prepare_pos.insert("line".to_string(), JsonValue::Number(core_call_line as f64));
    prepare_pos.insert(
        "character".to_string(),
        JsonValue::Number((core_call_col + 1) as f64),
    );
    let mut prepare_params = BTreeMap::new();
    prepare_params.insert("textDocument".to_string(), JsonValue::Object(prepare_doc));
    prepare_params.insert("position".to_string(), JsonValue::Object(prepare_pos));
    send_request(
        &mut stdin,
        6,
        "textDocument/prepareRename",
        JsonValue::Object(prepare_params),
    );
    let prepare = wait_response(&mut stdout, 6);
    let prepare_text = json::encode(&prepare);
    assert!(
        prepare_text.contains("\"placeholder\":\"Core\"")
            && prepare_text.contains("\"range\""),
        "prepareRename on root module alias receiver should expose the alias range: {prepare_text}"
    );

    let mut def_doc = BTreeMap::new();
    def_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut def_pos = BTreeMap::new();
    def_pos.insert("line".to_string(), JsonValue::Number(core_call_line as f64));
    def_pos.insert(
        "character".to_string(),
        JsonValue::Number((core_call_col + "Core.".len() + 1) as f64),
    );
    let mut def_params = BTreeMap::new();
    def_params.insert("textDocument".to_string(), JsonValue::Object(def_doc));
    def_params.insert("position".to_string(), JsonValue::Object(def_pos));
    send_request(
        &mut stdin,
        7,
        "textDocument/definition",
        JsonValue::Object(def_params),
    );
    let definition = wait_response(&mut stdout, 7);
    let definition_text = json::encode(&definition);
    assert!(
        definition_text.contains(&core_uri),
        "definition should resolve to root module target: {definition_text}"
    );

    let (dep_call_line, dep_call_col) = line_col_of(main_src, "Auth.plus_one(a)");
    let mut alias_refs_doc = BTreeMap::new();
    alias_refs_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut alias_refs_pos = BTreeMap::new();
    alias_refs_pos.insert("line".to_string(), JsonValue::Number(dep_call_line as f64));
    alias_refs_pos.insert(
        "character".to_string(),
        JsonValue::Number((dep_call_col + 1) as f64),
    );
    let mut alias_refs_ctx = BTreeMap::new();
    alias_refs_ctx.insert("includeDeclaration".to_string(), JsonValue::Bool(true));
    let mut alias_refs_params = BTreeMap::new();
    alias_refs_params.insert("textDocument".to_string(), JsonValue::Object(alias_refs_doc));
    alias_refs_params.insert("position".to_string(), JsonValue::Object(alias_refs_pos));
    alias_refs_params.insert("context".to_string(), JsonValue::Object(alias_refs_ctx));
    send_request(
        &mut stdin,
        8,
        "textDocument/references",
        JsonValue::Object(alias_refs_params),
    );
    let alias_refs = wait_response(&mut stdout, 8);
    let alias_refs_text = json::encode(&alias_refs);
    assert!(
        alias_refs_text.contains(&main_uri),
        "references on dependency module alias receiver should include the local binding and use site: {alias_refs_text}"
    );

    let mut refs_doc = BTreeMap::new();
    refs_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut refs_pos = BTreeMap::new();
    refs_pos.insert("line".to_string(), JsonValue::Number(dep_call_line as f64));
    refs_pos.insert(
        "character".to_string(),
        JsonValue::Number((dep_call_col + "Auth.".len() + 1) as f64),
    );
    let mut refs_ctx = BTreeMap::new();
    refs_ctx.insert("includeDeclaration".to_string(), JsonValue::Bool(true));
    let mut refs_params = BTreeMap::new();
    refs_params.insert("textDocument".to_string(), JsonValue::Object(refs_doc));
    refs_params.insert("position".to_string(), JsonValue::Object(refs_pos));
    refs_params.insert("context".to_string(), JsonValue::Object(refs_ctx));
    send_request(
        &mut stdin,
        9,
        "textDocument/references",
        JsonValue::Object(refs_params),
    );
    let refs = wait_response(&mut stdout, 9);
    let refs_text = json::encode(&refs);
    assert!(
        refs_text.contains(&main_uri) && refs_text.contains(&dep_uri),
        "references should include caller and dependency declaration: {refs_text}"
    );

    let mut dep_prepare_doc = BTreeMap::new();
    dep_prepare_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut dep_prepare_pos = BTreeMap::new();
    dep_prepare_pos.insert("line".to_string(), JsonValue::Number(dep_call_line as f64));
    dep_prepare_pos.insert(
        "character".to_string(),
        JsonValue::Number((dep_call_col + 1) as f64),
    );
    let mut dep_prepare_params = BTreeMap::new();
    dep_prepare_params.insert("textDocument".to_string(), JsonValue::Object(dep_prepare_doc));
    dep_prepare_params.insert("position".to_string(), JsonValue::Object(dep_prepare_pos));
    send_request(
        &mut stdin,
        10,
        "textDocument/prepareRename",
        JsonValue::Object(dep_prepare_params),
    );
    let dep_prepare = wait_response(&mut stdout, 10);
    let dep_prepare_text = json::encode(&dep_prepare);
    assert!(
        dep_prepare_text.contains("\"placeholder\":\"Auth\"")
            && dep_prepare_text.contains("\"range\""),
        "prepareRename on dependency module alias receiver should expose the alias range: {dep_prepare_text}"
    );

    let mut rename_doc = BTreeMap::new();
    rename_doc.insert("uri".to_string(), JsonValue::String(main_uri.clone()));
    let mut rename_pos = BTreeMap::new();
    rename_pos.insert("line".to_string(), JsonValue::Number(dep_call_line as f64));
    rename_pos.insert(
        "character".to_string(),
        JsonValue::Number((dep_call_col + 1) as f64),
    );
    let mut rename_params = BTreeMap::new();
    rename_params.insert("textDocument".to_string(), JsonValue::Object(rename_doc));
    rename_params.insert("position".to_string(), JsonValue::Object(rename_pos));
    rename_params.insert(
        "newName".to_string(),
        JsonValue::String("Accounts".to_string()),
    );
    send_request(
        &mut stdin,
        11,
        "textDocument/rename",
        JsonValue::Object(rename_params),
    );
    let rename = wait_response(&mut stdout, 11);
    let rename_text = json::encode(&rename);
    assert!(
        rename_text.contains("Accounts"),
        "rename on dependency module alias receiver should include requested name: {rename_text}"
    );
    assert!(
        rename_text.contains(&main_uri) && !rename_text.contains(&dep_uri),
        "rename on dependency module alias receiver should stay local to the importing module: {rename_text}"
    );

    send_request(
        &mut stdin,
        12,
        "shutdown",
        JsonValue::Object(BTreeMap::new()),
    );
    let _ = wait_response(&mut stdout, 12);
    send_notification(&mut stdin, "exit", JsonValue::Object(BTreeMap::new()));
    let status = child.wait().expect("wait lsp");
    assert!(status.success(), "fuse-lsp exited with {status}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lsp_code_actions_and_formatting_smoke_over_jsonrpc() {
    let dir = temp_project_dir();
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("fuse.toml"),
        "[package]\nentry = \"main_missing.fuse\"\napp = \"Demo\"\n",
    )
    .expect("write fuse.toml");

    let util_src = r#"fn greet(name: String, times: Int) -> String:
  return "${name} x ${times}"

fn helper(input: String) -> String:
  return input
"#;
    let main_missing_src = r#"import { helper } from "./util"

fn main():
  let err = NotFound()
  print(helper("Ada"))
  print(err)
"#;
    let main_unsorted_src = r#"import { helper, greet } from "./util"

fn main():
  print(greet(helper("Ada"), 1))
"#;
    let main_unformatted_src = r#"fn main():
    let x=1
    print(x)
"#;

    let util_path = dir.join("util.fuse");
    let missing_path = dir.join("main_missing.fuse");
    let unsorted_path = dir.join("main_unsorted.fuse");
    let format_path = dir.join("main_unformatted.fuse");
    fs::write(&util_path, util_src).expect("write util.fuse");
    fs::write(&missing_path, main_missing_src).expect("write main_missing.fuse");
    fs::write(&unsorted_path, main_unsorted_src).expect("write main_unsorted.fuse");
    fs::write(&format_path, main_unformatted_src).expect("write main_unformatted.fuse");

    let root_uri = path_to_uri(&dir);
    let util_uri = path_to_uri(&util_path);
    let missing_uri = path_to_uri(&missing_path);
    let unsorted_uri = path_to_uri(&unsorted_path);
    let format_uri = path_to_uri(&format_path);

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
    util_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    util_doc.insert("version".to_string(), JsonValue::Number(1.0));
    util_doc.insert("text".to_string(), JsonValue::String(util_src.to_string()));
    let mut util_open_params = BTreeMap::new();
    util_open_params.insert("textDocument".to_string(), JsonValue::Object(util_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(util_open_params),
    );

    let mut missing_doc = BTreeMap::new();
    missing_doc.insert("uri".to_string(), JsonValue::String(missing_uri.clone()));
    missing_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    missing_doc.insert("version".to_string(), JsonValue::Number(1.0));
    missing_doc.insert(
        "text".to_string(),
        JsonValue::String(main_missing_src.to_string()),
    );
    let mut missing_open_params = BTreeMap::new();
    missing_open_params.insert("textDocument".to_string(), JsonValue::Object(missing_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(missing_open_params),
    );

    let util_diags = wait_diagnostics(&mut stdout, &util_uri);
    assert!(
        util_diags.is_empty(),
        "expected no util diagnostics, got {}",
        json::encode(&JsonValue::Array(util_diags))
    );
    let missing_diags = wait_diagnostics(&mut stdout, &missing_uri);
    assert!(
        !missing_diags.is_empty(),
        "expected missing-import diagnostics"
    );

    let mut action_doc = BTreeMap::new();
    action_doc.insert("uri".to_string(), JsonValue::String(missing_uri.clone()));
    let mut range_start = BTreeMap::new();
    range_start.insert("line".to_string(), JsonValue::Number(0.0));
    range_start.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range_end = BTreeMap::new();
    range_end.insert("line".to_string(), JsonValue::Number(20.0));
    range_end.insert("character".to_string(), JsonValue::Number(0.0));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(range_start));
    range.insert("end".to_string(), JsonValue::Object(range_end));
    let mut context = BTreeMap::new();
    context.insert(
        "diagnostics".to_string(),
        JsonValue::Array(missing_diags.clone()),
    );
    let mut action_params = BTreeMap::new();
    action_params.insert("textDocument".to_string(), JsonValue::Object(action_doc));
    action_params.insert("range".to_string(), JsonValue::Object(range.clone()));
    action_params.insert("context".to_string(), JsonValue::Object(context));
    send_request(
        &mut stdin,
        2,
        "textDocument/codeAction",
        JsonValue::Object(action_params),
    );
    let missing_actions = wait_response(&mut stdout, 2);
    let missing_actions_text = json::encode(&missing_actions);
    assert!(
        missing_actions_text.contains("Import NotFound from std.Error"),
        "raw codeAction should include missing-import quickfix: {missing_actions_text}"
    );

    let mut unsorted_doc = BTreeMap::new();
    unsorted_doc.insert("uri".to_string(), JsonValue::String(unsorted_uri.clone()));
    unsorted_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    unsorted_doc.insert("version".to_string(), JsonValue::Number(1.0));
    unsorted_doc.insert(
        "text".to_string(),
        JsonValue::String(main_unsorted_src.to_string()),
    );
    let mut unsorted_open_params = BTreeMap::new();
    unsorted_open_params.insert("textDocument".to_string(), JsonValue::Object(unsorted_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(unsorted_open_params),
    );
    let unsorted_diags = wait_diagnostics(&mut stdout, &unsorted_uri);
    assert!(
        unsorted_diags.is_empty(),
        "unexpected diagnostics in organize-imports scenario: {}",
        json::encode(&JsonValue::Array(unsorted_diags))
    );

    let mut organize_doc = BTreeMap::new();
    organize_doc.insert("uri".to_string(), JsonValue::String(unsorted_uri.clone()));
    let mut organize_context = BTreeMap::new();
    organize_context.insert("diagnostics".to_string(), JsonValue::Array(Vec::new()));
    let mut organize_params = BTreeMap::new();
    organize_params.insert("textDocument".to_string(), JsonValue::Object(organize_doc));
    organize_params.insert("range".to_string(), JsonValue::Object(range.clone()));
    organize_params.insert("context".to_string(), JsonValue::Object(organize_context));
    send_request(
        &mut stdin,
        3,
        "textDocument/codeAction",
        JsonValue::Object(organize_params),
    );
    let organize_actions = wait_response(&mut stdout, 3);
    let organize_actions_text = json::encode(&organize_actions);
    assert!(
        organize_actions_text.contains("Organize imports")
            && organize_actions_text.contains("greet, helper"),
        "raw codeAction should include organize-imports edit: {organize_actions_text}"
    );

    let mut format_doc = BTreeMap::new();
    format_doc.insert("uri".to_string(), JsonValue::String(format_uri.clone()));
    format_doc.insert(
        "languageId".to_string(),
        JsonValue::String("fuse".to_string()),
    );
    format_doc.insert("version".to_string(), JsonValue::Number(1.0));
    format_doc.insert(
        "text".to_string(),
        JsonValue::String(main_unformatted_src.to_string()),
    );
    let mut format_open_params = BTreeMap::new();
    format_open_params.insert("textDocument".to_string(), JsonValue::Object(format_doc));
    send_notification(
        &mut stdin,
        "textDocument/didOpen",
        JsonValue::Object(format_open_params),
    );
    let format_diags = wait_diagnostics(&mut stdout, &format_uri);
    assert!(
        format_diags.is_empty(),
        "formatter scenario should parse without diagnostics: {}",
        json::encode(&JsonValue::Array(format_diags))
    );

    let mut format_req_doc = BTreeMap::new();
    format_req_doc.insert("uri".to_string(), JsonValue::String(format_uri.clone()));
    let mut format_params = BTreeMap::new();
    format_params.insert("textDocument".to_string(), JsonValue::Object(format_req_doc));
    send_request(
        &mut stdin,
        4,
        "textDocument/formatting",
        JsonValue::Object(format_params),
    );
    let formatting = wait_response(&mut stdout, 4);
    let formatting_text = json::encode(&formatting);
    assert!(
        formatting_text.starts_with('['),
        "raw formatting response should be an edit array: {formatting_text}"
    );

    send_request(
        &mut stdin,
        5,
        "shutdown",
        JsonValue::Object(BTreeMap::new()),
    );
    let _ = wait_response(&mut stdout, 5);
    send_notification(&mut stdin, "exit", JsonValue::Object(BTreeMap::new()));
    let status = child.wait().expect("wait lsp");
    assert!(status.success(), "fuse-lsp exited with {status}");

    let _ = fs::remove_dir_all(&dir);
}
