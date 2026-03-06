use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use fuse_rt::json::{self, JsonValue};
use fusec::span::Span;

use super::symbols::{SymbolDef, SymbolKind};
use super::workspace::{WorkspaceCache, try_incremental_module_update};

pub(crate) const SEMANTIC_TOKEN_TYPES: [&str; 14] = [
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
    "htmlTag",
    "htmlAttribute",
];
pub(crate) const SEM_NAMESPACE: usize = 0;
pub(crate) const SEM_TYPE: usize = 1;
pub(crate) const SEM_ENUM: usize = 2;
pub(crate) const SEM_ENUM_MEMBER: usize = 3;
pub(crate) const SEM_FUNCTION: usize = 4;
pub(crate) const SEM_PARAMETER: usize = 5;
pub(crate) const SEM_VARIABLE: usize = 6;
pub(crate) const SEM_PROPERTY: usize = 7;
pub(crate) const SEM_KEYWORD: usize = 8;
pub(crate) const SEM_STRING: usize = 9;
pub(crate) const SEM_NUMBER: usize = 10;
pub(crate) const SEM_COMMENT: usize = 11;
pub(crate) const SEM_HTML_TAG: usize = 12;
pub(crate) const SEM_HTML_ATTRIBUTE: usize = 13;
pub(crate) const COMPLETION_KEYWORDS: [&str; 35] = [
    "app",
    "service",
    "at",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "fn",
    "type",
    "enum",
    "let",
    "var",
    "return",
    "if",
    "else",
    "match",
    "for",
    "in",
    "while",
    "break",
    "continue",
    "requires",
    "import",
    "from",
    "as",
    "config",
    "migration",
    "table",
    "test",
    "body",
    "and",
    "or",
    "without",
    "spawn",
];
pub(crate) const COMPLETION_BUILTIN_RECEIVERS: [&str; 8] = [
    "db", "json", "html", "svg", "request", "response", "time", "crypto",
];
pub(crate) const COMPLETION_BUILTIN_FUNCTIONS: [&str; 9] = [
    "print",
    "env",
    "env_int",
    "env_float",
    "env_bool",
    "serve",
    "log",
    "assert",
    "asset",
];
pub(crate) const COMPLETION_BUILTIN_TYPES: [&str; 14] = [
    "Unit", "Int", "Float", "Bool", "String", "Bytes", "Html", "Id", "Email", "Error", "List",
    "Map", "Task", "Range",
];
pub(crate) const STD_ERROR_MODULE_SOURCE: &str = r#"
type Error:
  code: String
  message: String
  status: Int = 500

type ValidationField:
  path: String
  code: String
  message: String

type Validation:
  message: String
  fields: List<ValidationField>

type BadRequest:
  message: String

type Unauthorized:
  message: String

type Forbidden:
  message: String

type NotFound:
  message: String

type Conflict:
  message: String
"#;

pub(crate) fn capabilities_result() -> JsonValue {
    let mut caps = BTreeMap::new();
    caps.insert("textDocumentSync".to_string(), JsonValue::Number(1.0));
    caps.insert("definitionProvider".to_string(), JsonValue::Bool(true));
    caps.insert("hoverProvider".to_string(), JsonValue::Bool(true));
    let mut signature_help = BTreeMap::new();
    signature_help.insert(
        "triggerCharacters".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("(".to_string()),
            JsonValue::String(",".to_string()),
        ]),
    );
    signature_help.insert(
        "retriggerCharacters".to_string(),
        JsonValue::Array(vec![JsonValue::String(",".to_string())]),
    );
    caps.insert(
        "signatureHelpProvider".to_string(),
        JsonValue::Object(signature_help),
    );
    let mut completion_provider = BTreeMap::new();
    completion_provider.insert("resolveProvider".to_string(), JsonValue::Bool(false));
    completion_provider.insert(
        "triggerCharacters".to_string(),
        JsonValue::Array(vec![JsonValue::String(".".to_string())]),
    );
    caps.insert(
        "completionProvider".to_string(),
        JsonValue::Object(completion_provider),
    );
    let mut rename_provider = BTreeMap::new();
    rename_provider.insert("prepareProvider".to_string(), JsonValue::Bool(true));
    caps.insert(
        "renameProvider".to_string(),
        JsonValue::Object(rename_provider),
    );
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
pub(crate) struct LspState {
    pub(crate) docs: BTreeMap<String, String>,
    pub(crate) root_uri: Option<String>,
    pub(crate) cancelled: HashSet<String>,
    pub(crate) docs_revision: u64,
    pub(crate) workspace_cache: Option<WorkspaceCache>,
    pub(crate) workspace_builds: u64,
    /// Set to `true` when `try_incremental_module_update` clears the cache due
    /// to a non-incremental change.  The next diagnostics call consumes this
    /// flag and triggers a full workspace rebuild even for non-entry files.
    pub(crate) workspace_rebuild_pending: bool,
    /// Focus-file snapshot cache: loaded lazily for diagnostics before the full
    /// workspace snapshot is built.  Keyed by (docs_revision, focus_uri).
    pub(crate) progressive_cache: Option<WorkspaceCache>,
    pub(crate) progressive_builds: u64,
}

impl LspState {
    pub(crate) fn invalidate_workspace_cache(&mut self) {
        self.docs_revision = self.docs_revision.saturating_add(1);
        self.workspace_cache = None;
        self.progressive_cache = None;
    }
}

pub(crate) fn handle_cancel(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) {
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

pub(crate) fn cancelled_error(state: &mut LspState, id: Option<&JsonValue>) -> Option<String> {
    let id = id?;
    let key = request_id_key(id)?;
    if state.cancelled.remove(&key) {
        Some("request cancelled".to_string())
    } else {
        None
    }
}

pub(crate) fn request_id_key(id: &JsonValue) -> Option<String> {
    match id {
        JsonValue::Number(num) => Some(format!("{num}")),
        JsonValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(crate) fn apply_doc_overlay_change(state: &mut LspState, uri: &str, text: Option<String>) {
    if let Some(contents) = text.as_ref() {
        state.docs.insert(uri.to_string(), contents.clone());
    } else {
        state.docs.remove(uri);
    }
    state.docs_revision = state.docs_revision.saturating_add(1);
    if !try_incremental_module_update(state, uri, text.as_deref()) {
        state.workspace_cache = None;
        state.workspace_rebuild_pending = true;
    }
}

pub(crate) fn full_document_edit(original: &str, new_text: &str) -> JsonValue {
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

pub(crate) fn range_json(
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> JsonValue {
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

pub(crate) fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            offsets.push(idx + 1);
        }
    }
    offsets
}

pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
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

pub(crate) fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    if raw.contains("://") {
        return raw;
    }
    format!("file://{}", raw)
}

pub(crate) fn decode_uri_component(value: &str) -> String {
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

pub(crate) fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn offset_to_line_col(offsets: &[usize], offset: usize) -> (usize, usize) {
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

pub(crate) fn line_col_to_offset(text: &str, offsets: &[usize], line: usize, col: usize) -> usize {
    if offsets.is_empty() {
        return 0;
    }
    let line = line.min(offsets.len() - 1);
    let start = offsets[line];
    let end = offsets.get(line + 1).copied().unwrap_or_else(|| text.len());
    let offset = start.saturating_add(col);
    offset.min(end)
}

pub(crate) fn json_response(id: Option<JsonValue>, result: JsonValue) -> JsonValue {
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

pub(crate) fn json_error_response(id: Option<JsonValue>, code: i64, message: &str) -> JsonValue {
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

pub(crate) fn json_notification(method: &str, params: JsonValue) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    root.insert("method".to_string(), JsonValue::String(method.to_string()));
    root.insert("params".to_string(), params);
    JsonValue::Object(root)
}

pub(crate) fn get_string(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(JsonValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn extract_text_doc_uri(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
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

pub(crate) fn extract_text_doc_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
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

pub(crate) fn extract_change_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
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

pub(crate) fn extract_position(
    obj: &BTreeMap<String, JsonValue>,
) -> Option<(String, usize, usize)> {
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

pub(crate) fn extract_include_declaration(obj: &BTreeMap<String, JsonValue>) -> bool {
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

pub(crate) fn extract_root_uri(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
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

pub(crate) fn lsp_range_to_span(range: &JsonValue, text: &str, offsets: &[usize]) -> Option<Span> {
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

pub(crate) fn read_message(reader: &mut impl Read) -> io::Result<Option<String>> {
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

pub(crate) fn write_message(out: &mut impl Write, value: &JsonValue) -> io::Result<()> {
    let body = json::encode(value);
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

pub(crate) fn location_json(uri: &str, text: &str, span: Span) -> JsonValue {
    let range = span_range_json(text, span);
    let mut out = BTreeMap::new();
    out.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    out.insert("range".to_string(), range);
    JsonValue::Object(out)
}

pub(crate) fn span_range_json(text: &str, span: Span) -> JsonValue {
    let offsets = line_offsets(text);
    let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
    let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
    range_json(start_line, start_col, end_line, end_col)
}

pub(crate) fn symbol_info_json(uri: &str, text: &str, def: &SymbolDef) -> JsonValue {
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

pub(crate) fn is_valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn is_keyword_or_literal(name: &str) -> bool {
    COMPLETION_KEYWORDS.contains(&name) || matches!(name, "true" | "false" | "null")
}

pub(crate) fn is_renamable_symbol_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Module
            | SymbolKind::Type
            | SymbolKind::Enum
            | SymbolKind::EnumVariant
            | SymbolKind::Function
            | SymbolKind::Config
            | SymbolKind::Param
            | SymbolKind::Variable
            | SymbolKind::Field
    )
}
