use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};

use fusec::ast::{
    Block, ConfigDecl, Doc, EnumDecl, Expr, ExprKind, FnDecl, Ident, ImportDecl, ImportSpec, Item,
    Pattern, PatternKind, Program, ServiceDecl, Stmt, StmtKind, TypeDecl, TypeDerive, TypeRef,
    TypeRefKind,
};
use fusec::diag::{Diag, Level};
use fusec::parse_source;
use fusec::sema;
use fusec::span::Span;
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
            Some("textDocument/definition") => {
                let result = handle_definition(&docs, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/hover") => {
                let result = handle_hover(&docs, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/rename") => {
                let result = handle_rename(&docs, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("workspace/symbol") => {
                let result = handle_workspace_symbol(&docs, &obj);
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
    caps.insert("workspaceSymbolProvider".to_string(), JsonValue::Bool(true));
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

fn line_col_to_offset(text: &str, offsets: &[usize], line: usize, col: usize) -> usize {
    if offsets.is_empty() {
        return 0;
    }
    let line = line.min(offsets.len() - 1);
    let start = offsets[line];
    let end = offsets
        .get(line + 1)
        .copied()
        .unwrap_or_else(|| text.len());
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

fn extract_position(obj: &BTreeMap<String, JsonValue>) -> Option<(String, usize, usize)> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else { return None };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else { return None };
    let uri = match text_doc.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    let position = params.get("position")?;
    let JsonValue::Object(position) = position else { return None };
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
    let JsonValue::Object(params) = params else { return None };
    match params.get("newName") {
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn extract_workspace_query(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else { return None };
    match params.get("query") {
        Some(JsonValue::String(query)) => Some(query.clone()),
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

fn handle_definition(docs: &BTreeMap<String, String>, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = docs.get(&uri) else { return JsonValue::Null };
    let offsets = line_offsets(text);
    let offset = line_col_to_offset(text, &offsets, line, character);
    let index = build_index(text);
    let def_id = match index.definition_at(offset) {
        Some(def_id) => def_id,
        None => return JsonValue::Null,
    };
    let def = &index.defs[def_id];
    let location = location_json(&uri, text, def.span);
    JsonValue::Array(vec![location])
}

fn handle_hover(docs: &BTreeMap<String, String>, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = docs.get(&uri) else { return JsonValue::Null };
    let offsets = line_offsets(text);
    let offset = line_col_to_offset(text, &offsets, line, character);
    let index = build_index(text);
    let def_id = match index.definition_at(offset) {
        Some(def_id) => def_id,
        None => return JsonValue::Null,
    };
    let def = &index.defs[def_id];
    let mut value = def.detail.clone();
    if let Some(doc) = &def.doc {
        if !doc.trim().is_empty() {
            value.push_str("\n\n");
            value.push_str(doc.trim());
        }
    }
    let mut contents = BTreeMap::new();
    contents.insert("kind".to_string(), JsonValue::String("plaintext".to_string()));
    contents.insert("value".to_string(), JsonValue::String(value));
    let mut out = BTreeMap::new();
    out.insert("contents".to_string(), JsonValue::Object(contents));
    JsonValue::Object(out)
}

fn handle_workspace_symbol(
    docs: &BTreeMap<String, String>,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let query = extract_workspace_query(obj).unwrap_or_default().to_lowercase();
    let mut symbols = Vec::new();
    for (uri, text) in docs {
        let index = build_index(text);
        for def in &index.defs {
            if !query.is_empty() && !def.name.to_lowercase().contains(&query) {
                continue;
            }
            let symbol = symbol_info_json(uri, text, def);
            symbols.push(symbol);
        }
    }
    JsonValue::Array(symbols)
}

fn handle_rename(docs: &BTreeMap<String, String>, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(new_name) = extract_new_name(obj) else {
        return JsonValue::Null;
    };
    if !is_valid_ident(&new_name) {
        return JsonValue::Null;
    }
    let Some(text) = docs.get(&uri) else { return JsonValue::Null };
    let offsets = line_offsets(text);
    let offset = line_col_to_offset(text, &offsets, line, character);
    let index = build_index(text);
    let def_id = match index.definition_at(offset) {
        Some(def_id) => def_id,
        None => return JsonValue::Null,
    };
    let edits = index.rename_edits(text, def_id, &new_name);
    if edits.is_empty() {
        return JsonValue::Null;
    }
    let mut changes = BTreeMap::new();
    changes.insert(uri, JsonValue::Array(edits));
    let mut root = BTreeMap::new();
    root.insert("changes".to_string(), JsonValue::Object(changes));
    JsonValue::Object(root)
}

fn location_json(uri: &str, text: &str, span: Span) -> JsonValue {
    let offsets = line_offsets(text);
    let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
    let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
    let range = range_json(start_line, start_col, end_line, end_col);
    let mut out = BTreeMap::new();
    out.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    out.insert("range".to_string(), range);
    JsonValue::Object(out)
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
        out.insert("containerName".to_string(), JsonValue::String(container.clone()));
    }
    JsonValue::Object(out)
}

fn is_valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

struct Index {
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
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

    fn rename_edits(&self, text: &str, def_id: usize, new_name: &str) -> Vec<JsonValue> {
        let mut spans = Vec::new();
        let def = &self.defs[def_id];
        spans.push(def.span);
        for reference in &self.refs {
            if reference.target == def_id {
                spans.push(reference.span);
            }
        }
        let mut seen = HashSet::new();
        let offsets = line_offsets(text);
        spans
            .into_iter()
            .filter(|span| seen.insert((span.start, span.end)))
            .map(|span| {
                let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
                let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
                let range = range_json(start_line, start_col, end_line, end_col);
                let mut edit = BTreeMap::new();
                edit.insert("range".to_string(), range);
                edit.insert("newText".to_string(), JsonValue::String(new_name.to_string()));
                JsonValue::Object(edit)
            })
            .collect()
    }
}

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

#[derive(Clone, Copy)]
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
}

fn span_contains(span: Span, offset: usize) -> bool {
    offset >= span.start && offset <= span.end
}

fn build_index(text: &str) -> Index {
    let (program, _diags) = parse_source(text);
    let mut builder = IndexBuilder::new(text);
    builder.collect(&program);
    builder.finish()
}

struct IndexBuilder<'a> {
    text: &'a str,
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    scopes: Vec<HashMap<String, usize>>,
    globals: HashMap<String, usize>,
    type_defs: HashMap<String, usize>,
    enum_variants: HashMap<String, usize>,
    enum_variant_ambiguous: HashSet<String>,
    enum_variants_by_enum: HashMap<String, HashMap<String, usize>>,
}

impl<'a> IndexBuilder<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            defs: Vec::new(),
            refs: Vec::new(),
            scopes: Vec::new(),
            globals: HashMap::new(),
            type_defs: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_ambiguous: HashSet::new(),
            enum_variants_by_enum: HashMap::new(),
        }
    }

    fn finish(self) -> Index {
        Index {
            defs: self.defs,
            refs: self.refs,
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
                    self.define_literal_decl(&decl.name, SymbolKind::App, detail, decl.doc.as_ref());
                }
                Item::Migration(decl) => {
                    let detail = format!("migration {}", decl.name);
                    self.define_span_decl(decl.span, decl.name.clone(), SymbolKind::Migration, detail, decl.doc.as_ref());
                }
                Item::Test(decl) => {
                    let detail = format!("test \"{}\"", decl.name.value);
                    self.define_literal_decl(&decl.name, SymbolKind::Test, detail, decl.doc.as_ref());
                }
            }
        }
    }

    fn define_import(&mut self, decl: &ImportDecl) {
        match &decl.spec {
            ImportSpec::Module { name } => {
                self.define_global(name, SymbolKind::Module, format!("module {}", name.name), None, None);
            }
            ImportSpec::ModuleFrom { name, .. } => {
                self.define_global(name, SymbolKind::Module, format!("module {}", name.name), None, None);
            }
            ImportSpec::AliasFrom { alias, .. } => {
                self.define_global(alias, SymbolKind::Module, format!("module {}", alias.name), None, None);
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
                self.enum_variant_ambiguous.insert(variant.name.name.clone());
            } else {
                self.enum_variants
                    .insert(variant.name.name.clone(), def_id);
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
            Item::Fn(decl) => self.visit_fn_decl(decl),
            Item::Config(decl) => self.visit_config_decl(decl),
            Item::Service(decl) => self.visit_service_decl(decl),
            Item::App(decl) => self.visit_block(&decl.body),
            Item::Migration(decl) => self.visit_block(&decl.body),
            Item::Test(decl) => self.visit_block(&decl.body),
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
            let detail = format!("param {}: {}", param.name.name, self.type_ref_text(&param.ty));
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
            let detail = format!("field {}: {}", field.name.name, self.type_ref_text(&field.ty));
            self.define_span_decl(field.name.span, field.name.name.clone(), SymbolKind::Field, detail, None);
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
            if route.body_type.is_some() {
                let detail = "param body".to_string();
                let def_id = self.define_span_decl(
                    route.span,
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

    fn add_type_ref(&mut self, ident: &Ident) {
        if ident.name.contains('.') {
            return;
        }
        if is_builtin_type(&ident.name) {
            return;
        }
        if let Some(def_id) = self.type_defs.get(&ident.name) {
            self.add_ref(ident.span, *def_id);
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
            | "Id"
            | "Email"
            | "Error"
            | "List"
            | "Map"
            | "Option"
            | "Result"
    )
}
