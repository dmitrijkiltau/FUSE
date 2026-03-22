use std::collections::{BTreeMap, HashSet};

use fuse_rt::json::JsonValue;
use fusec::ast::{
    Block, CallArg, Capability, ConfigDecl, Expr, ExprKind, ImportDecl, ImportSpec, Item, Literal,
    Program, Stmt, StmtKind, TypeRef, TypeRefKind,
};
use fusec::diag::Level;
use fusec::frontend::html_shorthand::{HTML_ATTR_COMMA_DIAG_CODE, HTML_ATTR_MAP_DIAG_CODE};
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
    code: Option<String>,
    message: String,
    span: Option<Span>,
}

#[derive(Clone, Copy)]
struct WorkspaceSymbolMatch {
    tier: u8,
    start: usize,
    span: usize,
    gaps: usize,
}

struct WorkspaceSymbolCandidate {
    score: WorkspaceSymbolMatch,
    kind_tier: u8,
    name: String,
    name_lower: String,
    uri: String,
    span_start: usize,
    symbol: JsonValue,
}

fn symbol_kind_nav_tier(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Function
        | SymbolKind::Service
        | SymbolKind::App
        | SymbolKind::Migration
        | SymbolKind::Test => 0,
        SymbolKind::Type | SymbolKind::Interface | SymbolKind::Enum | SymbolKind::Config => 1,
        SymbolKind::EnumVariant | SymbolKind::Field | SymbolKind::Module => 2,
        SymbolKind::Param | SymbolKind::Variable => 3,
    }
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
        let code = match diag_obj.get("code") {
            Some(JsonValue::String(value)) => Some(value.clone()),
            _ => None,
        };
        let span = diag_obj
            .get("range")
            .and_then(|range| lsp_range_to_span(range, text, &offsets));
        out.push(CodeActionDiag {
            code,
            message,
            span,
        });
    }
    out
}

/// Parses an optional kind-filter prefix from the query string.
/// Supported prefixes: `fn:`, `type:`, `enum:`, `config:`, `service:`.
/// Returns `(kind_filter, remainder_query)`.
fn parse_query_kind_filter(query: &str) -> (Option<SymbolKind>, &str) {
    for (prefix, kind) in [
        ("fn:", SymbolKind::Function),
        ("type:", SymbolKind::Type),
        ("enum:", SymbolKind::Enum),
        ("config:", SymbolKind::Config),
        ("service:", SymbolKind::Service),
    ] {
        if let Some(rest) = query.strip_prefix(prefix) {
            return (Some(kind), rest);
        }
    }
    (None, query)
}

fn kind_matches_filter(kind: SymbolKind, filter: SymbolKind) -> bool {
    match filter {
        SymbolKind::Function => matches!(
            kind,
            SymbolKind::Function | SymbolKind::Migration | SymbolKind::Test
        ),
        SymbolKind::Type => matches!(kind, SymbolKind::Type),
        SymbolKind::Enum => matches!(kind, SymbolKind::Enum | SymbolKind::EnumVariant),
        SymbolKind::Config => matches!(kind, SymbolKind::Config),
        SymbolKind::Service => matches!(kind, SymbolKind::Service | SymbolKind::App),
        _ => kind == filter,
    }
}

pub(crate) fn handle_workspace_symbol(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let raw_query = extract_workspace_query(obj).unwrap_or_default();
    let (kind_filter, query_str) = parse_query_kind_filter(&raw_query);
    let query = query_str.to_string();
    let mut symbols = Vec::new();
    let index = match build_workspace_index_cached(state, "") {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let mut candidates = Vec::new();
    for def in &index.defs {
        if matches!(def.def.kind, SymbolKind::Param | SymbolKind::Variable) {
            continue;
        }
        if let Some(filter) = kind_filter {
            if !kind_matches_filter(def.def.kind, filter) {
                continue;
            }
        }
        let Some(score) = workspace_symbol_match_score(&def.def.name, &query) else {
            continue;
        };
        let Some(file_idx) = index.file_by_uri.get(&def.uri) else {
            continue;
        };
        let file = &index.files[*file_idx];
        let symbol = symbol_info_json(&def.uri, &file.text, &def.def);
        candidates.push(WorkspaceSymbolCandidate {
            score,
            kind_tier: symbol_kind_nav_tier(def.def.kind),
            name: def.def.name.clone(),
            name_lower: def.def.name.to_lowercase(),
            uri: def.uri.clone(),
            span_start: def.def.span.start,
            symbol,
        });
    }
    candidates.sort_by(|left, right| {
        left.score
            .tier
            .cmp(&right.score.tier)
            .then_with(|| left.kind_tier.cmp(&right.kind_tier))
            .then_with(|| left.score.start.cmp(&right.score.start))
            .then_with(|| left.score.gaps.cmp(&right.score.gaps))
            .then_with(|| left.score.span.cmp(&right.score.span))
            .then_with(|| left.name.len().cmp(&right.name.len()))
            .then_with(|| left.name_lower.cmp(&right.name_lower))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.uri.cmp(&right.uri))
            .then_with(|| left.span_start.cmp(&right.span_start))
    });
    let cap = if query.is_empty() { 50 } else { 128 };
    candidates.truncate(cap);
    for candidate in candidates {
        symbols.push(candidate.symbol);
    }
    JsonValue::Array(symbols)
}

fn workspace_symbol_match_score(name: &str, query: &str) -> Option<WorkspaceSymbolMatch> {
    if query.is_empty() {
        return Some(WorkspaceSymbolMatch {
            tier: 9,
            start: 0,
            span: 0,
            gaps: 0,
        });
    }
    if name == query {
        return Some(WorkspaceSymbolMatch {
            tier: 0,
            start: 0,
            span: query.chars().count(),
            gaps: 0,
        });
    }

    let name_lower = name.to_lowercase();
    let query_lower = query.to_lowercase();

    if name_lower == query_lower {
        return Some(WorkspaceSymbolMatch {
            tier: 1,
            start: 0,
            span: query_lower.chars().count(),
            gaps: 0,
        });
    }
    if name.starts_with(query) {
        return Some(WorkspaceSymbolMatch {
            tier: 2,
            start: 0,
            span: query.chars().count(),
            gaps: 0,
        });
    }
    if name_lower.starts_with(&query_lower) {
        return Some(WorkspaceSymbolMatch {
            tier: 3,
            start: 0,
            span: query_lower.chars().count(),
            gaps: 0,
        });
    }

    let initials = workspace_symbol_initials(name);
    if initials.starts_with(&query_lower) {
        return Some(WorkspaceSymbolMatch {
            tier: 4,
            start: 0,
            span: query_lower.chars().count(),
            gaps: 0,
        });
    }

    if let Some(start) = name_lower.find(&query_lower) {
        return Some(WorkspaceSymbolMatch {
            tier: 5,
            start,
            span: query_lower.len(),
            gaps: 0,
        });
    }

    let (start, end, gaps) = workspace_symbol_subsequence_match(&name_lower, &query_lower)?;
    Some(WorkspaceSymbolMatch {
        tier: 6,
        start,
        span: end.saturating_sub(start),
        gaps,
    })
}

fn workspace_symbol_initials(name: &str) -> String {
    let mut out = String::new();
    let mut prev_is_alnum = false;
    let mut prev_was_lower = false;
    for (idx, ch) in name.chars().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            prev_is_alnum = false;
            prev_was_lower = false;
            continue;
        }
        let boundary = idx == 0 || !prev_is_alnum || (ch.is_ascii_uppercase() && prev_was_lower);
        if boundary {
            out.push(ch.to_ascii_lowercase());
        }
        prev_is_alnum = true;
        prev_was_lower = ch.is_ascii_lowercase();
    }
    out
}

/// Returns `(start, end, gaps)` for the shortest-span subsequence match of `query` in `name`.
/// For each candidate start position in `name` that begins with the first query character,
/// attempts a greedy forward match from that position and tracks the (span, gaps) score.
/// Picks the match with minimum span, breaking ties by fewest gaps.
fn workspace_symbol_subsequence_match(name: &str, query: &str) -> Option<(usize, usize, usize)> {
    let name_chars: Vec<char> = name.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    if query_chars.is_empty() {
        return None;
    }
    let first = query_chars[0];
    let mut best: Option<(usize, usize, usize)> = None; // (start, end, gaps)

    for start_pos in 0..name_chars.len() {
        if name_chars[start_pos] != first {
            continue;
        }
        // Greedy forward match from start_pos
        let mut qi = 0usize;
        let mut last_match = start_pos;
        let mut gaps = 0usize;
        let mut ni = start_pos;
        while ni < name_chars.len() && qi < query_chars.len() {
            if name_chars[ni] == query_chars[qi] {
                if qi > 0 {
                    gaps += ni - last_match - 1;
                }
                last_match = ni;
                qi += 1;
            }
            ni += 1;
        }
        if qi < query_chars.len() {
            continue; // didn't match full query from this start
        }
        let end = last_match + 1;
        let span = end - start_pos;
        let is_better = best.map_or(true, |(best_start, best_end, best_gaps)| {
            let best_span = best_end - best_start;
            span < best_span || (span == best_span && gaps < best_gaps)
        });
        if is_better {
            best = Some((start_pos, end, gaps));
        }
    }
    best
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
        if diag.code.as_deref() == Some(HTML_ATTR_MAP_DIAG_CODE) {
            if let Some(span) = diag.span {
                if let Some(edit) = html_attr_map_workspace_edit(&uri, &text, &program, span) {
                    let title = "Rewrite HTML attrs from map literal";
                    let key = format!("quickfix:{title}:{}:{}", span.start, span.end);
                    if seen.insert(key) {
                        actions.push(code_action_json(title, "quickfix", edit));
                    }
                }
            }
        }

        if diag.code.as_deref() == Some(HTML_ATTR_COMMA_DIAG_CODE) {
            if let Some(span) = diag.span {
                if let Some(edit) = html_attr_comma_workspace_edit(&uri, &text, span) {
                    let title = "Remove comma between HTML attrs";
                    let key = format!("quickfix:{title}:{}:{}", span.start, span.end);
                    if seen.insert(key) {
                        actions.push(code_action_json(title, "quickfix", edit));
                    }
                }
            }
        }

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

        if let Some(capability) = parse_missing_capability_diag(&diag.message) {
            let Some(edit) =
                missing_capability_requires_workspace_edit(&uri, &text, &program, capability)
            else {
                continue;
            };
            let title = format!("Add requires {}", capability.as_str());
            let key = format!("quickfix:{title}");
            if seen.insert(key) {
                actions.push(code_action_json(&title, "quickfix", edit));
            }
        }

        if diag.code.as_deref() == Some("FUSE_WRONG_ARITY") {
            if let Some(span) = diag.span {
                if let Some((expected, got)) = parse_wrong_arity_diag(&diag.message) {
                    if got > expected {
                        if let Some(edit) =
                            wrong_arity_trim_workspace_edit(&uri, &text, &program, span, expected)
                        {
                            let surplus = got - expected;
                            let title = format!(
                                "Remove {} surplus argument{}",
                                surplus,
                                if surplus == 1 { "" } else { "s" }
                            );
                            let key = format!("quickfix:{title}:{}:{}", span.start, span.end);
                            if seen.insert(key) {
                                actions.push(code_action_json(&title, "quickfix", edit));
                            }
                        }
                    }
                }
            }
        }

        if diag.code.as_deref() == Some("FUSE_DETACHED_TASK") {
            if let Some(span) = diag.span {
                if let Some(edit) = detached_task_wrap_workspace_edit(&uri, &text, span) {
                    let title = "Await detached task (let _task = …; await _task)";
                    let key = format!("quickfix:{title}:{}:{}", span.start, span.end);
                    if seen.insert(key) {
                        actions.push(code_action_json(title, "quickfix", edit));
                    }
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

fn parse_missing_capability_diag(message: &str) -> Option<Capability> {
    if !message.contains("module top-level") {
        return None;
    }
    for marker in [
        "requires capability ",
        "require capability ",
        "leaks capability ",
    ] {
        let Some((_, rest)) = message.split_once(marker) else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphabetic())
            .collect();
        if let Some(capability) = Capability::from_name(&name) {
            return Some(capability);
        }
    }
    None
}

/// Parses `"expected N arguments, got M"` → `(expected, got)`.
fn parse_wrong_arity_diag(message: &str) -> Option<(usize, usize)> {
    let rest = message.strip_prefix("expected ")?;
    let (expected_str, rest) = rest.split_once(" arguments, got ")?;
    let expected: usize = expected_str.parse().ok()?;
    let got: usize = rest.parse().ok()?;
    Some((expected, got))
}

/// Walks the program AST and returns the `args` slice of the first `Call` expression
/// whose span exactly matches `target_span`.
fn find_call_args_at_span<'a>(program: &'a Program, target_span: Span) -> Option<&'a [CallArg]> {
    for item in &program.items {
        if let Some(args) = find_call_args_in_item(item, target_span) {
            return Some(args);
        }
    }
    None
}

fn find_call_args_in_expr<'a>(expr: &'a Expr, target: Span) -> Option<&'a [CallArg]> {
    if expr.span == target {
        if let ExprKind::Call { args, .. } = &expr.kind {
            return Some(args);
        }
    }
    match &expr.kind {
        ExprKind::Call { callee, args, .. } => {
            if let Some(found) = find_call_args_in_expr(callee, target) {
                return Some(found);
            }
            for arg in args {
                if let Some(found) = find_call_args_in_expr(&arg.value, target) {
                    return Some(found);
                }
            }
        }
        ExprKind::Binary { left, right, .. } => {
            if let Some(f) = find_call_args_in_expr(left, target) {
                return Some(f);
            }
            return find_call_args_in_expr(right, target);
        }
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::Await { expr: inner }
        | ExprKind::Box { expr: inner } => return find_call_args_in_expr(inner, target),
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            return find_call_args_in_expr(base, target)
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            if let Some(f) = find_call_args_in_expr(base, target) {
                return Some(f);
            }
            return find_call_args_in_expr(index, target);
        }
        ExprKind::StructLit { fields, .. } => {
            for f in fields {
                if let Some(found) = find_call_args_in_expr(&f.value, target) {
                    return Some(found);
                }
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                if let Some(found) = find_call_args_in_expr(item, target) {
                    return Some(found);
                }
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                if let Some(f) = find_call_args_in_expr(k, target).or_else(|| find_call_args_in_expr(v, target)) {
                    return Some(f);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            if let Some(f) = find_call_args_in_expr(left, target) {
                return Some(f);
            }
            return find_call_args_in_expr(right, target);
        }
        ExprKind::BangChain { expr: inner, error } => {
            if let Some(f) = find_call_args_in_expr(inner, target) {
                return Some(f);
            }
            if let Some(err) = error {
                return find_call_args_in_expr(err, target);
            }
        }
        ExprKind::Spawn { block } => return find_call_args_in_block(block, target),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            if let Some(f) = find_call_args_in_expr(cond, target) {
                return Some(f);
            }
            for child in then_children.iter().chain(else_children.iter()) {
                if let Some(f) = find_call_args_in_expr(child, target) {
                    return Some(f);
                }
            }
            for (branch_cond, children) in else_if {
                if let Some(f) = find_call_args_in_expr(branch_cond, target) {
                    return Some(f);
                }
                for child in children {
                    if let Some(f) = find_call_args_in_expr(child, target) {
                        return Some(f);
                    }
                }
            }
        }
        ExprKind::HtmlFor {
            iter,
            body_children,
            ..
        } => {
            if let Some(f) = find_call_args_in_expr(iter, target) {
                return Some(f);
            }
            for child in body_children {
                if let Some(f) = find_call_args_in_expr(child, target) {
                    return Some(f);
                }
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(inner) = part {
                    if let Some(f) = find_call_args_in_expr(inner, target) {
                        return Some(f);
                    }
                }
            }
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
    None
}

fn find_call_args_in_block<'a>(block: &'a Block, target: Span) -> Option<&'a [CallArg]> {
    for stmt in &block.stmts {
        if let Some(f) = find_call_args_in_stmt(stmt, target) {
            return Some(f);
        }
    }
    None
}

fn find_call_args_in_stmt<'a>(stmt: &'a Stmt, target: Span) -> Option<&'a [CallArg]> {
    match &stmt.kind {
        StmtKind::Let { expr, .. } | StmtKind::Var { expr, .. } | StmtKind::Expr(expr) => {
            find_call_args_in_expr(expr, target)
        }
        StmtKind::Return { expr: Some(expr) } => find_call_args_in_expr(expr, target),
        StmtKind::Return { expr: None } => None,
        StmtKind::While { cond, block } => find_call_args_in_expr(cond, target)
            .or_else(|| find_call_args_in_block(block, target)),
        StmtKind::For { iter, block, .. } => find_call_args_in_expr(iter, target)
            .or_else(|| find_call_args_in_block(block, target)),
        StmtKind::If { cond, then_block, else_if, else_block } => {
            if let Some(f) = find_call_args_in_expr(cond, target) {
                return Some(f);
            }
            if let Some(f) = find_call_args_in_block(then_block, target) {
                return Some(f);
            }
            for (elif_cond, elif_block) in else_if {
                if let Some(f) = find_call_args_in_expr(elif_cond, target)
                    .or_else(|| find_call_args_in_block(elif_block, target))
                {
                    return Some(f);
                }
            }
            if let Some(else_b) = else_block {
                return find_call_args_in_block(else_b, target);
            }
            None
        }
        StmtKind::Match { expr, cases } => {
            if let Some(f) = find_call_args_in_expr(expr, target) {
                return Some(f);
            }
            for (_, block) in cases {
                if let Some(f) = find_call_args_in_block(block, target) {
                    return Some(f);
                }
            }
            None
        }
        StmtKind::Transaction { block } => find_call_args_in_block(block, target),
        StmtKind::Assign { target: tgt, expr } => find_call_args_in_expr(expr, target)
            .or_else(|| find_call_args_in_expr(tgt, target)),
        StmtKind::Break | StmtKind::Continue => None,
    }
}

fn find_call_args_in_item<'a>(item: &'a Item, target: Span) -> Option<&'a [CallArg]> {
    match item {
        Item::Fn(decl) => find_call_args_in_block(&decl.body, target),
        Item::Service(decl) => {
            for route in &decl.routes {
                if let Some(f) = find_call_args_in_block(&route.body, target) {
                    return Some(f);
                }
            }
            None
        }
        Item::Component(decl) => find_call_args_in_block(&decl.body, target),
        Item::App(decl) => find_call_args_in_block(&decl.body, target),
        Item::Migration(decl) => find_call_args_in_block(&decl.body, target),
        Item::Test(decl) => find_call_args_in_block(&decl.body, target),
        Item::Config(decl) => {
            for field in &decl.fields {
                if let Some(f) = find_call_args_in_expr(&field.value, target) {
                    return Some(f);
                }
            }
            None
        }
        Item::Type(decl) => {
            for field in &decl.fields {
                if let Some(default) = &field.default {
                    if let Some(f) = find_call_args_in_expr(default, target) {
                        return Some(f);
                    }
                }
            }
            None
        }
        Item::Import(_) | Item::Enum(_) | Item::Interface(_) | Item::Impl(_) => None,
    }
}

/// Generates a workspace edit that removes surplus arguments from a call expression.
/// `diag_span` is the span of the Call expression; `expected` is the correct arg count.
fn wrong_arity_trim_workspace_edit(
    uri: &str,
    text: &str,
    program: &Program,
    diag_span: Span,
    expected: usize,
) -> Option<JsonValue> {
    let args = find_call_args_at_span(program, diag_span)?;
    if args.len() <= expected {
        return None;
    }
    // Remove from args[expected-1].span.end to args[last].span.end,
    // which removes the ", surplus_arg1, surplus_arg2, ..." text.
    // When expected == 0, remove args[0].span.start to args[last].span.end.
    let remove_start = if expected == 0 {
        args[0].span.start
    } else {
        args[expected - 1].span.end
    };
    let remove_end = args[args.len() - 1].span.end;
    let remove_span = Span::new(remove_start, remove_end);
    Some(workspace_edit_with_single_span(uri, text, remove_span, ""))
}

/// Generates a workspace edit that wraps a bare `spawn { ... }` expression in
/// `let _task = spawn { ... }; await _task`, turning a detached task into an awaited one.
fn detached_task_wrap_workspace_edit(uri: &str, text: &str, span: Span) -> Option<JsonValue> {
    let expr_text = text.get(span.start..span.end)?;
    let indent = line_indent_at(text, span.start);
    let new_text = format!("let _task = {expr_text};\n{indent}await _task");
    Some(workspace_edit_with_single_span(uri, text, span, &new_text))
}

fn html_attr_comma_workspace_edit(uri: &str, text: &str, span: Span) -> Option<JsonValue> {
    let snippet = text.get(span.start..span.end)?;
    if !snippet.contains(',') {
        return None;
    }
    Some(workspace_edit_with_single_span(uri, text, span, ""))
}

fn html_attr_map_workspace_edit(
    uri: &str,
    text: &str,
    program: &Program,
    span: Span,
) -> Option<JsonValue> {
    let pairs = map_literal_attr_pairs_for_span(program, span)?;
    let mut rendered = Vec::new();
    for (attr_name, value_span) in pairs {
        let value = text.get(value_span.start..value_span.end)?;
        rendered.push(format!("{attr_name}={value}"));
    }
    let replacement = rendered.join(" ");
    Some(workspace_edit_with_single_span(
        uri,
        text,
        span,
        &replacement,
    ))
}

fn map_literal_attr_pairs_for_span(program: &Program, span: Span) -> Option<Vec<(String, Span)>> {
    for item in &program.items {
        if let Some(pairs) = map_literal_attr_pairs_in_item(item, span) {
            return Some(pairs);
        }
    }
    None
}

fn map_literal_attr_pairs_in_item(item: &Item, span: Span) -> Option<Vec<(String, Span)>> {
    match item {
        Item::Import(_) => None,
        Item::Type(decl) => {
            for field in &decl.fields {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(&field.ty, span) {
                    return Some(pairs);
                }
                if let Some(default) = &field.default {
                    if let Some(pairs) = map_literal_attr_pairs_in_expr(default, span) {
                        return Some(pairs);
                    }
                }
            }
            None
        }
        Item::Enum(decl) => {
            for variant in &decl.variants {
                for payload in &variant.payload {
                    if let Some(pairs) = map_literal_attr_pairs_in_type_ref(payload, span) {
                        return Some(pairs);
                    }
                }
            }
            None
        }
        Item::Fn(decl) => {
            for param in &decl.params {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(&param.ty, span) {
                    return Some(pairs);
                }
                if let Some(default) = &param.default {
                    if let Some(pairs) = map_literal_attr_pairs_in_expr(default, span) {
                        return Some(pairs);
                    }
                }
            }
            if let Some(ret) = &decl.ret {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(ret, span) {
                    return Some(pairs);
                }
            }
            map_literal_attr_pairs_in_block(&decl.body, span)
        }
        Item::Service(decl) => {
            for route in &decl.routes {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(&route.ret_type, span) {
                    return Some(pairs);
                }
                if let Some(body_type) = &route.body_type {
                    if let Some(pairs) = map_literal_attr_pairs_in_type_ref(body_type, span) {
                        return Some(pairs);
                    }
                }
                if let Some(pairs) = map_literal_attr_pairs_in_block(&route.body, span) {
                    return Some(pairs);
                }
            }
            None
        }
        Item::Config(decl) => {
            for field in &decl.fields {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(&field.ty, span) {
                    return Some(pairs);
                }
                if let Some(pairs) = map_literal_attr_pairs_in_expr(&field.value, span) {
                    return Some(pairs);
                }
            }
            None
        }
        Item::Component(decl) => map_literal_attr_pairs_in_block(&decl.body, span),
        Item::App(decl) => map_literal_attr_pairs_in_block(&decl.body, span),
        Item::Migration(decl) => map_literal_attr_pairs_in_block(&decl.body, span),
        Item::Test(decl) => map_literal_attr_pairs_in_block(&decl.body, span),
        Item::Interface(_) | Item::Impl(_) => None,
    }
}

fn map_literal_attr_pairs_in_block(block: &Block, span: Span) -> Option<Vec<(String, Span)>> {
    if !span_contains(block.span, span.start) {
        return None;
    }
    for stmt in &block.stmts {
        if let Some(pairs) = map_literal_attr_pairs_in_stmt(stmt, span) {
            return Some(pairs);
        }
    }
    None
}

fn map_literal_attr_pairs_in_stmt(stmt: &Stmt, span: Span) -> Option<Vec<(String, Span)>> {
    if !span_contains(stmt.span, span.start) {
        return None;
    }
    match &stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(ty) = ty {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(ty, span) {
                    return Some(pairs);
                }
            }
            map_literal_attr_pairs_in_expr(expr, span)
        }
        StmtKind::Assign { target, expr } => map_literal_attr_pairs_in_expr(target, span)
            .or_else(|| map_literal_attr_pairs_in_expr(expr, span)),
        StmtKind::Return { expr } => expr
            .as_ref()
            .and_then(|expr| map_literal_attr_pairs_in_expr(expr, span)),
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            if let Some(pairs) = map_literal_attr_pairs_in_expr(cond, span) {
                return Some(pairs);
            }
            if let Some(pairs) = map_literal_attr_pairs_in_block(then_block, span) {
                return Some(pairs);
            }
            for (branch_cond, branch_block) in else_if {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(branch_cond, span) {
                    return Some(pairs);
                }
                if let Some(pairs) = map_literal_attr_pairs_in_block(branch_block, span) {
                    return Some(pairs);
                }
            }
            else_block
                .as_ref()
                .and_then(|block| map_literal_attr_pairs_in_block(block, span))
        }
        StmtKind::Match { expr, cases } => {
            if let Some(pairs) = map_literal_attr_pairs_in_expr(expr, span) {
                return Some(pairs);
            }
            for (_, block) in cases {
                if let Some(pairs) = map_literal_attr_pairs_in_block(block, span) {
                    return Some(pairs);
                }
            }
            None
        }
        StmtKind::For { iter, block, .. } => map_literal_attr_pairs_in_expr(iter, span)
            .or_else(|| map_literal_attr_pairs_in_block(block, span)),
        StmtKind::While { cond, block } => map_literal_attr_pairs_in_expr(cond, span)
            .or_else(|| map_literal_attr_pairs_in_block(block, span)),
        StmtKind::Transaction { block } => map_literal_attr_pairs_in_block(block, span),
        StmtKind::Expr(expr) => map_literal_attr_pairs_in_expr(expr, span),
        StmtKind::Break | StmtKind::Continue => None,
    }
}

fn map_literal_attr_pairs_in_expr(expr: &Expr, span: Span) -> Option<Vec<(String, Span)>> {
    if expr.span.start == span.start && expr.span.end == span.end {
        if let ExprKind::MapLit(items) = &expr.kind {
            return map_literal_attr_pairs_from_items(items);
        }
    }
    if !span_contains(expr.span, span.start) {
        return None;
    }
    match &expr.kind {
        ExprKind::Call { callee, args, .. } => {
            if let Some(pairs) = map_literal_attr_pairs_in_expr(callee, span) {
                return Some(pairs);
            }
            for arg in args {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(&arg.value, span) {
                    return Some(pairs);
                }
            }
            None
        }
        ExprKind::Binary { left, right, .. } => map_literal_attr_pairs_in_expr(left, span)
            .or_else(|| map_literal_attr_pairs_in_expr(right, span)),
        ExprKind::Unary { expr, .. } | ExprKind::Await { expr } | ExprKind::Box { expr } => {
            map_literal_attr_pairs_in_expr(expr, span)
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            map_literal_attr_pairs_in_expr(base, span)
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            map_literal_attr_pairs_in_expr(base, span)
                .or_else(|| map_literal_attr_pairs_in_expr(index, span))
        }
        ExprKind::StructLit { fields, .. } => {
            for field in fields {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(&field.value, span) {
                    return Some(pairs);
                }
            }
            None
        }
        ExprKind::ListLit(items) => {
            for item in items {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(item, span) {
                    return Some(pairs);
                }
            }
            None
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(key, span) {
                    return Some(pairs);
                }
                if let Some(pairs) = map_literal_attr_pairs_in_expr(value, span) {
                    return Some(pairs);
                }
            }
            None
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(part_expr) = part {
                    if let Some(pairs) = map_literal_attr_pairs_in_expr(part_expr, span) {
                        return Some(pairs);
                    }
                }
            }
            None
        }
        ExprKind::Coalesce { left, right } => map_literal_attr_pairs_in_expr(left, span)
            .or_else(|| map_literal_attr_pairs_in_expr(right, span)),
        ExprKind::BangChain { expr, error } => {
            map_literal_attr_pairs_in_expr(expr, span).or_else(|| {
                error
                    .as_ref()
                    .and_then(|error| map_literal_attr_pairs_in_expr(error, span))
            })
        }
        ExprKind::Spawn { block } => map_literal_attr_pairs_in_block(block, span),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            if let Some(pairs) = map_literal_attr_pairs_in_expr(cond, span) {
                return Some(pairs);
            }
            for child in then_children {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(child, span) {
                    return Some(pairs);
                }
            }
            for (branch_cond, branch_children) in else_if {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(branch_cond, span) {
                    return Some(pairs);
                }
                for child in branch_children {
                    if let Some(pairs) = map_literal_attr_pairs_in_expr(child, span) {
                        return Some(pairs);
                    }
                }
            }
            for child in else_children {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(child, span) {
                    return Some(pairs);
                }
            }
            None
        }
        ExprKind::HtmlFor {
            iter,
            body_children,
            ..
        } => {
            if let Some(pairs) = map_literal_attr_pairs_in_expr(iter, span) {
                return Some(pairs);
            }
            for child in body_children {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(child, span) {
                    return Some(pairs);
                }
            }
            None
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => None,
    }
}

fn map_literal_attr_pairs_in_type_ref(ty: &TypeRef, span: Span) -> Option<Vec<(String, Span)>> {
    if !span_contains(ty.span, span.start) {
        return None;
    }
    match &ty.kind {
        TypeRefKind::Simple(_) => None,
        TypeRefKind::Generic { args, .. } => {
            for arg in args {
                if let Some(pairs) = map_literal_attr_pairs_in_type_ref(arg, span) {
                    return Some(pairs);
                }
            }
            None
        }
        TypeRefKind::Optional(inner) => map_literal_attr_pairs_in_type_ref(inner, span),
        TypeRefKind::Result { ok, err } => {
            map_literal_attr_pairs_in_type_ref(ok, span).or_else(|| {
                err.as_ref()
                    .and_then(|err| map_literal_attr_pairs_in_type_ref(err, span))
            })
        }
        TypeRefKind::Refined { args, .. } => {
            for arg in args {
                if let Some(pairs) = map_literal_attr_pairs_in_expr(arg, span) {
                    return Some(pairs);
                }
            }
            None
        }
    }
}

fn map_literal_attr_pairs_from_items(items: &[(Expr, Expr)]) -> Option<Vec<(String, Span)>> {
    let mut out = Vec::new();
    for (key, value) in items {
        let attr_name = html_attr_ident_from_map_key(key)?;
        out.push((attr_name, value.span));
    }
    Some(out)
}

fn html_attr_ident_from_map_key(key: &Expr) -> Option<String> {
    let ExprKind::Literal(Literal::String(raw_name)) = &key.kind else {
        return None;
    };
    let mut ident = String::new();
    for ch in raw_name.chars() {
        if ch == '-' {
            ident.push('_');
            continue;
        }
        if ch == '_' || ch.is_ascii_alphanumeric() {
            ident.push(ch);
            continue;
        }
        return None;
    }
    if !is_valid_ident(&ident) || is_keyword_or_literal(&ident) {
        return None;
    }
    Some(ident)
}

fn capability_sort_rank(capability: Capability) -> u8 {
    match capability {
        Capability::Db => 0,
        Capability::Network => 1,
        Capability::Time => 2,
        Capability::Crypto => 3,
    }
}

fn render_requires_block(capabilities: &[Capability]) -> String {
    capabilities
        .iter()
        .map(|capability| format!("requires {}", capability.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn missing_capability_requires_workspace_edit(
    uri: &str,
    text: &str,
    program: &Program,
    capability: Capability,
) -> Option<JsonValue> {
    let mut capabilities: Vec<Capability> = program
        .requires
        .iter()
        .map(|decl| decl.capability)
        .collect();
    if capabilities.contains(&capability) {
        return None;
    }
    capabilities.push(capability);
    capabilities.sort_by_key(|cap| capability_sort_rank(*cap));
    capabilities.dedup();

    let rendered = render_requires_block(&capabilities);
    if program.requires.is_empty() {
        let mut new_text = format!("{rendered}\n");
        if !text.is_empty() && !text.starts_with('\n') {
            new_text.push('\n');
        }
        return Some(workspace_edit_with_single_span(
            uri,
            text,
            Span::new(0, 0),
            &new_text,
        ));
    }

    let first = program
        .requires
        .iter()
        .map(|decl| line_start_offset(text, decl.span.start))
        .min()?;
    let mut end = program
        .requires
        .iter()
        .map(|decl| line_end_offset(text, decl.span.end))
        .max()?;
    while end < text.len() && text.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }

    let mut replacement = format!("{rendered}\n");
    if end < text.len() {
        replacement.push('\n');
    }
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(first, end),
        &replacement,
    ))
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
