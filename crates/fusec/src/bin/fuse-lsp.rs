use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use fuse_rt::json::{self, JsonValue};
use fusec::ast::{
    BinaryOp, Block, ConfigDecl, Doc, EnumDecl, Expr, ExprKind, FnDecl, Ident, ImportDecl,
    ImportSpec, Item, Literal, Pattern, PatternKind, Program, ServiceDecl, Stmt, StmtKind,
    TypeDecl, TypeDerive, TypeRef, TypeRefKind, UnaryOp,
};
use fusec::diag::{Diag, Level};
use fusec::loader::{ModuleRegistry, load_program_with_modules_and_deps_and_overrides};
use fusec::parse_source;
use fusec::sema;
use fusec::span::Span;

const SEMANTIC_TOKEN_TYPES: [&str; 12] = [
    "namespace",
    "type",
    "enum",
    "enumMember",
    "function",
    "parameter",
    "variable",
    "property",
    "keyword",
    "string",
    "number",
    "comment",
];
const SEM_NAMESPACE: usize = 0;
const SEM_TYPE: usize = 1;
const SEM_ENUM: usize = 2;
const SEM_ENUM_MEMBER: usize = 3;
const SEM_FUNCTION: usize = 4;
const SEM_PARAMETER: usize = 5;
const SEM_VARIABLE: usize = 6;
const SEM_PROPERTY: usize = 7;
const SEM_KEYWORD: usize = 8;
const SEM_STRING: usize = 9;
const SEM_NUMBER: usize = 10;
const SEM_COMMENT: usize = 11;

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut state = LspState::default();
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
        let JsonValue::Object(obj) = value else {
            continue;
        };
        let method = get_string(&obj, "method");
        let id = obj.get("id").cloned();

        if method.as_deref() == Some("$/cancelRequest") {
            handle_cancel(&mut state, &obj);
            continue;
        }

        if let Some(err) = cancelled_error(&mut state, id.as_ref()) {
            if id.is_some() {
                let response = json_error_response(id, -32800, &err);
                write_message(&mut stdout, &response)?;
            }
            continue;
        }

        match method.as_deref() {
            Some("initialize") => {
                state.root_uri = extract_root_uri(&obj);
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
                        state.docs.insert(uri.clone(), text.clone());
                        publish_diagnostics(&mut stdout, &state, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didChange") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_change_text(&obj) {
                        state.docs.insert(uri.clone(), text.clone());
                        publish_diagnostics(&mut stdout, &state, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    state.docs.remove(&uri);
                    publish_empty_diagnostics(&mut stdout, &uri)?;
                }
            }
            Some("textDocument/formatting") => {
                let mut edits = Vec::new();
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = state.docs.get(&uri) {
                        let formatted = fusec::format::format_source(text);
                        if formatted != *text {
                            edits.push(full_document_edit(text, &formatted));
                            state.docs.insert(uri, formatted.clone());
                        }
                    }
                }
                let response = json_response(id, JsonValue::Array(edits));
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/definition") => {
                let result = handle_definition(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/hover") => {
                let result = handle_hover(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/rename") => {
                let result = handle_rename(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/references") => {
                let result = handle_references(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/prepareCallHierarchy") => {
                let result = handle_prepare_call_hierarchy(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("callHierarchy/incomingCalls") => {
                let result = handle_call_hierarchy_incoming(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("callHierarchy/outgoingCalls") => {
                let result = handle_call_hierarchy_outgoing(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("workspace/symbol") => {
                let result = handle_workspace_symbol(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/codeAction") => {
                let result = handle_code_action(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/semanticTokens/full") => {
                let result = handle_semantic_tokens(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/semanticTokens/range") => {
                let result = handle_semantic_tokens_range(&state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/inlayHint") => {
                let result = handle_inlay_hints(&state, &obj);
                let response = json_response(id, result);
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
    caps.insert("definitionProvider".to_string(), JsonValue::Bool(true));
    caps.insert("hoverProvider".to_string(), JsonValue::Bool(true));
    caps.insert("renameProvider".to_string(), JsonValue::Bool(true));
    caps.insert("referencesProvider".to_string(), JsonValue::Bool(true));
    caps.insert("callHierarchyProvider".to_string(), JsonValue::Bool(true));
    let mut code_action = BTreeMap::new();
    code_action.insert(
        "codeActionKinds".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("quickfix".to_string()),
            JsonValue::String("source.organizeImports".to_string()),
        ]),
    );
    caps.insert(
        "codeActionProvider".to_string(),
        JsonValue::Object(code_action),
    );
    caps.insert("inlayHintProvider".to_string(), JsonValue::Bool(true));
    let mut semantic = BTreeMap::new();
    let mut legend = BTreeMap::new();
    legend.insert(
        "tokenTypes".to_string(),
        JsonValue::Array(
            SEMANTIC_TOKEN_TYPES
                .iter()
                .map(|name| JsonValue::String((*name).to_string()))
                .collect(),
        ),
    );
    legend.insert("tokenModifiers".to_string(), JsonValue::Array(Vec::new()));
    semantic.insert("legend".to_string(), JsonValue::Object(legend));
    semantic.insert("full".to_string(), JsonValue::Bool(true));
    semantic.insert("range".to_string(), JsonValue::Bool(true));
    caps.insert(
        "semanticTokensProvider".to_string(),
        JsonValue::Object(semantic),
    );
    caps.insert("workspaceSymbolProvider".to_string(), JsonValue::Bool(true));
    let mut root = BTreeMap::new();
    root.insert("capabilities".to_string(), JsonValue::Object(caps));
    JsonValue::Object(root)
}

#[derive(Default)]
struct LspState {
    docs: BTreeMap<String, String>,
    root_uri: Option<String>,
    cancelled: HashSet<String>,
}

fn handle_cancel(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) {
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return;
    };
    let Some(id) = params.get("id") else {
        return;
    };
    if let Some(key) = request_id_key(id) {
        state.cancelled.insert(key);
    }
}

fn cancelled_error(state: &mut LspState, id: Option<&JsonValue>) -> Option<String> {
    let id = id?;
    let key = request_id_key(id)?;
    if state.cancelled.remove(&key) {
        Some("request cancelled".to_string())
    } else {
        None
    }
}

fn request_id_key(id: &JsonValue) -> Option<String> {
    match id {
        JsonValue::Number(num) => Some(format!("{num}")),
        JsonValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn publish_diagnostics(
    out: &mut impl Write,
    state: &LspState,
    uri: &str,
    text: &str,
) -> io::Result<()> {
    let diags = workspace_diags_for_uri(state, uri, text).unwrap_or_else(|| {
        let mut diags = Vec::new();
        let (program, parse_diags) = parse_source(text);
        diags.extend(parse_diags);
        if !diags.iter().any(|d| matches!(d.level, Level::Error)) {
            let (_analysis, sema_diags) = sema::analyze_program(&program);
            diags.extend(sema_diags);
        }
        diags
    });
    let diagnostics = to_lsp_diags(text, &diags);
    let params = diagnostics_params(uri, diagnostics);
    let notification = json_notification("textDocument/publishDiagnostics", params);
    write_message(out, &notification)
}

fn workspace_diags_for_uri(state: &LspState, uri: &str, _text: &str) -> Option<Vec<Diag>> {
    let focus_path = uri_to_path(uri)?;
    let root_path = if let Some(root_uri) = &state.root_uri {
        uri_to_path(root_uri)?
    } else {
        focus_path.clone()
    };
    let entry_path = resolve_entry_path(&root_path, Some(&focus_path))?;

    let mut overrides = HashMap::new();
    for (doc_uri, doc_text) in &state.docs {
        if let Some(path) = uri_to_path(doc_uri) {
            let key = path.canonicalize().unwrap_or(path);
            overrides.insert(key, doc_text.clone());
        }
    }

    let entry_key = entry_path
        .canonicalize()
        .unwrap_or_else(|_| entry_path.clone());
    let root_text = overrides
        .get(&entry_key)
        .cloned()
        .or_else(|| std::fs::read_to_string(&entry_path).ok())?;
    let (registry, loader_diags) = load_program_with_modules_and_deps_and_overrides(
        &entry_path,
        &root_text,
        &HashMap::new(),
        &overrides,
    );

    let focus_key = focus_path
        .canonicalize()
        .unwrap_or_else(|_| focus_path.clone());
    let module_id = registry
        .modules
        .iter()
        .find(|(_, unit)| {
            unit.path
                .canonicalize()
                .unwrap_or_else(|_| unit.path.clone())
                == focus_key
        })
        .map(|(id, _)| *id)?;

    let (_, sema_diags) = sema::analyze_module(&registry, module_id);
    let mut diags = Vec::new();
    for diag in loader_diags {
        if diag.path.is_none() {
            diags.push(diag);
            continue;
        }
        if let Some(path) = diag.path.as_ref() {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if key == focus_key {
                diags.push(diag);
            }
        }
    }
    diags.extend(sema_diags);
    Some(diags)
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
            out.insert(
                "message".to_string(),
                JsonValue::String(diag.message.clone()),
            );
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
    edit.insert(
        "newText".to_string(),
        JsonValue::String(new_text.to_string()),
    );
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

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    if !uri.starts_with("file://") {
        return None;
    }
    let mut raw = uri.trim_start_matches("file://").to_string();
    if raw.starts_with('/') && raw.len() > 2 && raw.as_bytes()[2] == b':' {
        raw.remove(0);
    }
    let decoded = decode_uri_component(&raw);
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    if raw.contains("://") {
        return raw;
    }
    format!("file://{}", raw)
}

fn decode_uri_component(value: &str) -> String {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex_val(bytes[idx + 1]), hex_val(bytes[idx + 2])) {
                out.push((a * 16 + b) as char);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx] as char);
        idx += 1;
    }
    out
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

fn line_col_to_offset(text: &str, offsets: &[usize], line: usize, col: usize) -> usize {
    if offsets.is_empty() {
        return 0;
    }
    let line = line.min(offsets.len() - 1);
    let start = offsets[line];
    let end = offsets.get(line + 1).copied().unwrap_or_else(|| text.len());
    let offset = start.saturating_add(col);
    offset.min(end)
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

fn json_error_response(id: Option<JsonValue>, code: i64, message: &str) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    if let Some(id) = id {
        root.insert("id".to_string(), id);
    } else {
        root.insert("id".to_string(), JsonValue::Null);
    }
    let mut err = BTreeMap::new();
    err.insert("code".to_string(), JsonValue::Number(code as f64));
    err.insert(
        "message".to_string(),
        JsonValue::String(message.to_string()),
    );
    root.insert("error".to_string(), JsonValue::Object(err));
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
    let JsonValue::Object(params) = params else {
        return None;
    };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else {
        return None;
    };
    match text_doc.get("uri") {
        Some(JsonValue::String(uri)) => Some(uri.clone()),
        _ => None,
    }
}

fn extract_text_doc_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else {
        return None;
    };
    match text_doc.get("text") {
        Some(JsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn extract_change_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let changes = params.get("contentChanges")?;
    let JsonValue::Array(changes) = changes else {
        return None;
    };
    let first = changes.get(0)?;
    let JsonValue::Object(first) = first else {
        return None;
    };
    match first.get("text") {
        Some(JsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn extract_position(obj: &BTreeMap<String, JsonValue>) -> Option<(String, usize, usize)> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else {
        return None;
    };
    let uri = match text_doc.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    let position = params.get("position")?;
    let JsonValue::Object(position) = position else {
        return None;
    };
    let line = match position.get("line") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let character = match position.get("character") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    Some((uri, line, character))
}

fn extract_new_name(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    match params.get("newName") {
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn extract_workspace_query(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    match params.get("query") {
        Some(JsonValue::String(query)) => Some(query.clone()),
        _ => None,
    }
}

fn extract_include_declaration(obj: &BTreeMap<String, JsonValue>) -> bool {
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return true;
    };
    match params.get("context") {
        Some(JsonValue::Object(context)) => match context.get("includeDeclaration") {
            Some(JsonValue::Bool(value)) => *value,
            _ => true,
        },
        _ => true,
    }
}

fn extract_root_uri(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    if let Some(JsonValue::String(uri)) = params.get("rootUri") {
        if !uri.is_empty() {
            return Some(uri.clone());
        }
    }
    if let Some(JsonValue::String(path)) = params.get("rootPath") {
        if !path.is_empty() {
            return Some(path_to_uri(Path::new(path)));
        }
    }
    None
}

#[derive(Clone)]
struct CodeActionDiag {
    message: String,
    span: Option<Span>,
}

fn extract_code_action_diagnostics(
    obj: &BTreeMap<String, JsonValue>,
    text: &str,
) -> Vec<CodeActionDiag> {
    let mut out = Vec::new();
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return out;
    };
    let Some(JsonValue::Object(context)) = params.get("context") else {
        return out;
    };
    let Some(JsonValue::Array(diags)) = context.get("diagnostics") else {
        return out;
    };
    let offsets = line_offsets(text);
    for diag in diags {
        let JsonValue::Object(diag_obj) = diag else {
            continue;
        };
        let message = match diag_obj.get("message") {
            Some(JsonValue::String(value)) => value.clone(),
            _ => continue,
        };
        let span = diag_obj
            .get("range")
            .and_then(|range| lsp_range_to_span(range, text, &offsets));
        out.push(CodeActionDiag { message, span });
    }
    out
}

fn lsp_range_to_span(range: &JsonValue, text: &str, offsets: &[usize]) -> Option<Span> {
    let JsonValue::Object(range_obj) = range else {
        return None;
    };
    let JsonValue::Object(start) = range_obj.get("start")? else {
        return None;
    };
    let JsonValue::Object(end) = range_obj.get("end")? else {
        return None;
    };
    let start_line = match start.get("line") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let start_col = match start.get("character") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let end_line = match end.get("line") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let end_col = match end.get("character") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let start_offset = line_col_to_offset(text, offsets, start_line, start_col);
    let end_offset = line_col_to_offset(text, offsets, end_line, end_col);
    Some(Span::new(start_offset, end_offset))
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
    let Some(len) = content_length else {
        return Ok(None);
    };
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

fn handle_definition(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    let Some(def_text) = index.file_text(&def.uri) else {
        return JsonValue::Null;
    };
    let location = location_json(&def.uri, def_text, def.def.span);
    JsonValue::Array(vec![location])
}

fn handle_hover(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    let mut value = format!(
        "**{}** `{}`\n\n```fuse\n{}\n```",
        def.def.kind.hover_label(),
        def.def.name,
        def.def.detail.trim()
    );
    if let Some(doc) = &def.def.doc {
        if !doc.trim().is_empty() {
            value.push_str("\n\n");
            value.push_str(doc.trim());
        }
    }
    let mut contents = BTreeMap::new();
    contents.insert(
        "kind".to_string(),
        JsonValue::String("markdown".to_string()),
    );
    contents.insert("value".to_string(), JsonValue::String(value));
    let mut out = BTreeMap::new();
    out.insert("contents".to_string(), JsonValue::Object(contents));
    if let Some(text) = index.file_text(&def.uri) {
        out.insert("range".to_string(), span_range_json(text, def.def.span));
    }
    JsonValue::Object(out)
}

fn handle_workspace_symbol(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let query = extract_workspace_query(obj)
        .unwrap_or_default()
        .to_lowercase();
    let mut symbols = Vec::new();
    let index = match build_workspace_index(state, "") {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    for def in &index.defs {
        if !query.is_empty() && !def.def.name.to_lowercase().contains(&query) {
            continue;
        }
        let Some(file_idx) = index.file_by_uri.get(&def.uri) else {
            continue;
        };
        let file = &index.files[*file_idx];
        let symbol = symbol_info_json(&def.uri, &file.text, &def.def);
        symbols.push(symbol);
    }
    JsonValue::Array(symbols)
}

fn handle_rename(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(new_name) = extract_new_name(obj) else {
        return JsonValue::Null;
    };
    if !is_valid_ident(&new_name) {
        return JsonValue::Null;
    }
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    let edits = index.rename_edits(def.id, &new_name);
    if edits.is_empty() {
        return JsonValue::Null;
    }
    let mut changes = BTreeMap::new();
    for (uri, edits) in edits {
        changes.insert(uri, JsonValue::Array(edits));
    }
    let mut root = BTreeMap::new();
    root.insert("changes".to_string(), JsonValue::Object(changes));
    JsonValue::Object(root)
}

fn handle_code_action(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Array(Vec::new());
    };
    let text = state
        .docs
        .get(&uri)
        .cloned()
        .or_else(|| uri_to_path(&uri).and_then(|path| std::fs::read_to_string(path).ok()));
    let Some(text) = text else {
        return JsonValue::Array(Vec::new());
    };
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let (program, _) = parse_source(&text);
    let imports = collect_imports(&program);
    let mut actions = Vec::new();
    let mut seen = HashSet::new();

    for diag in extract_code_action_diagnostics(obj, &text) {
        let Some(symbol) = parse_unknown_symbol_name(&diag.message) else {
            continue;
        };
        if !is_valid_ident(&symbol) {
            continue;
        }
        if let Some(span) = diag.span {
            for alias in index.alias_modules_for_symbol(&uri, &symbol) {
                let replacement = format!("{alias}.{symbol}");
                let edit = workspace_edit_with_single_span(&uri, &text, span, &replacement);
                let title = format!("Qualify as {alias}.{symbol}");
                let key = format!("quickfix:{title}");
                if seen.insert(key) {
                    actions.push(code_action_json(&title, "quickfix", edit));
                }
            }
        }

        for module_path in import_candidates_for_symbol(&index, &uri, &symbol)
            .into_iter()
            .take(8)
        {
            let Some(edit) =
                missing_import_workspace_edit(&uri, &text, &imports, &module_path, &symbol)
            else {
                continue;
            };
            let title = format!("Import {symbol} from {module_path}");
            let key = format!("quickfix:{title}");
            if seen.insert(key) {
                actions.push(code_action_json(&title, "quickfix", edit));
            }
        }
    }

    if let Some(edit) = organize_imports_workspace_edit(&uri, &text, &imports) {
        let title = "Organize imports";
        let key = "source:organizeImports".to_string();
        if seen.insert(key) {
            actions.push(code_action_json(title, "source.organizeImports", edit));
        }
    }

    JsonValue::Array(actions)
}

fn handle_semantic_tokens(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    semantic_tokens_for_text(state, &uri, &text, None)
}

fn handle_semantic_tokens_range(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    let range = extract_lsp_range(obj, &text);
    semantic_tokens_for_text(state, &uri, &text, range)
}

fn semantic_tokens_for_text(
    state: &LspState,
    uri: &str,
    text: &str,
    range: Option<Span>,
) -> JsonValue {
    let mut symbol_types: HashMap<(usize, usize), usize> = HashMap::new();
    if let Some(index) = build_workspace_index(state, uri) {
        for def in &index.defs {
            if def.uri != uri {
                continue;
            }
            if let Some(token_type) = semantic_type_for_symbol_kind(def.def.kind) {
                symbol_types.insert((def.def.span.start, def.def.span.end), token_type);
            }
        }
        for reference in &index.refs {
            if reference.uri != uri {
                continue;
            }
            let Some(def) = index.defs.get(reference.target) else {
                continue;
            };
            let Some(token_type) = semantic_type_for_symbol_kind(def.def.kind) else {
                continue;
            };
            symbol_types.insert((reference.span.start, reference.span.end), token_type);
        }
    } else {
        let (program, _) = parse_source(text);
        let index = build_index_with_program(text, &program);
        for def in &index.defs {
            if let Some(token_type) = semantic_type_for_symbol_kind(def.kind) {
                symbol_types.insert((def.span.start, def.span.end), token_type);
            }
        }
        for reference in &index.refs {
            let Some(def) = index.defs.get(reference.target) else {
                continue;
            };
            let Some(token_type) = semantic_type_for_symbol_kind(def.kind) else {
                continue;
            };
            symbol_types.insert((reference.span.start, reference.span.end), token_type);
        }
    }

    let mut token_diags = fusec::diag::Diagnostics::default();
    let tokens = fusec::lexer::lex(text, &mut token_diags);
    let mut rows = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        let token_type = match &token.kind {
            fusec::token::TokenKind::Keyword(fusec::token::Keyword::From) => {
                semantic_member_token_type(&tokens, idx).or(Some(SEM_KEYWORD))
            }
            fusec::token::TokenKind::Keyword(_) => Some(SEM_KEYWORD),
            fusec::token::TokenKind::String(_) | fusec::token::TokenKind::InterpString(_) => {
                Some(SEM_STRING)
            }
            fusec::token::TokenKind::Int(_) | fusec::token::TokenKind::Float(_) => Some(SEM_NUMBER),
            fusec::token::TokenKind::DocComment(_) => Some(SEM_COMMENT),
            fusec::token::TokenKind::Bool(_) | fusec::token::TokenKind::Null => Some(SEM_KEYWORD),
            fusec::token::TokenKind::Ident(name) => symbol_types
                .get(&(token.span.start, token.span.end))
                .copied()
                .or_else(|| semantic_ident_fallback(&tokens, idx, name))
                .or(Some(SEM_VARIABLE)),
            _ => None,
        };
        let Some(token_type) = token_type else {
            continue;
        };
        if let Some(range) = range {
            if token.span.end < range.start || token.span.start > range.end {
                continue;
            }
        }
        rows.push(SemanticTokenRow {
            span: token.span,
            token_type,
        });
    }
    rows.sort_by_key(|row| (row.span.start, row.span.end, row.token_type));

    let offsets = line_offsets(text);
    let mut data = Vec::new();
    let mut last_line = 0usize;
    let mut last_col = 0usize;
    let mut first = true;
    for row in rows {
        let Some(slice) = text.get(row.span.start..row.span.end) else {
            continue;
        };
        let length = slice.chars().count();
        if length == 0 {
            continue;
        }
        let (line, col) = offset_to_line_col(&offsets, row.span.start);
        let delta_line = if first {
            line
        } else {
            line.saturating_sub(last_line)
        };
        let delta_start = if first || delta_line > 0 {
            col
        } else {
            col.saturating_sub(last_col)
        };
        data.push(JsonValue::Number(delta_line as f64));
        data.push(JsonValue::Number(delta_start as f64));
        data.push(JsonValue::Number(length as f64));
        data.push(JsonValue::Number(row.token_type as f64));
        data.push(JsonValue::Number(0.0));
        first = false;
        last_line = line;
        last_col = col;
    }

    let mut out = BTreeMap::new();
    out.insert("data".to_string(), JsonValue::Array(data));
    JsonValue::Object(out)
}

fn semantic_ident_fallback(
    tokens: &[fusec::token::Token],
    idx: usize,
    name: &str,
) -> Option<usize> {
    if let Some(token_type) = semantic_std_error_token_type(tokens, idx) {
        return Some(token_type);
    }
    if let Some(token_type) = semantic_member_token_type(tokens, idx) {
        return Some(token_type);
    }
    if is_builtin_receiver(name)
        && matches!(
            next_non_layout_token(tokens, idx).map(|token| &token.kind),
            Some(fusec::token::TokenKind::Punct(fusec::token::Punct::Dot))
        )
    {
        return Some(SEM_NAMESPACE);
    }
    if is_builtin_function_name(name)
        && matches!(
            next_non_layout_token(tokens, idx).map(|token| &token.kind),
            Some(fusec::token::TokenKind::Punct(fusec::token::Punct::LParen))
        )
    {
        return Some(SEM_FUNCTION);
    }
    if is_builtin_type_name(name) && is_type_context(tokens, idx) {
        return Some(SEM_TYPE);
    }
    None
}

fn semantic_std_error_token_type(tokens: &[fusec::token::Token], idx: usize) -> Option<usize> {
    let kind = &tokens.get(idx)?.kind;
    if !matches!(kind, fusec::token::TokenKind::Ident(_)) {
        return None;
    }
    let mut start = idx;
    while start >= 2
        && matches!(
            tokens[start - 1].kind,
            fusec::token::TokenKind::Punct(fusec::token::Punct::Dot)
        )
        && matches!(tokens[start - 2].kind, fusec::token::TokenKind::Ident(_))
    {
        start -= 2;
    }
    let first = ident_token_name(&tokens[start].kind)?;
    if first != "std" {
        return None;
    }
    if start + 2 >= tokens.len()
        || !matches!(
            tokens[start + 1].kind,
            fusec::token::TokenKind::Punct(fusec::token::Punct::Dot)
        )
    {
        return None;
    }
    let second = ident_token_name(&tokens[start + 2].kind)?;
    if second != "Error" {
        return None;
    }
    if idx == start {
        Some(SEM_NAMESPACE)
    } else {
        Some(SEM_VARIABLE)
    }
}

fn ident_token_name(kind: &fusec::token::TokenKind) -> Option<&str> {
    match kind {
        fusec::token::TokenKind::Ident(name) => Some(name.as_str()),
        _ => None,
    }
}

fn semantic_member_token_type(tokens: &[fusec::token::Token], idx: usize) -> Option<usize> {
    let prev = prev_non_layout_token(tokens, idx)?;
    if !matches!(
        prev.kind,
        fusec::token::TokenKind::Punct(fusec::token::Punct::Dot)
    ) {
        return None;
    }
    if matches!(
        next_non_layout_token(tokens, idx).map(|token| &token.kind),
        Some(fusec::token::TokenKind::Punct(fusec::token::Punct::LParen))
    ) {
        Some(SEM_FUNCTION)
    } else {
        Some(SEM_PROPERTY)
    }
}

fn prev_non_layout_token(
    tokens: &[fusec::token::Token],
    idx: usize,
) -> Option<&fusec::token::Token> {
    let mut i = idx;
    while i > 0 {
        i -= 1;
        if !is_layout_token(&tokens[i].kind) {
            return Some(&tokens[i]);
        }
    }
    None
}

fn next_non_layout_token(
    tokens: &[fusec::token::Token],
    idx: usize,
) -> Option<&fusec::token::Token> {
    let mut i = idx + 1;
    while i < tokens.len() {
        if !is_layout_token(&tokens[i].kind) {
            return Some(&tokens[i]);
        }
        i += 1;
    }
    None
}

fn is_layout_token(kind: &fusec::token::TokenKind) -> bool {
    matches!(
        kind,
        fusec::token::TokenKind::Indent
            | fusec::token::TokenKind::Dedent
            | fusec::token::TokenKind::Newline
            | fusec::token::TokenKind::Eof
    )
}

fn is_type_context(tokens: &[fusec::token::Token], idx: usize) -> bool {
    matches!(
        prev_non_layout_token(tokens, idx).map(|token| &token.kind),
        Some(fusec::token::TokenKind::Punct(
            fusec::token::Punct::Colon
                | fusec::token::Punct::Lt
                | fusec::token::Punct::Comma
                | fusec::token::Punct::Arrow
                | fusec::token::Punct::Question
                | fusec::token::Punct::Bang
        ))
    )
}

fn is_builtin_receiver(name: &str) -> bool {
    matches!(name, "db" | "task" | "json" | "html" | "svg")
}

fn is_builtin_function_name(name: &str) -> bool {
    matches!(name, "print" | "env" | "serve" | "log" | "assert" | "asset")
        || fusec::html_tags::is_html_tag(name)
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "Unit"
            | "Int"
            | "Float"
            | "Bool"
            | "String"
            | "Bytes"
            | "Html"
            | "Id"
            | "Email"
            | "Error"
            | "List"
            | "Map"
            | "Task"
            | "Range"
    )
}

fn handle_inlay_hints(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Array(Vec::new());
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Array(Vec::new());
    };
    let range = extract_lsp_range(obj, &text);
    let (program, parse_diags) = parse_source(&text);
    if parse_diags
        .iter()
        .any(|diag| matches!(diag.level, Level::Error))
    {
        return JsonValue::Array(Vec::new());
    }
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let offsets = line_offsets(&text);
    let mut hints = Vec::new();
    let mut seen = HashSet::new();
    for item in &program.items {
        match item {
            Item::Fn(decl) => collect_inlay_hints_block(
                &index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_inlay_hints_block(
                        &index,
                        &uri,
                        &text,
                        &offsets,
                        &route.body,
                        range,
                        &mut hints,
                        &mut seen,
                    );
                }
            }
            Item::App(decl) => collect_inlay_hints_block(
                &index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Migration(decl) => collect_inlay_hints_block(
                &index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Test(decl) => collect_inlay_hints_block(
                &index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            _ => {}
        }
    }
    JsonValue::Array(hints)
}

fn semantic_type_for_symbol_kind(kind: SymbolKind) -> Option<usize> {
    match kind {
        SymbolKind::Module => Some(SEM_NAMESPACE),
        SymbolKind::Type | SymbolKind::Config => Some(SEM_TYPE),
        SymbolKind::Enum => Some(SEM_ENUM),
        SymbolKind::EnumVariant => Some(SEM_ENUM_MEMBER),
        SymbolKind::Function
        | SymbolKind::Service
        | SymbolKind::App
        | SymbolKind::Migration
        | SymbolKind::Test => Some(SEM_FUNCTION),
        SymbolKind::Param => Some(SEM_PARAMETER),
        SymbolKind::Variable => Some(SEM_VARIABLE),
        SymbolKind::Field => Some(SEM_PROPERTY),
    }
}

fn load_text_for_uri(state: &LspState, uri: &str) -> Option<String> {
    state
        .docs
        .get(uri)
        .cloned()
        .or_else(|| uri_to_path(uri).and_then(|path| std::fs::read_to_string(path).ok()))
}

fn extract_lsp_range(obj: &BTreeMap<String, JsonValue>, text: &str) -> Option<Span> {
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return None;
    };
    let range = params.get("range")?;
    let offsets = line_offsets(text);
    lsp_range_to_span(range, text, &offsets)
}

fn collect_inlay_hints_block(
    index: &WorkspaceIndex,
    uri: &str,
    text: &str,
    offsets: &[usize],
    block: &Block,
    range: Option<Span>,
    hints: &mut Vec<JsonValue>,
    seen: &mut HashSet<(usize, String)>,
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let { name, ty, expr } | StmtKind::Var { name, ty, expr } => {
                if ty.is_none() {
                    if let Some(ty_name) = infer_expr_type(index, uri, text, expr) {
                        let label = format!(": {ty_name}");
                        push_inlay_hint(
                            offsets,
                            name.span.end,
                            &label,
                            1,
                            false,
                            range,
                            hints,
                            seen,
                        );
                    }
                }
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            }
            StmtKind::Assign { target, expr } => {
                collect_inlay_hints_expr(index, uri, text, offsets, target, range, hints, seen);
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                collect_inlay_hints_expr(index, uri, text, offsets, cond, range, hints, seen);
                collect_inlay_hints_block(
                    index, uri, text, offsets, then_block, range, hints, seen,
                );
                for (else_cond, else_block) in else_if {
                    collect_inlay_hints_expr(
                        index, uri, text, offsets, else_cond, range, hints, seen,
                    );
                    collect_inlay_hints_block(
                        index, uri, text, offsets, else_block, range, hints, seen,
                    );
                }
                if let Some(else_block) = else_block {
                    collect_inlay_hints_block(
                        index, uri, text, offsets, else_block, range, hints, seen,
                    );
                }
            }
            StmtKind::Match { expr, cases } => {
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
                for (_, case_block) in cases {
                    collect_inlay_hints_block(
                        index, uri, text, offsets, case_block, range, hints, seen,
                    );
                }
            }
            StmtKind::For { iter, block, .. } => {
                collect_inlay_hints_expr(index, uri, text, offsets, iter, range, hints, seen);
                collect_inlay_hints_block(index, uri, text, offsets, block, range, hints, seen);
            }
            StmtKind::While { cond, block } => {
                collect_inlay_hints_expr(index, uri, text, offsets, cond, range, hints, seen);
                collect_inlay_hints_block(index, uri, text, offsets, block, range, hints, seen);
            }
            StmtKind::Expr(expr) => {
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn collect_inlay_hints_expr(
    index: &WorkspaceIndex,
    uri: &str,
    text: &str,
    offsets: &[usize],
    expr: &Expr,
    range: Option<Span>,
    hints: &mut Vec<JsonValue>,
    seen: &mut HashSet<(usize, String)>,
) {
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            if let Some(param_names) = call_param_names(index, uri, text, callee) {
                for (idx, arg) in args.iter().enumerate() {
                    if arg.name.is_none() {
                        if let Some(param_name) = param_names.get(idx) {
                            if !param_name.is_empty() {
                                let label = format!("{param_name}: ");
                                push_inlay_hint(
                                    offsets,
                                    arg.value.span.start,
                                    &label,
                                    2,
                                    true,
                                    range,
                                    hints,
                                    seen,
                                );
                            }
                        }
                    }
                    collect_inlay_hints_expr(
                        index, uri, text, offsets, &arg.value, range, hints, seen,
                    );
                }
            } else {
                for arg in args {
                    collect_inlay_hints_expr(
                        index, uri, text, offsets, &arg.value, range, hints, seen,
                    );
                }
            }
            collect_inlay_hints_expr(index, uri, text, offsets, callee, range, hints, seen);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_inlay_hints_expr(index, uri, text, offsets, left, range, hints, seen);
            collect_inlay_hints_expr(index, uri, text, offsets, right, range, hints, seen);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Await { expr } | ExprKind::Box { expr } => {
            collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            collect_inlay_hints_expr(index, uri, text, offsets, base, range, hints, seen);
        }
        ExprKind::Index {
            base,
            index: index_expr,
        }
        | ExprKind::OptionalIndex {
            base,
            index: index_expr,
        } => {
            collect_inlay_hints_expr(index, uri, text, offsets, base, range, hints, seen);
            collect_inlay_hints_expr(index, uri, text, offsets, index_expr, range, hints, seen);
        }
        ExprKind::StructLit { fields, .. } => {
            for field in fields {
                collect_inlay_hints_expr(
                    index,
                    uri,
                    text,
                    offsets,
                    &field.value,
                    range,
                    hints,
                    seen,
                );
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_inlay_hints_expr(index, uri, text, offsets, item, range, hints, seen);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_inlay_hints_expr(index, uri, text, offsets, key, range, hints, seen);
                collect_inlay_hints_expr(index, uri, text, offsets, value, range, hints, seen);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_inlay_hints_expr(index, uri, text, offsets, left, range, hints, seen);
            collect_inlay_hints_expr(index, uri, text, offsets, right, range, hints, seen);
        }
        ExprKind::BangChain { expr, error } => {
            collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            if let Some(error) = error {
                collect_inlay_hints_expr(index, uri, text, offsets, error, range, hints, seen);
            }
        }
        ExprKind::Spawn { block } => {
            collect_inlay_hints_block(index, uri, text, offsets, block, range, hints, seen);
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
}

fn push_inlay_hint(
    offsets: &[usize],
    offset: usize,
    label: &str,
    kind: u32,
    padding_right: bool,
    range: Option<Span>,
    hints: &mut Vec<JsonValue>,
    seen: &mut HashSet<(usize, String)>,
) {
    if let Some(range) = range {
        if offset < range.start || offset > range.end {
            return;
        }
    }
    let key = (offset, label.to_string());
    if !seen.insert(key) {
        return;
    }
    let (line, col) = offset_to_line_col(offsets, offset);
    let mut position = BTreeMap::new();
    position.insert("line".to_string(), JsonValue::Number(line as f64));
    position.insert("character".to_string(), JsonValue::Number(col as f64));
    let mut out = BTreeMap::new();
    out.insert("position".to_string(), JsonValue::Object(position));
    out.insert("label".to_string(), JsonValue::String(label.to_string()));
    out.insert("kind".to_string(), JsonValue::Number(kind as f64));
    if padding_right {
        out.insert("paddingRight".to_string(), JsonValue::Bool(true));
    }
    hints.push(JsonValue::Object(out));
}

fn call_param_names(
    index: &WorkspaceIndex,
    uri: &str,
    text: &str,
    callee: &Expr,
) -> Option<Vec<String>> {
    let span = match &callee.kind {
        ExprKind::Ident(ident) => ident.span,
        ExprKind::Member { name, .. } | ExprKind::OptionalMember { name, .. } => name.span,
        _ => callee.span,
    };
    let offsets = line_offsets(text);
    let (line, col) = offset_to_line_col(&offsets, span.start);
    let def = index.definition_at(uri, line, col)?;
    if def.def.kind != SymbolKind::Function {
        return None;
    }
    parse_fn_param_names(&def.def.detail)
}

fn parse_fn_param_names(detail: &str) -> Option<Vec<String>> {
    if !detail.starts_with("fn ") {
        return None;
    }
    let open = detail.find('(')?;
    let close = detail[open + 1..].find(')')? + open + 1;
    let params_text = detail[open + 1..close].trim();
    if params_text.is_empty() {
        return Some(Vec::new());
    }
    let mut names = Vec::new();
    for part in params_text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let name = part.split(':').next().unwrap_or("").trim();
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    Some(names)
}

fn parse_fn_return_type(detail: &str) -> Option<String> {
    let (_, ret) = detail.split_once("->")?;
    let ret = ret.trim();
    if ret.is_empty() {
        return None;
    }
    Some(ret.to_string())
}

fn parse_declared_type_from_detail(detail: &str) -> Option<String> {
    if !(detail.starts_with("let ")
        || detail.starts_with("var ")
        || detail.starts_with("param ")
        || detail.starts_with("field "))
    {
        return None;
    }
    let (_, ty) = detail.split_once(':')?;
    let ty = ty.trim();
    if ty.is_empty() {
        return None;
    }
    Some(ty.to_string())
}

fn infer_expr_type(index: &WorkspaceIndex, uri: &str, text: &str, expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(_)) => Some("Int".to_string()),
        ExprKind::Literal(Literal::Float(_)) => Some("Float".to_string()),
        ExprKind::Literal(Literal::Bool(_)) => Some("Bool".to_string()),
        ExprKind::Literal(Literal::String(_)) => Some("String".to_string()),
        ExprKind::Literal(Literal::Null) => Some("Null".to_string()),
        ExprKind::StructLit { name, .. } => Some(name.name.clone()),
        ExprKind::ListLit(_) => Some("List".to_string()),
        ExprKind::MapLit(_) => Some("Map".to_string()),
        ExprKind::InterpString(_) => Some("String".to_string()),
        ExprKind::Spawn { .. } => Some("Task".to_string()),
        ExprKind::Coalesce { left, .. } => infer_expr_type(index, uri, text, left),
        ExprKind::Await { expr } | ExprKind::Box { expr } | ExprKind::BangChain { expr, .. } => {
            infer_expr_type(index, uri, text, expr)
        }
        ExprKind::Ident(ident) => {
            let offsets = line_offsets(text);
            let (line, col) = offset_to_line_col(&offsets, ident.span.start);
            let def = index.definition_at(uri, line, col)?;
            parse_declared_type_from_detail(&def.def.detail)
        }
        ExprKind::Call { callee, .. } => {
            let span = match &callee.kind {
                ExprKind::Ident(ident) => ident.span,
                ExprKind::Member { name, .. } | ExprKind::OptionalMember { name, .. } => name.span,
                _ => callee.span,
            };
            let offsets = line_offsets(text);
            let (line, col) = offset_to_line_col(&offsets, span.start);
            let def = index.definition_at(uri, line, col)?;
            parse_fn_return_type(&def.def.detail)
        }
        ExprKind::Unary { op, expr } => match op {
            UnaryOp::Not => Some("Bool".to_string()),
            UnaryOp::Neg => infer_expr_type(index, uri, text, expr),
        },
        ExprKind::Binary { op, left, right } => match op {
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::And
            | BinaryOp::Or => Some("Bool".to_string()),
            BinaryOp::Range => Some("List".to_string()),
            BinaryOp::Add => {
                let left_ty = infer_expr_type(index, uri, text, left)?;
                if left_ty == "String" {
                    Some("String".to_string())
                } else {
                    Some(left_ty)
                }
            }
            _ => infer_expr_type(index, uri, text, left)
                .or_else(|| infer_expr_type(index, uri, text, right)),
        },
        ExprKind::Member { .. }
        | ExprKind::OptionalMember { .. }
        | ExprKind::Index { .. }
        | ExprKind::OptionalIndex { .. } => None,
    }
}

#[derive(Clone)]
struct SemanticTokenRow {
    span: Span,
    token_type: usize,
}

fn handle_references(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let include_declaration = extract_include_declaration(obj);
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    JsonValue::Array(index.reference_locations(def.id, include_declaration))
}

fn handle_prepare_call_hierarchy(state: &LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    if !is_callable_def_kind(def.def.kind) {
        return JsonValue::Null;
    }
    let Some(item) = call_hierarchy_item_json(&index, &def) else {
        return JsonValue::Null;
    };
    JsonValue::Array(vec![item])
}

fn handle_call_hierarchy_incoming(
    state: &LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(index) = build_workspace_index_for_call_hierarchy(state, obj) else {
        return JsonValue::Null;
    };
    let Some(def_id) = call_hierarchy_target_def_id(&index, obj) else {
        return JsonValue::Null;
    };
    let mut result = Vec::new();
    for (from_id, sites) in index.incoming_calls(def_id) {
        let Some(from_def) = index.def_for_target(from_id) else {
            continue;
        };
        let Some(from_item) = call_hierarchy_item_json(&index, &from_def) else {
            continue;
        };
        let mut ranges = Vec::new();
        let mut seen = HashSet::new();
        for site in sites {
            if !seen.insert((site.span.start, site.span.end)) {
                continue;
            }
            if let Some(range) = index.span_range_json(&site.uri, site.span) {
                ranges.push(range);
            }
        }
        let mut item = BTreeMap::new();
        item.insert("from".to_string(), from_item);
        item.insert("fromRanges".to_string(), JsonValue::Array(ranges));
        result.push(JsonValue::Object(item));
    }
    JsonValue::Array(result)
}

fn collect_imports(program: &Program) -> Vec<ImportDecl> {
    let mut imports = Vec::new();
    for item in &program.items {
        if let Item::Import(decl) = item {
            imports.push(decl.clone());
        }
    }
    imports.sort_by_key(|decl| decl.span.start);
    imports
}

fn parse_unknown_symbol_name(message: &str) -> Option<String> {
    for prefix in ["unknown identifier ", "unknown type "] {
        if let Some(rest) = message.strip_prefix(prefix) {
            let symbol = rest.trim();
            if !symbol.is_empty() {
                return Some(symbol.to_string());
            }
        }
    }
    None
}

fn import_candidates_for_symbol(index: &WorkspaceIndex, uri: &str, symbol: &str) -> Vec<String> {
    let mut out = Vec::new();
    for def in &index.defs {
        if def.uri == uri {
            continue;
        }
        if def.def.name != symbol {
            continue;
        }
        if !is_exported_def_kind(def.def.kind) {
            continue;
        }
        if let Some(path) = module_import_path_between(uri, &def.uri) {
            out.push(path);
        }
    }
    if is_std_error_symbol(symbol) {
        out.push("std.Error".to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn module_import_path_between(from_uri: &str, to_uri: &str) -> Option<String> {
    let from = uri_to_path(from_uri)?;
    let to = uri_to_path(to_uri)?;
    let from_dir = from.parent()?;
    let to_no_ext = to.with_extension("");

    let mut base = from_dir;
    let mut up_count = 0usize;
    loop {
        if let Ok(rest) = to_no_ext.strip_prefix(base) {
            let rest = rest.to_string_lossy().replace('\\', "/");
            if rest.is_empty() {
                return None;
            }
            if up_count == 0 {
                return Some(format!("./{rest}"));
            }
            return Some(format!("{}{}", "../".repeat(up_count), rest));
        }
        base = base.parent()?;
        up_count += 1;
    }
}

fn is_std_error_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "Error"
            | "ValidationField"
            | "Validation"
            | "BadRequest"
            | "Unauthorized"
            | "Forbidden"
            | "NotFound"
            | "Conflict"
    )
}

fn missing_import_workspace_edit(
    uri: &str,
    text: &str,
    imports: &[ImportDecl],
    module_path: &str,
    symbol: &str,
) -> Option<JsonValue> {
    if let Some(existing) = imports.iter().find(|decl| match &decl.spec {
        ImportSpec::NamedFrom { path, .. } => path.value == module_path,
        _ => false,
    }) {
        let ImportSpec::NamedFrom { names, .. } = &existing.spec else {
            return None;
        };
        let mut merged: Vec<String> = names.iter().map(|ident| ident.name.clone()).collect();
        if merged.iter().any(|name| name == symbol) {
            return None;
        }
        merged.push(symbol.to_string());
        merged.sort();
        merged.dedup();
        let line = render_named_import(module_path, &merged);
        return Some(workspace_edit_with_single_span(
            uri,
            text,
            existing.span,
            &line,
        ));
    }

    if import_already_binds_symbol(imports, symbol) {
        return None;
    }
    let line = render_named_import(module_path, &[symbol.to_string()]);
    let insert_offset = imports.iter().map(|decl| decl.span.end).max().unwrap_or(0);
    let mut new_text = String::new();
    if insert_offset > 0 && !text[..insert_offset].ends_with('\n') {
        new_text.push('\n');
    }
    new_text.push_str(&line);
    new_text.push('\n');
    if insert_offset == 0 && !text.is_empty() {
        new_text.push('\n');
    }
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(insert_offset, insert_offset),
        &new_text,
    ))
}

fn organize_imports_workspace_edit(
    uri: &str,
    text: &str,
    imports: &[ImportDecl],
) -> Option<JsonValue> {
    if imports.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = imports.iter().map(render_import_decl).collect();
    lines.sort();
    lines.dedup();
    let first = imports.iter().map(|decl| decl.span.start).min()?;
    let mut end = imports.iter().map(|decl| decl.span.end).max()?;
    if end < text.len() && text.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    let replacement = if end < text.len() {
        format!("{}\n\n", lines.join("\n"))
    } else {
        format!("{}\n", lines.join("\n"))
    };
    if text.get(first..end) == Some(replacement.as_str()) {
        return None;
    }
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(first, end),
        &replacement,
    ))
}

fn import_already_binds_symbol(imports: &[ImportDecl], symbol: &str) -> bool {
    for decl in imports {
        match &decl.spec {
            ImportSpec::Module { name } | ImportSpec::ModuleFrom { name, .. } => {
                if name.name == symbol {
                    return true;
                }
            }
            ImportSpec::AliasFrom { alias, .. } => {
                if alias.name == symbol {
                    return true;
                }
            }
            ImportSpec::NamedFrom { names, .. } => {
                if names.iter().any(|name| name.name == symbol) {
                    return true;
                }
            }
        }
    }
    false
}

fn render_import_decl(decl: &ImportDecl) -> String {
    match &decl.spec {
        ImportSpec::Module { name } => format!("import {}", name.name),
        ImportSpec::ModuleFrom { name, path } => {
            format!(
                "import {} from {}",
                name.name,
                render_import_path(&path.value)
            )
        }
        ImportSpec::AliasFrom { name, alias, path } => format!(
            "import {} as {} from {}",
            name.name,
            alias.name,
            render_import_path(&path.value)
        ),
        ImportSpec::NamedFrom { names, path } => {
            let mut symbols: Vec<String> = names.iter().map(|name| name.name.clone()).collect();
            symbols.sort();
            symbols.dedup();
            render_named_import(&path.value, &symbols)
        }
    }
}

fn render_named_import(path: &str, names: &[String]) -> String {
    format!(
        "import {{ {} }} from {}",
        names.join(", "),
        render_import_path(path)
    )
}

fn render_import_path(path: &str) -> String {
    if path.starts_with("./")
        || path.starts_with("../")
        || path.contains('/')
        || path.contains('\\')
    {
        return format!("\"{}\"", path);
    }
    path.to_string()
}

fn workspace_edit_with_single_span(uri: &str, text: &str, span: Span, new_text: &str) -> JsonValue {
    let edit = text_edit_json(text, span, new_text);
    let mut changes = BTreeMap::new();
    changes.insert(uri.to_string(), JsonValue::Array(vec![edit]));
    let mut root = BTreeMap::new();
    root.insert("changes".to_string(), JsonValue::Object(changes));
    JsonValue::Object(root)
}

fn text_edit_json(text: &str, span: Span, new_text: &str) -> JsonValue {
    let offsets = line_offsets(text);
    let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
    let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
    let mut edit = BTreeMap::new();
    edit.insert(
        "range".to_string(),
        range_json(start_line, start_col, end_line, end_col),
    );
    edit.insert(
        "newText".to_string(),
        JsonValue::String(new_text.to_string()),
    );
    JsonValue::Object(edit)
}

fn code_action_json(title: &str, kind: &str, edit: JsonValue) -> JsonValue {
    let mut out = BTreeMap::new();
    out.insert("title".to_string(), JsonValue::String(title.to_string()));
    out.insert("kind".to_string(), JsonValue::String(kind.to_string()));
    out.insert("edit".to_string(), edit);
    JsonValue::Object(out)
}

fn handle_call_hierarchy_outgoing(
    state: &LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(index) = build_workspace_index_for_call_hierarchy(state, obj) else {
        return JsonValue::Null;
    };
    let Some(def_id) = call_hierarchy_target_def_id(&index, obj) else {
        return JsonValue::Null;
    };
    let mut result = Vec::new();
    for (to_id, sites) in index.outgoing_calls(def_id) {
        let Some(to_def) = index.def_for_target(to_id) else {
            continue;
        };
        let Some(to_item) = call_hierarchy_item_json(&index, &to_def) else {
            continue;
        };
        let mut ranges = Vec::new();
        let mut seen = HashSet::new();
        for site in sites {
            if !seen.insert((site.span.start, site.span.end)) {
                continue;
            }
            if let Some(range) = index.span_range_json(&site.uri, site.span) {
                ranges.push(range);
            }
        }
        let mut item = BTreeMap::new();
        item.insert("to".to_string(), to_item);
        item.insert("fromRanges".to_string(), JsonValue::Array(ranges));
        result.push(JsonValue::Object(item));
    }
    JsonValue::Array(result)
}

fn build_workspace_index_for_call_hierarchy(
    state: &LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> Option<WorkspaceIndex> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let item = params.get("item")?;
    let JsonValue::Object(item) = item else {
        return None;
    };
    let uri = match item.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    build_workspace_index(state, &uri)
}

fn call_hierarchy_target_def_id(
    index: &WorkspaceIndex,
    obj: &BTreeMap<String, JsonValue>,
) -> Option<usize> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let item = params.get("item")?;
    let JsonValue::Object(item) = item else {
        return None;
    };
    if let Some(def_id) = item.get("data").and_then(|value| match value {
        JsonValue::Number(num) if *num >= 0.0 => Some(*num as usize),
        _ => None,
    }) {
        return Some(def_id);
    }
    let uri = match item.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    let selection_range = item.get("selectionRange").or_else(|| item.get("range"))?;
    let JsonValue::Object(selection_range) = selection_range else {
        return None;
    };
    let start = selection_range.get("start")?;
    let JsonValue::Object(start) = start else {
        return None;
    };
    let line = match start.get("line") {
        Some(JsonValue::Number(line)) => *line as usize,
        _ => return None,
    };
    let character = match start.get("character") {
        Some(JsonValue::Number(character)) => *character as usize,
        _ => return None,
    };
    let def = index.definition_at(&uri, line, character)?;
    Some(def.id)
}

fn call_hierarchy_item_json(index: &WorkspaceIndex, def: &WorkspaceDef) -> Option<JsonValue> {
    let text = index.file_text(&def.uri)?;
    let range = span_range_json(text, def.def.span);
    let mut out = BTreeMap::new();
    out.insert("name".to_string(), JsonValue::String(def.def.name.clone()));
    out.insert(
        "kind".to_string(),
        JsonValue::Number(def.def.kind.lsp_kind() as f64),
    );
    out.insert("uri".to_string(), JsonValue::String(def.uri.clone()));
    out.insert("range".to_string(), range.clone());
    out.insert("selectionRange".to_string(), range);
    out.insert("data".to_string(), JsonValue::Number(def.id as f64));
    if !def.def.detail.is_empty() {
        out.insert(
            "detail".to_string(),
            JsonValue::String(def.def.detail.clone()),
        );
    }
    Some(JsonValue::Object(out))
}

fn location_json(uri: &str, text: &str, span: Span) -> JsonValue {
    let range = span_range_json(text, span);
    let mut out = BTreeMap::new();
    out.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    out.insert("range".to_string(), range);
    JsonValue::Object(out)
}

fn span_range_json(text: &str, span: Span) -> JsonValue {
    let offsets = line_offsets(text);
    let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
    let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
    range_json(start_line, start_col, end_line, end_col)
}

fn symbol_info_json(uri: &str, text: &str, def: &SymbolDef) -> JsonValue {
    let location = location_json(uri, text, def.span);
    let mut out = BTreeMap::new();
    out.insert("name".to_string(), JsonValue::String(def.name.clone()));
    out.insert(
        "kind".to_string(),
        JsonValue::Number(def.kind.lsp_kind() as f64),
    );
    out.insert("location".to_string(), location);
    if let Some(container) = &def.container {
        out.insert(
            "containerName".to_string(),
            JsonValue::String(container.clone()),
        );
    }
    JsonValue::Object(out)
}

fn is_valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

struct Index {
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    calls: Vec<CallRef>,
    qualified_calls: Vec<QualifiedCallRef>,
}

impl Index {
    fn definition_at(&self, offset: usize) -> Option<usize> {
        if let Some(def_id) = self.reference_at(offset) {
            return Some(def_id);
        }
        self.def_at(offset)
    }

    fn reference_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for reference in &self.refs {
            if span_contains(reference.span, offset) {
                let size = reference.span.end.saturating_sub(reference.span.start);
                if best.map_or(true, |(_, best_size)| size < best_size) {
                    best = Some((reference.target, size));
                }
            }
        }
        best.map(|(def_id, _)| def_id)
    }

    fn def_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (id, def) in self.defs.iter().enumerate() {
            if span_contains(def.span, offset) {
                let size = def.span.end.saturating_sub(def.span.start);
                if best.map_or(true, |(_, best_size)| size < best_size) {
                    best = Some((id, size));
                }
            }
        }
        best.map(|(id, _)| id)
    }
}

#[derive(Clone)]
struct SymbolDef {
    name: String,
    span: Span,
    kind: SymbolKind,
    detail: String,
    doc: Option<String>,
    container: Option<String>,
}

struct SymbolRef {
    span: Span,
    target: usize,
}

struct CallRef {
    caller: usize,
    callee: usize,
    span: Span,
}

struct QualifiedCallRef {
    caller: usize,
    module: String,
    item: String,
    span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Module,
    Type,
    Enum,
    EnumVariant,
    Function,
    Config,
    Service,
    App,
    Migration,
    Test,
    Param,
    Variable,
    Field,
}

impl SymbolKind {
    fn lsp_kind(self) -> u32 {
        match self {
            SymbolKind::Module => 2,
            SymbolKind::Type => 23,
            SymbolKind::Enum => 10,
            SymbolKind::EnumVariant => 22,
            SymbolKind::Function => 12,
            SymbolKind::Config => 23,
            SymbolKind::Service => 11,
            SymbolKind::App => 5,
            SymbolKind::Migration => 12,
            SymbolKind::Test => 12,
            SymbolKind::Param => 13,
            SymbolKind::Variable => 13,
            SymbolKind::Field => 8,
        }
    }

    fn hover_label(self) -> &'static str {
        match self {
            SymbolKind::Module => "Module",
            SymbolKind::Type => "Type",
            SymbolKind::Enum => "Enum",
            SymbolKind::EnumVariant => "Enum Variant",
            SymbolKind::Function => "Function",
            SymbolKind::Config => "Config",
            SymbolKind::Service => "Service",
            SymbolKind::App => "App",
            SymbolKind::Migration => "Migration",
            SymbolKind::Test => "Test",
            SymbolKind::Param => "Parameter",
            SymbolKind::Variable => "Variable",
            SymbolKind::Field => "Field",
        }
    }
}

fn span_contains(span: Span, offset: usize) -> bool {
    offset >= span.start && offset <= span.end
}

struct WorkspaceIndex {
    files: Vec<WorkspaceFile>,
    file_by_uri: HashMap<String, usize>,
    defs: Vec<WorkspaceDef>,
    refs: Vec<WorkspaceRef>,
    calls: Vec<WorkspaceCall>,
    module_alias_exports: HashMap<String, HashMap<String, HashSet<String>>>,
    redirects: HashMap<usize, usize>,
}

struct WorkspaceFile {
    uri: String,
    text: String,
    index: Index,
    def_map: Vec<usize>,
    qualified_refs: Vec<QualifiedRef>,
}

#[derive(Clone)]
struct WorkspaceDef {
    id: usize,
    uri: String,
    def: SymbolDef,
}

struct WorkspaceRef {
    uri: String,
    span: Span,
    target: usize,
}

#[derive(Clone)]
struct WorkspaceCall {
    uri: String,
    span: Span,
    from: usize,
    to: usize,
}

struct QualifiedRef {
    span: Span,
    target: usize,
}

impl WorkspaceIndex {
    fn definition_at(&self, uri: &str, line: usize, character: usize) -> Option<WorkspaceDef> {
        let file_idx = *self.file_by_uri.get(uri)?;
        let file = &self.files[file_idx];
        let offsets = line_offsets(&file.text);
        let offset = line_col_to_offset(&file.text, &offsets, line, character);
        if let Some(target) = best_ref_target(&file.qualified_refs, offset) {
            let def = self.def_for_target(target)?;
            return Some(def);
        }
        let local_def_id = file.index.definition_at(offset)?;
        let mut def_id = *file.def_map.get(local_def_id)?;
        while let Some(next) = self.redirects.get(&def_id) {
            if *next == def_id {
                break;
            }
            def_id = *next;
        }
        let def = self.def_for_target(def_id)?;
        Some(def)
    }

    fn rename_edits(&self, def_id: usize, new_name: &str) -> HashMap<String, Vec<JsonValue>> {
        let mut spans_by_uri: HashMap<String, Vec<Span>> = HashMap::new();
        if let Some(def) = self.def_for_target(def_id) {
            spans_by_uri
                .entry(def.uri.clone())
                .or_default()
                .push(def.def.span);
        }
        for reference in &self.refs {
            if reference.target == def_id {
                spans_by_uri
                    .entry(reference.uri.clone())
                    .or_default()
                    .push(reference.span);
            }
        }
        let mut edits_by_uri = HashMap::new();
        for (uri, spans) in spans_by_uri {
            let Some(text) = self.file_text(&uri) else {
                continue;
            };
            let offsets = line_offsets(text);
            let mut edits = Vec::new();
            let mut seen = HashSet::new();
            for span in spans {
                if !seen.insert((span.start, span.end)) {
                    continue;
                }
                let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
                let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
                let range = range_json(start_line, start_col, end_line, end_col);
                let mut edit = BTreeMap::new();
                edit.insert("range".to_string(), range);
                edit.insert(
                    "newText".to_string(),
                    JsonValue::String(new_name.to_string()),
                );
                edits.push(JsonValue::Object(edit));
            }
            if !edits.is_empty() {
                edits_by_uri.insert(uri, edits);
            }
        }
        edits_by_uri
    }

    fn reference_locations(&self, def_id: usize, include_declaration: bool) -> Vec<JsonValue> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if include_declaration {
            if let Some(def) = self.def_for_target(def_id) {
                if let Some(text) = self.file_text(&def.uri) {
                    let key = (def.uri.clone(), def.def.span.start, def.def.span.end);
                    if seen.insert(key) {
                        out.push(location_json(&def.uri, text, def.def.span));
                    }
                }
            }
        }
        for reference in &self.refs {
            if reference.target != def_id {
                continue;
            }
            let key = (
                reference.uri.clone(),
                reference.span.start,
                reference.span.end,
            );
            if !seen.insert(key) {
                continue;
            }
            let Some(text) = self.file_text(&reference.uri) else {
                continue;
            };
            out.push(location_json(&reference.uri, text, reference.span));
        }
        out
    }

    fn span_range_json(&self, uri: &str, span: Span) -> Option<JsonValue> {
        let text = self.file_text(uri)?;
        Some(span_range_json(text, span))
    }

    fn incoming_calls(&self, target: usize) -> Vec<(usize, Vec<WorkspaceCall>)> {
        let mut grouped: HashMap<usize, Vec<WorkspaceCall>> = HashMap::new();
        for call in &self.calls {
            if call.to != target {
                continue;
            }
            grouped.entry(call.from).or_default().push(call.clone());
        }
        let mut out: Vec<(usize, Vec<WorkspaceCall>)> = grouped.into_iter().collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    fn outgoing_calls(&self, source: usize) -> Vec<(usize, Vec<WorkspaceCall>)> {
        let mut grouped: HashMap<usize, Vec<WorkspaceCall>> = HashMap::new();
        for call in &self.calls {
            if call.from != source {
                continue;
            }
            grouped.entry(call.to).or_default().push(call.clone());
        }
        let mut out: Vec<(usize, Vec<WorkspaceCall>)> = grouped.into_iter().collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    fn alias_modules_for_symbol(&self, uri: &str, symbol: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(aliases) = self.module_alias_exports.get(uri) else {
            return out;
        };
        for (alias, exports) in aliases {
            if exports.contains(symbol) {
                out.push(alias.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn def_for_target(&self, target: usize) -> Option<WorkspaceDef> {
        self.defs.get(target).cloned()
    }

    fn file_text(&self, uri: &str) -> Option<&str> {
        let idx = *self.file_by_uri.get(uri)?;
        Some(self.files[idx].text.as_str())
    }
}

fn best_ref_target(refs: &[QualifiedRef], offset: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for reference in refs {
        if span_contains(reference.span, offset) {
            let size = reference.span.end.saturating_sub(reference.span.start);
            if best.map_or(true, |(_, best_size)| size <= best_size) {
                best = Some((reference.target, size));
            }
        }
    }
    best.map(|(target, _)| target)
}

fn build_index_with_program(text: &str, program: &Program) -> Index {
    let mut builder = IndexBuilder::new(text);
    builder.collect(program);
    builder.finish()
}

fn build_workspace_index(state: &LspState, focus_uri: &str) -> Option<WorkspaceIndex> {
    let focus_path = if !focus_uri.is_empty() {
        uri_to_path(focus_uri)
    } else {
        None
    };
    let root_path = focus_path
        .clone()
        .or_else(|| state.root_uri.as_deref().and_then(uri_to_path))?;
    let entry_path = resolve_entry_path(&root_path, focus_path.as_deref())?;
    let mut overrides = HashMap::new();
    for (uri, text) in &state.docs {
        if let Some(path) = uri_to_path(uri) {
            let key = path.canonicalize().unwrap_or(path);
            overrides.insert(key, text.clone());
        }
    }
    let entry_key = entry_path
        .canonicalize()
        .unwrap_or_else(|_| entry_path.clone());
    let root_text = overrides
        .get(&entry_key)
        .cloned()
        .or_else(|| std::fs::read_to_string(&entry_path).ok())?;
    let (registry, _diags) = load_program_with_modules_and_deps_and_overrides(
        &entry_path,
        &root_text,
        &HashMap::new(),
        &overrides,
    );
    build_workspace_from_registry(&registry, &overrides)
}

fn resolve_entry_path(root_path: &Path, focus: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = focus {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    if root_path.is_file() {
        return Some(root_path.to_path_buf());
    }
    if root_path.is_dir() {
        if let Some(entry) = read_manifest_entry(root_path) {
            return Some(entry);
        }
        let candidate = root_path.join("src").join("main.fuse");
        if candidate.exists() {
            return Some(candidate);
        }
        if let Some(first) = find_first_fuse_file(root_path) {
            return Some(first);
        }
    }
    None
}

fn read_manifest_entry(root: &Path) -> Option<PathBuf> {
    let manifest = root.join("fuse.toml");
    let contents = std::fs::read_to_string(&manifest).ok()?;
    let mut in_package = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = parts.next()?.trim();
        if key != "entry" {
            continue;
        }
        let value = parts.next()?.trim();
        let value = value.trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            continue;
        }
        return Some(root.join(value));
    }
    None
}

fn find_first_fuse_file(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let ignore_dirs = [
        ".git",
        ".fuse",
        "target",
        "tmp",
        "dist",
        "build",
        ".cargo-target",
        ".cargo-tmp",
        "node_modules",
    ];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if ignore_dirs.contains(&name) {
                        continue;
                    }
                }
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("fuse") {
                return Some(path);
            }
        }
    }
    None
}

fn build_workspace_from_registry(
    registry: &ModuleRegistry,
    overrides: &HashMap<PathBuf, String>,
) -> Option<WorkspaceIndex> {
    let mut files = Vec::new();
    let mut file_by_uri = HashMap::new();
    let mut module_to_file = HashMap::new();
    let mut modules_sorted: Vec<(usize, &fusec::loader::ModuleUnit)> = registry
        .modules
        .iter()
        .map(|(id, unit)| (*id, unit))
        .collect();
    modules_sorted.sort_by_key(|(id, _)| *id);
    for (id, unit) in modules_sorted {
        let path_str = unit.path.to_string_lossy();
        if path_str.starts_with('<') {
            continue;
        }
        let uri = path_to_uri(&unit.path);
        let key = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        let text = overrides
            .get(&key)
            .cloned()
            .or_else(|| std::fs::read_to_string(&unit.path).ok())
            .unwrap_or_default();
        let index = build_index_with_program(&text, &unit.program);
        let def_map = vec![0; index.defs.len()];
        let file_idx = files.len();
        files.push(WorkspaceFile {
            uri: uri.clone(),
            text,
            index,
            def_map,
            qualified_refs: Vec::new(),
        });
        file_by_uri.insert(uri.clone(), file_idx);
        module_to_file.insert(id, file_idx);
    }

    let mut defs = Vec::new();
    for file in files.iter_mut() {
        for (local_id, def) in file.index.defs.iter().enumerate() {
            let global_id = defs.len();
            defs.push(WorkspaceDef {
                id: global_id,
                uri: file.uri.clone(),
                def: def.clone(),
            });
            file.def_map[local_id] = global_id;
        }
    }

    let mut refs = Vec::new();
    for file in &files {
        for reference in &file.index.refs {
            if let Some(global_id) = file.def_map.get(reference.target) {
                refs.push(WorkspaceRef {
                    uri: file.uri.clone(),
                    span: reference.span,
                    target: *global_id,
                });
            }
        }
    }
    let mut calls = Vec::new();
    let mut module_alias_exports = HashMap::new();

    let mut exports_by_module: HashMap<usize, HashMap<String, usize>> = HashMap::new();
    for (module_id, file_idx) in &module_to_file {
        let file = &files[*file_idx];
        let mut exports = HashMap::new();
        for (local_id, def) in file.index.defs.iter().enumerate() {
            if !is_exported_def_kind(def.kind) {
                continue;
            }
            let global_id = file.def_map[local_id];
            exports.entry(def.name.clone()).or_insert(global_id);
        }
        exports_by_module.insert(*module_id, exports);
    }

    let mut redirects = HashMap::new();
    let mut modules_sorted: Vec<(usize, &fusec::loader::ModuleUnit)> = registry
        .modules
        .iter()
        .map(|(id, unit)| (*id, unit))
        .collect();
    modules_sorted.sort_by_key(|(id, _)| *id);
    for (module_id, unit) in modules_sorted {
        let Some(file_idx) = module_to_file.get(&module_id) else {
            continue;
        };
        let file = &mut files[*file_idx];
        for (name, link) in &unit.import_items {
            let Some(exports) = exports_by_module.get(&link.id) else {
                continue;
            };
            let Some(target) = exports.get(name) else {
                continue;
            };
            if let Some(local_def_id) = find_import_def(&file.index, name) {
                let global_id = file.def_map[local_def_id];
                redirects.insert(global_id, *target);
                refs.push(WorkspaceRef {
                    uri: file.uri.clone(),
                    span: file.index.defs[local_def_id].span,
                    target: *target,
                });
            }
        }

        let module_aliases: HashMap<String, usize> = unit
            .modules
            .modules
            .iter()
            .map(|(name, link)| (name.clone(), link.id))
            .collect();
        let mut alias_exports = HashMap::new();
        for (alias, module_id) in &module_aliases {
            if let Some(exports) = exports_by_module.get(module_id) {
                alias_exports.insert(alias.clone(), exports.keys().cloned().collect());
            }
        }
        if !alias_exports.is_empty() {
            module_alias_exports.insert(file.uri.clone(), alias_exports);
        }

        for call in &file.index.calls {
            let Some(from) = file.def_map.get(call.caller).copied() else {
                continue;
            };
            let Some(to) = file.def_map.get(call.callee).copied() else {
                continue;
            };
            calls.push(WorkspaceCall {
                uri: file.uri.clone(),
                span: call.span,
                from,
                to,
            });
        }

        for call in &file.index.qualified_calls {
            let Some(module_id) = module_aliases.get(&call.module) else {
                continue;
            };
            let Some(exports) = exports_by_module.get(module_id) else {
                continue;
            };
            let Some(target) = exports.get(&call.item) else {
                continue;
            };
            let Some(from) = file.def_map.get(call.caller).copied() else {
                continue;
            };
            calls.push(WorkspaceCall {
                uri: file.uri.clone(),
                span: call.span,
                from,
                to: *target,
            });
        }

        let qualified_refs = collect_qualified_refs(&unit.program);
        for qualified in qualified_refs {
            let Some(module_id) = module_aliases.get(&qualified.module) else {
                continue;
            };
            let Some(exports) = exports_by_module.get(module_id) else {
                continue;
            };
            let Some(target) = exports.get(&qualified.item) else {
                continue;
            };
            file.qualified_refs.push(QualifiedRef {
                span: qualified.span,
                target: *target,
            });
            refs.push(WorkspaceRef {
                uri: file.uri.clone(),
                span: qualified.span,
                target: *target,
            });
        }
    }

    for reference in refs.iter_mut() {
        let mut target = reference.target;
        while let Some(next) = redirects.get(&target) {
            if *next == target {
                break;
            }
            target = *next;
        }
        reference.target = target;
    }
    for call in calls.iter_mut() {
        while let Some(next) = redirects.get(&call.from) {
            if *next == call.from {
                break;
            }
            call.from = *next;
        }
        while let Some(next) = redirects.get(&call.to) {
            if *next == call.to {
                break;
            }
            call.to = *next;
        }
    }
    calls.retain(|call| {
        let Some(from) = defs.get(call.from) else {
            return false;
        };
        let Some(to) = defs.get(call.to) else {
            return false;
        };
        is_callable_def_kind(from.def.kind) && is_callable_def_kind(to.def.kind)
    });

    Some(WorkspaceIndex {
        files,
        file_by_uri,
        defs,
        refs,
        calls,
        module_alias_exports,
        redirects,
    })
}

fn is_exported_def_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Type
            | SymbolKind::Enum
            | SymbolKind::Function
            | SymbolKind::Config
            | SymbolKind::Service
            | SymbolKind::App
            | SymbolKind::Migration
            | SymbolKind::Test
    )
}

fn is_callable_def_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Service
            | SymbolKind::App
            | SymbolKind::Migration
            | SymbolKind::Test
    )
}

fn find_import_def(index: &Index, name: &str) -> Option<usize> {
    index.defs.iter().enumerate().find_map(|(idx, def)| {
        if def.kind != SymbolKind::Variable {
            return None;
        }
        if def.name != name {
            return None;
        }
        if def.detail.starts_with("import ") {
            return Some(idx);
        }
        None
    })
}

struct QualifiedNameRef {
    span: Span,
    module: String,
    item: String,
}

fn collect_qualified_refs(program: &Program) -> Vec<QualifiedNameRef> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) => {
                for field in &decl.fields {
                    collect_qualified_type_ref(&field.ty, &mut out);
                }
            }
            Item::Enum(decl) => {
                for variant in &decl.variants {
                    for ty in &variant.payload {
                        collect_qualified_type_ref(ty, &mut out);
                    }
                }
            }
            Item::Fn(decl) => {
                for param in &decl.params {
                    collect_qualified_type_ref(&param.ty, &mut out);
                }
                if let Some(ret) = &decl.ret {
                    collect_qualified_type_ref(ret, &mut out);
                }
                collect_qualified_block(&decl.body, &mut out);
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_qualified_type_ref(&route.ret_type, &mut out);
                    if let Some(body) = &route.body_type {
                        collect_qualified_type_ref(body, &mut out);
                    }
                    collect_qualified_block(&route.body, &mut out);
                }
            }
            Item::Config(decl) => {
                for field in &decl.fields {
                    collect_qualified_type_ref(&field.ty, &mut out);
                    collect_qualified_expr(&field.value, &mut out);
                }
            }
            Item::App(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Migration(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Test(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Import(_) => {}
        }
    }
    out
}

fn collect_qualified_block(block: &Block, out: &mut Vec<QualifiedNameRef>) {
    for stmt in &block.stmts {
        collect_qualified_stmt(stmt, out);
    }
}

fn collect_qualified_stmt(stmt: &Stmt, out: &mut Vec<QualifiedNameRef>) {
    match &stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(ty) = ty {
                collect_qualified_type_ref(ty, out);
            }
            collect_qualified_expr(expr, out);
        }
        StmtKind::Assign { target, expr } => {
            collect_qualified_expr(target, out);
            collect_qualified_expr(expr, out);
        }
        StmtKind::Return { expr } => {
            if let Some(expr) = expr {
                collect_qualified_expr(expr, out);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            collect_qualified_expr(cond, out);
            collect_qualified_block(then_block, out);
            for (cond, block) in else_if {
                collect_qualified_expr(cond, out);
                collect_qualified_block(block, out);
            }
            if let Some(block) = else_block {
                collect_qualified_block(block, out);
            }
        }
        StmtKind::Match { expr, cases } => {
            collect_qualified_expr(expr, out);
            for (pat, block) in cases {
                collect_qualified_pattern(pat, out);
                collect_qualified_block(block, out);
            }
        }
        StmtKind::For { pat, iter, block } => {
            collect_qualified_pattern(pat, out);
            collect_qualified_expr(iter, out);
            collect_qualified_block(block, out);
        }
        StmtKind::While { cond, block } => {
            collect_qualified_expr(cond, out);
            collect_qualified_block(block, out);
        }
        StmtKind::Expr(expr) => collect_qualified_expr(expr, out),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_qualified_expr(expr: &Expr, out: &mut Vec<QualifiedNameRef>) {
    match &expr.kind {
        ExprKind::Literal(_) => {}
        ExprKind::Ident(_) => {}
        ExprKind::Binary { left, right, .. } => {
            collect_qualified_expr(left, out);
            collect_qualified_expr(right, out);
        }
        ExprKind::Unary { expr, .. } => collect_qualified_expr(expr, out),
        ExprKind::Call { callee, args } => {
            collect_qualified_expr(callee, out);
            for arg in args {
                collect_qualified_expr(&arg.value, out);
            }
        }
        ExprKind::Member { base, name } => {
            if let ExprKind::Ident(ident) = &base.kind {
                if let Some((module, item)) =
                    split_qualified_name(&format!("{}.{}", ident.name, name.name))
                {
                    out.push(QualifiedNameRef {
                        span: name.span,
                        module: module.to_string(),
                        item: item.to_string(),
                    });
                }
            }
            collect_qualified_expr(base, out);
        }
        ExprKind::OptionalMember { base, name } => {
            if let ExprKind::Ident(ident) = &base.kind {
                if let Some((module, item)) =
                    split_qualified_name(&format!("{}.{}", ident.name, name.name))
                {
                    out.push(QualifiedNameRef {
                        span: name.span,
                        module: module.to_string(),
                        item: item.to_string(),
                    });
                }
            }
            collect_qualified_expr(base, out);
        }
        ExprKind::StructLit { name, fields } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for field in fields {
                collect_qualified_expr(&field.value, out);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_qualified_expr(item, out);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_qualified_expr(key, out);
                collect_qualified_expr(value, out);
            }
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            collect_qualified_expr(base, out);
            collect_qualified_expr(index, out);
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_qualified_expr(expr, out);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_qualified_expr(left, out);
            collect_qualified_expr(right, out);
        }
        ExprKind::BangChain { expr, error } => {
            collect_qualified_expr(expr, out);
            if let Some(err) = error {
                collect_qualified_expr(err, out);
            }
        }
        ExprKind::Spawn { block } => collect_qualified_block(block, out),
        ExprKind::Await { expr } => collect_qualified_expr(expr, out),
        ExprKind::Box { expr } => collect_qualified_expr(expr, out),
    }
}

fn collect_qualified_pattern(pattern: &Pattern, out: &mut Vec<QualifiedNameRef>) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Ident(_) => {}
        PatternKind::EnumVariant { name, args } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_pattern(arg, out);
            }
        }
        PatternKind::Struct { name, fields } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for field in fields {
                collect_qualified_pattern(&field.pat, out);
            }
        }
    }
}

fn collect_qualified_type_ref(ty: &TypeRef, out: &mut Vec<QualifiedNameRef>) {
    match &ty.kind {
        TypeRefKind::Simple(ident) => {
            if let Some((module, item)) = split_qualified_name(&ident.name) {
                out.push(QualifiedNameRef {
                    span: ident.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
        }
        TypeRefKind::Generic { base, args } => {
            if let Some((module, item)) = split_qualified_name(&base.name) {
                out.push(QualifiedNameRef {
                    span: base.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_type_ref(arg, out);
            }
        }
        TypeRefKind::Optional(inner) => collect_qualified_type_ref(inner, out),
        TypeRefKind::Result { ok, err } => {
            collect_qualified_type_ref(ok, out);
            if let Some(err) = err {
                collect_qualified_type_ref(err, out);
            }
        }
        TypeRefKind::Refined { base, args } => {
            if let Some((module, item)) = split_qualified_name(&base.name) {
                out.push(QualifiedNameRef {
                    span: base.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_expr(arg, out);
            }
        }
    }
}

fn split_qualified_name(name: &str) -> Option<(&str, &str)> {
    let mut iter = name.rsplitn(2, '.');
    let item = iter.next()?;
    let module = iter.next()?;
    if module.is_empty() || item.is_empty() {
        return None;
    }
    Some((module, item))
}

struct IndexBuilder<'a> {
    text: &'a str,
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    calls: Vec<CallRef>,
    qualified_calls: Vec<QualifiedCallRef>,
    scopes: Vec<HashMap<String, usize>>,
    globals: HashMap<String, usize>,
    app_defs: HashMap<String, usize>,
    migration_defs: HashMap<String, usize>,
    test_defs: HashMap<String, usize>,
    type_defs: HashMap<String, usize>,
    enum_variants: HashMap<String, usize>,
    enum_variant_ambiguous: HashSet<String>,
    enum_variants_by_enum: HashMap<String, HashMap<String, usize>>,
    current_callable: Option<usize>,
}

impl<'a> IndexBuilder<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            defs: Vec::new(),
            refs: Vec::new(),
            calls: Vec::new(),
            qualified_calls: Vec::new(),
            scopes: Vec::new(),
            globals: HashMap::new(),
            app_defs: HashMap::new(),
            migration_defs: HashMap::new(),
            test_defs: HashMap::new(),
            type_defs: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_ambiguous: HashSet::new(),
            enum_variants_by_enum: HashMap::new(),
            current_callable: None,
        }
    }

    fn finish(self) -> Index {
        Index {
            defs: self.defs,
            refs: self.refs,
            calls: self.calls,
            qualified_calls: self.qualified_calls,
        }
    }

    fn collect(&mut self, program: &Program) {
        self.collect_globals(program);
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn collect_globals(&mut self, program: &Program) {
        for item in &program.items {
            match item {
                Item::Import(decl) => self.define_import(decl),
                Item::Type(decl) => self.define_type(decl),
                Item::Enum(decl) => self.define_enum(decl),
                Item::Fn(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Function,
                        self.fn_signature(decl),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::Config(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Config,
                        format!("config {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::Service(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Service,
                        format!("service {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::App(decl) => {
                    let detail = format!("app \"{}\"", decl.name.value);
                    let def_id = self.define_literal_decl(
                        &decl.name,
                        SymbolKind::App,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.app_defs.insert(decl.name.value.clone(), def_id);
                }
                Item::Migration(decl) => {
                    let detail = format!("migration {}", decl.name);
                    let def_id = self.define_span_decl(
                        decl.span,
                        decl.name.clone(),
                        SymbolKind::Migration,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.migration_defs.insert(decl.name.clone(), def_id);
                }
                Item::Test(decl) => {
                    let detail = format!("test \"{}\"", decl.name.value);
                    let def_id = self.define_literal_decl(
                        &decl.name,
                        SymbolKind::Test,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.test_defs.insert(decl.name.value.clone(), def_id);
                }
            }
        }
    }

    fn define_import(&mut self, decl: &ImportDecl) {
        match &decl.spec {
            ImportSpec::Module { name } => {
                self.define_global(
                    name,
                    SymbolKind::Module,
                    format!("module {}", name.name),
                    None,
                    None,
                );
            }
            ImportSpec::ModuleFrom { name, .. } => {
                self.define_global(
                    name,
                    SymbolKind::Module,
                    format!("module {}", name.name),
                    None,
                    None,
                );
            }
            ImportSpec::AliasFrom { alias, .. } => {
                self.define_global(
                    alias,
                    SymbolKind::Module,
                    format!("module {}", alias.name),
                    None,
                    None,
                );
            }
            ImportSpec::NamedFrom { names, .. } => {
                for name in names {
                    self.define_global(
                        name,
                        SymbolKind::Variable,
                        format!("import {}", name.name),
                        None,
                        None,
                    );
                }
            }
        }
    }

    fn define_type(&mut self, decl: &TypeDecl) {
        let def_id = self.define_global(
            &decl.name,
            SymbolKind::Type,
            format!("type {}", decl.name.name),
            decl.doc.as_ref(),
            None,
        );
        self.type_defs.insert(decl.name.name.clone(), def_id);
    }

    fn define_enum(&mut self, decl: &EnumDecl) {
        let def_id = self.define_global(
            &decl.name,
            SymbolKind::Enum,
            format!("enum {}", decl.name.name),
            decl.doc.as_ref(),
            None,
        );
        self.type_defs.insert(decl.name.name.clone(), def_id);
        let mut variants = HashMap::new();
        for variant in &decl.variants {
            let detail = if variant.payload.is_empty() {
                format!("variant {}", variant.name.name)
            } else {
                let payload = variant
                    .payload
                    .iter()
                    .map(|ty| self.type_ref_text(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("variant {}({})", variant.name.name, payload)
            };
            let def_id = self.define_span_decl(
                variant.name.span,
                variant.name.name.clone(),
                SymbolKind::EnumVariant,
                detail,
                decl.doc.as_ref(),
            );
            variants.insert(variant.name.name.clone(), def_id);
            if self.enum_variant_ambiguous.contains(&variant.name.name) {
                continue;
            }
            if self.enum_variants.contains_key(&variant.name.name) {
                self.enum_variants.remove(&variant.name.name);
                self.enum_variant_ambiguous
                    .insert(variant.name.name.clone());
            } else {
                self.enum_variants.insert(variant.name.name.clone(), def_id);
            }
        }
        self.enum_variants_by_enum
            .insert(decl.name.name.clone(), variants);
    }

    fn visit_item(&mut self, item: &Item) {
        match item {
            Item::Import(_) => {}
            Item::Type(decl) => self.visit_type_decl(decl),
            Item::Enum(decl) => self.visit_enum_decl(decl),
            Item::Fn(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_fn_decl(decl);
                self.current_callable = prev;
            }
            Item::Config(decl) => self.visit_config_decl(decl),
            Item::Service(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_service_decl(decl);
                self.current_callable = prev;
            }
            Item::App(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.app_defs.get(&decl.name.value).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Migration(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.migration_defs.get(&decl.name).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Test(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.test_defs.get(&decl.name.value).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
        }
    }

    fn visit_type_decl(&mut self, decl: &TypeDecl) {
        for field in &decl.fields {
            self.visit_type_ref(&field.ty);
            if let Some(expr) = &field.default {
                self.visit_expr(expr);
            }
        }
        if let Some(TypeDerive { base, .. }) = &decl.derive {
            self.add_type_ref(base);
        }
    }

    fn visit_enum_decl(&mut self, decl: &EnumDecl) {
        for variant in &decl.variants {
            for ty in &variant.payload {
                self.visit_type_ref(ty);
            }
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.enter_scope();
        for param in &decl.params {
            let detail = format!(
                "param {}: {}",
                param.name.name,
                self.type_ref_text(&param.ty)
            );
            let def_id = self.define_local(&param.name, SymbolKind::Param, detail, None, None);
            self.insert_local(&param.name.name, def_id);
            self.visit_type_ref(&param.ty);
            if let Some(expr) = &param.default {
                self.visit_expr(expr);
            }
        }
        if let Some(ret) = &decl.ret {
            self.visit_type_ref(ret);
        }
        self.visit_block_body(&decl.body);
        self.exit_scope();
    }

    fn visit_config_decl(&mut self, decl: &ConfigDecl) {
        for field in &decl.fields {
            let detail = format!(
                "field {}: {}",
                field.name.name,
                self.type_ref_text(&field.ty)
            );
            self.define_span_decl(
                field.name.span,
                field.name.name.clone(),
                SymbolKind::Field,
                detail,
                None,
            );
            self.visit_type_ref(&field.ty);
            self.visit_expr(&field.value);
        }
    }

    fn visit_service_decl(&mut self, decl: &ServiceDecl) {
        for route in &decl.routes {
            self.visit_type_ref(&route.ret_type);
            if let Some(body_ty) = &route.body_type {
                self.visit_type_ref(body_ty);
            }
            self.enter_scope();
            if let Some(body_ty) = &route.body_type {
                let detail = format!("param body: {}", self.type_ref_text(body_ty));
                let span = route.body_span.unwrap_or(body_ty.span);
                let def_id = self.define_span_decl(
                    span,
                    "body".to_string(),
                    SymbolKind::Param,
                    detail,
                    None,
                );
                self.insert_local("body", def_id);
            }
            self.visit_block_body(&route.body);
            self.exit_scope();
        }
    }

    fn visit_block(&mut self, block: &Block) {
        self.enter_scope();
        self.visit_block_body(block);
        self.exit_scope();
    }

    fn visit_block_body(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.visit_type_ref(ty);
                }
                self.visit_expr(expr);
                let detail = match ty {
                    Some(ty) => format!("let {}: {}", name.name, self.type_ref_text(ty)),
                    None => format!("let {}", name.name),
                };
                let def_id = self.define_local(name, SymbolKind::Variable, detail, None, None);
                self.insert_local(&name.name, def_id);
            }
            StmtKind::Var { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.visit_type_ref(ty);
                }
                self.visit_expr(expr);
                let detail = match ty {
                    Some(ty) => format!("var {}: {}", name.name, self.type_ref_text(ty)),
                    None => format!("var {}", name.name),
                };
                let def_id = self.define_local(name, SymbolKind::Variable, detail, None, None);
                self.insert_local(&name.name, def_id);
            }
            StmtKind::Assign { target, expr } => {
                self.visit_expr(target);
                self.visit_expr(expr);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    self.visit_expr(expr);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                self.visit_expr(cond);
                self.visit_block(then_block);
                for (expr, block) in else_if {
                    self.visit_expr(expr);
                    self.visit_block(block);
                }
                if let Some(block) = else_block {
                    self.visit_block(block);
                }
            }
            StmtKind::Match { expr, cases } => {
                self.visit_expr(expr);
                for (pat, block) in cases {
                    self.enter_scope();
                    self.visit_pattern(pat);
                    self.visit_block_body(block);
                    self.exit_scope();
                }
            }
            StmtKind::For { pat, iter, block } => {
                self.visit_expr(iter);
                self.enter_scope();
                self.visit_pattern(pat);
                self.visit_block_body(block);
                self.exit_scope();
            }
            StmtKind::While { cond, block } => {
                self.visit_expr(cond);
                self.visit_block(block);
            }
            StmtKind::Expr(expr) => {
                self.visit_expr(expr);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Literal(_) => {}
            ExprKind::Ident(ident) => {
                if let Some(def_id) = self.resolve_value(&ident.name) {
                    self.add_ref(ident.span, def_id);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::Unary { expr, .. } => {
                self.visit_expr(expr);
            }
            ExprKind::Call { callee, args } => {
                self.record_call(callee);
                self.visit_expr(callee);
                for arg in args {
                    if let Some(name) = &arg.name {
                        if let Some(def_id) = self.resolve_value(&name.name) {
                            self.add_ref(name.span, def_id);
                        }
                    }
                    self.visit_expr(&arg.value);
                }
            }
            ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
                if let ExprKind::Ident(base_ident) = &base.kind {
                    if let Some(map) = self.enum_variants_by_enum.get(&base_ident.name) {
                        if let Some(def_id) = map.get(&name.name) {
                            self.add_ref(name.span, *def_id);
                        }
                    }
                }
                self.visit_expr(base);
            }
            ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
                self.visit_expr(base);
                self.visit_expr(index);
            }
            ExprKind::StructLit { name, fields } => {
                self.add_type_ref(name);
                for field in fields {
                    self.visit_expr(&field.value);
                }
            }
            ExprKind::ListLit(items) => {
                for item in items {
                    self.visit_expr(item);
                }
            }
            ExprKind::MapLit(items) => {
                for (key, value) in items {
                    self.visit_expr(key);
                    self.visit_expr(value);
                }
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    if let fusec::ast::InterpPart::Expr(expr) = part {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::Coalesce { left, right } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::BangChain { expr, error } => {
                self.visit_expr(expr);
                if let Some(err) = error {
                    self.visit_expr(err);
                }
            }
            ExprKind::Spawn { block } => self.visit_block(block),
            ExprKind::Await { expr } => self.visit_expr(expr),
            ExprKind::Box { expr } => self.visit_expr(expr),
        }
    }

    fn visit_type_ref(&mut self, ty: &TypeRef) {
        match &ty.kind {
            TypeRefKind::Simple(ident) => self.add_type_ref(ident),
            TypeRefKind::Generic { base, args } => {
                self.add_type_ref(base);
                for arg in args {
                    self.visit_type_ref(arg);
                }
            }
            TypeRefKind::Optional(inner) => self.visit_type_ref(inner),
            TypeRefKind::Result { ok, err } => {
                self.visit_type_ref(ok);
                if let Some(err) = err {
                    self.visit_type_ref(err);
                }
            }
            TypeRefKind::Refined { base, args } => {
                self.add_type_ref(base);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
        }
    }

    fn visit_pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
            PatternKind::Ident(ident) => {
                let detail = format!("let {}", ident.name);
                let def_id = self.define_local(ident, SymbolKind::Variable, detail, None, None);
                self.insert_local(&ident.name, def_id);
            }
            PatternKind::EnumVariant { name, args } => {
                if let Some(def_id) = self.enum_variants.get(&name.name) {
                    self.add_ref(name.span, *def_id);
                }
                for arg in args {
                    self.visit_pattern(arg);
                }
            }
            PatternKind::Struct { name, fields } => {
                self.add_type_ref(name);
                for field in fields {
                    self.visit_pattern(&field.pat);
                }
            }
        }
    }

    fn record_call(&mut self, callee: &Expr) {
        let Some(caller) = self.current_callable else {
            return;
        };
        if let Some(target) = self.call_target_local(callee) {
            self.calls.push(CallRef {
                caller,
                callee: target,
                span: callee.span,
            });
            return;
        }
        if let Some((module, item, span)) = self.call_target_qualified(callee) {
            self.qualified_calls.push(QualifiedCallRef {
                caller,
                module,
                item,
                span,
            });
        }
    }

    fn call_target_local(&self, callee: &Expr) -> Option<usize> {
        match &callee.kind {
            ExprKind::Ident(ident) => self.resolve_value(&ident.name),
            _ => None,
        }
    }

    fn call_target_qualified(&self, callee: &Expr) -> Option<(String, String, Span)> {
        let (base, name) = match &callee.kind {
            ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
                (base, name)
            }
            _ => return None,
        };
        let ExprKind::Ident(base_ident) = &base.kind else {
            return None;
        };
        let Some(base_def_id) = self.resolve_value(&base_ident.name) else {
            return None;
        };
        let Some(base_def) = self.defs.get(base_def_id) else {
            return None;
        };
        if base_def.kind != SymbolKind::Module {
            return None;
        }
        Some((base_ident.name.clone(), name.name.clone(), name.span))
    }

    fn add_type_ref(&mut self, ident: &Ident) {
        if ident.name.contains('.') {
            return;
        }
        if is_builtin_type(&ident.name) {
            return;
        }
        if let Some(def_id) = self
            .type_defs
            .get(&ident.name)
            .copied()
            .or_else(|| self.globals.get(&ident.name).copied())
        {
            self.add_ref(ident.span, def_id);
        }
    }

    fn resolve_value(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(def_id) = scope.get(name) {
                return Some(*def_id);
            }
        }
        self.globals.get(name).copied()
    }

    fn add_ref(&mut self, span: Span, target: usize) {
        self.refs.push(SymbolRef { span, target });
    }

    fn define_global(
        &mut self,
        ident: &Ident,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        if let Some(def_id) = self.globals.get(&ident.name) {
            return *def_id;
        }
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name: ident.name.clone(),
            span: ident.span,
            kind,
            detail,
            doc,
            container,
        });
        self.globals.insert(ident.name.clone(), def_id);
        def_id
    }

    fn define_literal_decl(
        &mut self,
        lit: &fusec::ast::StringLit,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
    ) -> usize {
        self.define_span_decl(lit.span, lit.value.clone(), kind, detail, doc)
    }

    fn define_span_decl(
        &mut self,
        span: Span,
        name: String,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
    ) -> usize {
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name,
            span,
            kind,
            detail,
            doc,
            container: None,
        });
        def_id
    }

    fn define_local(
        &mut self,
        ident: &Ident,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name: ident.name.clone(),
            span: ident.span,
            kind,
            detail,
            doc,
            container,
        });
        def_id
    }

    fn insert_local(&mut self, name: &str, def_id: usize) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), def_id);
        }
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn fn_signature(&self, decl: &FnDecl) -> String {
        let mut out = format!("fn {}(", decl.name.name);
        for (idx, param) in decl.params.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&param.name.name);
            out.push_str(": ");
            out.push_str(&self.type_ref_text(&param.ty));
        }
        out.push(')');
        if let Some(ret) = &decl.ret {
            out.push_str(" -> ");
            out.push_str(&self.type_ref_text(ret));
        }
        out
    }

    fn type_ref_text(&self, ty: &TypeRef) -> String {
        self.slice_span(ty.span).trim().to_string()
    }

    fn slice_span(&self, span: Span) -> String {
        self.text
            .get(span.start..span.end)
            .unwrap_or("")
            .to_string()
    }
}

fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Float"
            | "Bool"
            | "String"
            | "Bytes"
            | "Html"
            | "Id"
            | "Email"
            | "Error"
            | "List"
            | "Map"
            | "Option"
            | "Result"
    )
}
