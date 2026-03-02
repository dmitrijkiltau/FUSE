use std::collections::{BTreeMap, HashSet};

use fuse_rt::json::JsonValue;
use fusec::ast::{ConfigDecl, ImportDecl, ImportSpec, Item, Program};
use fusec::diag::Level;
use fusec::parse_source;
use fusec::span::Span;

use super::super::{
    LspState, SymbolKind, WorkspaceDef, WorkspaceIndex, build_workspace_index_cached,
    extract_position, extract_text_doc_uri, is_exported_def_kind, is_keyword_or_literal,
    is_renamable_symbol_kind, is_valid_ident, line_col_to_offset, line_offsets, lsp_range_to_span,
    offset_to_line_col, range_json, span_contains, span_range_json, symbol_info_json, uri_to_path,
};

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

pub(crate) fn handle_workspace_symbol(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let query = extract_workspace_query(obj)
        .unwrap_or_default()
        .to_lowercase();
    let mut symbols = Vec::new();
    let index = match build_workspace_index_cached(state, "") {
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

pub(crate) fn handle_rename(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(new_name) = extract_new_name(obj) else {
        return JsonValue::Null;
    };
    if !is_valid_ident(&new_name) || is_keyword_or_literal(&new_name) {
        return JsonValue::Null;
    }
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    if !is_renamable_symbol_kind(def.def.kind) {
        return JsonValue::Null;
    }
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

pub(crate) fn handle_prepare_rename(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    if !is_renamable_symbol_kind(def.def.kind) {
        return JsonValue::Null;
    }
    let Some(text) = index.file_text(&uri) else {
        return JsonValue::Null;
    };
    let offsets = line_offsets(text);
    let offset = line_col_to_offset(text, &offsets, line, character);
    let Some(span) = rename_span_at_position(index, &uri, offset, def.id) else {
        return JsonValue::Null;
    };

    let mut out = BTreeMap::new();
    out.insert("range".to_string(), span_range_json(text, span));
    out.insert(
        "placeholder".to_string(),
        JsonValue::String(def.def.name.clone()),
    );
    JsonValue::Object(out)
}

fn rename_span_at_position(
    index: &WorkspaceIndex,
    uri: &str,
    offset: usize,
    def_id: usize,
) -> Option<Span> {
    let mut best_ref: Option<(Span, usize)> = None;
    for reference in &index.refs {
        if reference.uri != uri || reference.target != def_id {
            continue;
        }
        if !span_contains(reference.span, offset) {
            continue;
        }
        let size = reference.span.end.saturating_sub(reference.span.start);
        if best_ref.map_or(true, |(_, best_size)| size < best_size) {
            best_ref = Some((reference.span, size));
        }
    }
    if let Some((span, _)) = best_ref {
        return Some(span);
    }
    let def = index.def_for_target(def_id)?;
    if def.uri == uri && span_contains(def.def.span, offset) {
        return Some(def.def.span);
    }
    None
}

pub(crate) fn handle_code_action(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
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
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let (program, _) = parse_source(&text);
    let imports = collect_imports(&program);
    let mut actions = Vec::new();
    let mut seen = HashSet::new();

    for diag in extract_code_action_diagnostics(obj, &text) {
        if let Some(symbol) = parse_unknown_symbol_name(&diag.message) {
            if is_valid_ident(&symbol) {
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

                for module_path in import_candidates_for_symbol(index, &uri, &symbol)
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
        }

        if let Some((field, config)) = parse_unknown_field_on_type(&diag.message) {
            for (title, edit) in missing_config_field_actions(index, &config, &field) {
                let key = format!("quickfix:{title}");
                if seen.insert(key) {
                    actions.push(code_action_json(&title, "quickfix", edit));
                }
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

fn parse_unknown_field_on_type(message: &str) -> Option<(String, String)> {
    let rest = message.strip_prefix("unknown field ")?;
    let (field, ty) = rest.split_once(" on ")?;
    let field = field.trim();
    let ty = ty.trim();
    if !is_valid_ident(field) || !is_valid_ident(ty) {
        return None;
    }
    Some((field.to_string(), ty.to_string()))
}

fn missing_config_field_actions(
    index: &WorkspaceIndex,
    config_name: &str,
    field_name: &str,
) -> Vec<(String, JsonValue)> {
    let mut defs = config_defs_named(index, config_name);
    defs.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then(a.def.span.start.cmp(&b.def.span.start))
    });
    defs.dedup_by(|a, b| a.uri == b.uri && a.def.span.start == b.def.span.start);

    let multiple = defs.len() > 1;
    let mut out = Vec::new();
    for def in defs {
        let Some(text) = index.file_text(&def.uri) else {
            continue;
        };
        let Some(edit) =
            missing_config_field_workspace_edit(&def.uri, text, config_name, field_name)
        else {
            continue;
        };
        let title = if multiple {
            let location = path_display_for_uri(&def.uri);
            format!("Add {config_name}.{field_name} to config in {location}")
        } else {
            format!("Add {config_name}.{field_name} to config")
        };
        out.push((title, edit));
    }
    out
}

fn config_defs_named(index: &WorkspaceIndex, config_name: &str) -> Vec<WorkspaceDef> {
    index
        .defs
        .iter()
        .filter(|def| def.def.kind == SymbolKind::Config && def.def.name == config_name)
        .cloned()
        .collect()
}

fn missing_config_field_workspace_edit(
    uri: &str,
    text: &str,
    config_name: &str,
    field_name: &str,
) -> Option<JsonValue> {
    let (program, parse_diags) = parse_source(text);
    if parse_diags
        .iter()
        .any(|diag| matches!(diag.level, Level::Error))
    {
        return None;
    }
    let config = program.items.iter().find_map(|item| match item {
        Item::Config(decl) if decl.name.name == config_name => Some(decl),
        _ => None,
    })?;
    if config
        .fields
        .iter()
        .any(|field| field.name.name == field_name)
    {
        return None;
    }

    let insert_offset = config_field_insert_offset(text, config);
    let indent = config_field_indent(text, config);
    let new_text = format!("\n{indent}{field_name}: String = \"\"");
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(insert_offset, insert_offset),
        &new_text,
    ))
}

fn config_field_insert_offset(text: &str, config: &ConfigDecl) -> usize {
    if let Some(last_field) = config.fields.last() {
        return line_end_offset(text, last_field.span.end);
    }
    line_end_offset(text, config.name.span.end)
}

fn config_field_indent(text: &str, config: &ConfigDecl) -> String {
    if let Some(first_field) = config.fields.first() {
        return line_indent_at(text, first_field.span.start);
    }
    let base = line_indent_at(text, config.span.start);
    format!("{base}  ")
}

fn line_indent_at(text: &str, offset: usize) -> String {
    let line_start = line_start_offset(text, offset);
    let mut idx = line_start;
    let bytes = text.as_bytes();
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    text[line_start..idx].to_string()
}

fn line_start_offset(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[..offset].rfind('\n').map_or(0, |idx| idx + 1)
}

fn line_end_offset(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    match text[offset..].find('\n') {
        Some(rel) => offset + rel,
        None => text.len(),
    }
}

fn path_display_for_uri(uri: &str) -> String {
    uri_to_path(uri)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| uri.to_string())
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
    while end < text.len() && text.as_bytes().get(end) == Some(&b'\n') {
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
