use std::collections::BTreeMap;
use std::io::{self, Read, Write};

use fusec::diag::{Diag, Level};
use fusec::parse_source;
use fusec::sema;
use fuse_rt::json::{self, JsonValue};

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut docs: BTreeMap<String, String> = BTreeMap::new();
    let mut shutdown = false;

    loop {
        let message = match read_message(&mut stdin)? {
            Some(value) => value,
            None => break,
        };
        let value = match json::decode(&message) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let JsonValue::Object(obj) = value else { continue };
        let method = get_string(&obj, "method");
        let id = obj.get("id").cloned();

        match method.as_deref() {
            Some("initialize") => {
                let result = capabilities_result();
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("initialized") => {}
            Some("shutdown") => {
                shutdown = true;
                let response = json_response(id, JsonValue::Null);
                write_message(&mut stdout, &response)?;
            }
            Some("exit") => {
                if shutdown {
                    break;
                } else {
                    std::process::exit(1);
                }
            }
            Some("textDocument/didOpen") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_text_doc_text(&obj) {
                        docs.insert(uri.clone(), text.clone());
                        publish_diagnostics(&mut stdout, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didChange") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_change_text(&obj) {
                        docs.insert(uri.clone(), text.clone());
                        publish_diagnostics(&mut stdout, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    docs.remove(&uri);
                    publish_empty_diagnostics(&mut stdout, &uri)?;
                }
            }
            Some("textDocument/formatting") => {
                let mut edits = Vec::new();
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = docs.get(&uri) {
                        let formatted = fusec::format::format_source(text);
                        if formatted != *text {
                            edits.push(full_document_edit(text, &formatted));
                            docs.insert(uri, formatted.clone());
                        }
                    }
                }
                let response = json_response(id, JsonValue::Array(edits));
                write_message(&mut stdout, &response)?;
            }
            _ => {
                if id.is_some() {
                    let response = json_response(id, JsonValue::Null);
                    write_message(&mut stdout, &response)?;
                }
            }
        }
    }
    Ok(())
}

fn capabilities_result() -> JsonValue {
    let mut caps = BTreeMap::new();
    caps.insert("textDocumentSync".to_string(), JsonValue::Number(1.0));
    let mut root = BTreeMap::new();
    root.insert("capabilities".to_string(), JsonValue::Object(caps));
    JsonValue::Object(root)
}

fn publish_diagnostics(out: &mut impl Write, uri: &str, text: &str) -> io::Result<()> {
    let mut diags = Vec::new();
    let (program, parse_diags) = parse_source(text);
    diags.extend(parse_diags);
    if !diags.iter().any(|d| matches!(d.level, Level::Error)) {
        let (_analysis, sema_diags) = sema::analyze_program(&program);
        diags.extend(sema_diags);
    }
    let diagnostics = to_lsp_diags(text, &diags);
    let params = diagnostics_params(uri, diagnostics);
    let notification = json_notification("textDocument/publishDiagnostics", params);
    write_message(out, &notification)
}

fn publish_empty_diagnostics(out: &mut impl Write, uri: &str) -> io::Result<()> {
    let params = diagnostics_params(uri, Vec::new());
    let notification = json_notification("textDocument/publishDiagnostics", params);
    write_message(out, &notification)
}

fn diagnostics_params(uri: &str, diagnostics: Vec<JsonValue>) -> JsonValue {
    let mut params = BTreeMap::new();
    params.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    params.insert("diagnostics".to_string(), JsonValue::Array(diagnostics));
    JsonValue::Object(params)
}

fn to_lsp_diags(text: &str, diags: &[Diag]) -> Vec<JsonValue> {
    let line_offsets = line_offsets(text);
    diags
        .iter()
        .map(|diag| {
            let (start_line, start_col) = offset_to_line_col(&line_offsets, diag.span.start);
            let (end_line, end_col) = offset_to_line_col(&line_offsets, diag.span.end);
            let range = range_json(start_line, start_col, end_line, end_col);
            let severity = match diag.level {
                Level::Error => 1.0,
                Level::Warning => 2.0,
            };
            let mut out = BTreeMap::new();
            out.insert("range".to_string(), range);
            out.insert("severity".to_string(), JsonValue::Number(severity));
            out.insert("message".to_string(), JsonValue::String(diag.message.clone()));
            out.insert("source".to_string(), JsonValue::String("fusec".to_string()));
            JsonValue::Object(out)
        })
        .collect()
}

fn full_document_edit(original: &str, new_text: &str) -> JsonValue {
    let offsets = line_offsets(original);
    let end_offset = original.len();
    let (end_line, end_col) = offset_to_line_col(&offsets, end_offset);
    let range = range_json(0, 0, end_line, end_col);
    let mut edit = BTreeMap::new();
    edit.insert("range".to_string(), range);
    edit.insert("newText".to_string(), JsonValue::String(new_text.to_string()));
    JsonValue::Object(edit)
}

fn range_json(start_line: usize, start_col: usize, end_line: usize, end_col: usize) -> JsonValue {
    let mut start = BTreeMap::new();
    start.insert("line".to_string(), JsonValue::Number(start_line as f64));
    start.insert("character".to_string(), JsonValue::Number(start_col as f64));
    let mut end = BTreeMap::new();
    end.insert("line".to_string(), JsonValue::Number(end_line as f64));
    end.insert("character".to_string(), JsonValue::Number(end_col as f64));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(start));
    range.insert("end".to_string(), JsonValue::Object(end));
    JsonValue::Object(range)
}

fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            offsets.push(idx + 1);
        }
    }
    offsets
}

fn offset_to_line_col(offsets: &[usize], offset: usize) -> (usize, usize) {
    let mut lo = 0usize;
    let mut hi = offsets.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if offsets[mid] <= offset {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let line = lo;
    let col = offset.saturating_sub(offsets[lo]);
    (line, col)
}

fn json_response(id: Option<JsonValue>, result: JsonValue) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    if let Some(id) = id {
        root.insert("id".to_string(), id);
    } else {
        root.insert("id".to_string(), JsonValue::Null);
    }
    root.insert("result".to_string(), result);
    JsonValue::Object(root)
}

fn json_notification(method: &str, params: JsonValue) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    root.insert("method".to_string(), JsonValue::String(method.to_string()));
    root.insert("params".to_string(), params);
    JsonValue::Object(root)
}

fn get_string(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(JsonValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn extract_text_doc_uri(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else { return None };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else { return None };
    match text_doc.get("uri") {
        Some(JsonValue::String(uri)) => Some(uri.clone()),
        _ => None,
    }
}

fn extract_text_doc_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else { return None };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else { return None };
    match text_doc.get("text") {
        Some(JsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn extract_change_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else { return None };
    let changes = params.get("contentChanges")?;
    let JsonValue::Array(changes) = changes else { return None };
    let first = changes.get(0)?;
    let JsonValue::Object(first) = first else { return None };
    match first.get("text") {
        Some(JsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn read_message(reader: &mut impl Read) -> io::Result<Option<String>> {
    let mut header = Vec::new();
    let mut buf = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            if header.is_empty() {
                return Ok(None);
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
    let Some(len) = content_length else { return Ok(None) };
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(String::from_utf8_lossy(&body).to_string()))
}

fn write_message(out: &mut impl Write, value: &JsonValue) -> io::Result<()> {
    let body = json::encode(value);
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}
