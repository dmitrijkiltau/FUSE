use std::collections::{BTreeMap, HashMap};

use fuse_rt::json::JsonValue;
use fusec::ast::{
    Block, CallArg, Expr, ExprKind, Item, Program, Stmt, StmtKind, TypeRef, TypeRefKind,
};
use fusec::parse_source;
use fusec::span::Span;

use super::super::{
    COMPLETION_BUILTIN_FUNCTIONS, COMPLETION_BUILTIN_RECEIVERS, COMPLETION_BUILTIN_TYPES,
    COMPLETION_KEYWORDS, LspState, SymbolDef, SymbolKind, WorkspaceDef, WorkspaceIndex,
    build_workspace_index_cached, extract_position, is_callable_def_kind, is_exported_def_kind,
    line_col_to_offset, line_offsets, offset_to_line_col, span_contains,
};
use super::tokens::{load_text_for_uri, parse_fn_parameter_labels};

struct SignatureInfo {
    label: String,
    params: Vec<String>,
    documentation: Option<String>,
}

#[derive(Clone)]
enum SignatureTarget {
    Ident { name: String, span: Span },
    Member { base: Option<String>, span: Span },
    Other { span: Span },
}

#[derive(Clone)]
struct CallContext {
    span: Span,
    target: SignatureTarget,
    active_arg: usize,
}

pub(crate) fn handle_signature_help(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    let offsets = line_offsets(&text);
    let cursor = line_col_to_offset(&text, &offsets, line, character);
    let (program, _parse_diags) = parse_source(&text);
    let Some(call) = find_call_context(&program, cursor) else {
        return JsonValue::Null;
    };

    let index = build_workspace_index_cached(state, &uri);
    let signature = match &call.target {
        SignatureTarget::Ident { name, span } => {
            signature_info_for_symbol_ref(index, &uri, &offsets, *span)
                .or_else(|| builtin_signature_info(name))
        }
        SignatureTarget::Member { base, span } => {
            signature_info_for_symbol_ref(index, &uri, &offsets, *span).or_else(|| {
                base.as_deref()
                    .and_then(builtin_member_signature_info)
                    .or_else(|| base.as_deref().and_then(builtin_signature_info))
            })
        }
        SignatureTarget::Other { span } => {
            signature_info_for_symbol_ref(index, &uri, &offsets, *span)
        }
    };

    let Some(signature) = signature else {
        return JsonValue::Null;
    };
    signature_help_json(&signature, call.active_arg)
}

fn signature_help_json(signature: &SignatureInfo, active_arg: usize) -> JsonValue {
    let mut signature_obj = BTreeMap::new();
    signature_obj.insert(
        "label".to_string(),
        JsonValue::String(signature.label.clone()),
    );
    let params = signature
        .params
        .iter()
        .map(|label| {
            let mut param = BTreeMap::new();
            param.insert("label".to_string(), JsonValue::String(label.clone()));
            JsonValue::Object(param)
        })
        .collect();
    signature_obj.insert("parameters".to_string(), JsonValue::Array(params));
    if let Some(doc) = &signature.documentation {
        if !doc.trim().is_empty() {
            signature_obj.insert("documentation".to_string(), JsonValue::String(doc.clone()));
        }
    }

    let mut out = BTreeMap::new();
    out.insert(
        "signatures".to_string(),
        JsonValue::Array(vec![JsonValue::Object(signature_obj)]),
    );
    out.insert("activeSignature".to_string(), JsonValue::Number(0.0));
    let active_param = if signature.params.is_empty() {
        0usize
    } else {
        active_arg.min(signature.params.len().saturating_sub(1))
    };
    out.insert(
        "activeParameter".to_string(),
        JsonValue::Number(active_param as f64),
    );
    JsonValue::Object(out)
}

fn signature_info_for_symbol_ref(
    index: Option<&WorkspaceIndex>,
    uri: &str,
    offsets: &[usize],
    span: Span,
) -> Option<SignatureInfo> {
    let index = index?;
    let (line, col) = offset_to_line_col(offsets, span.start);
    let def = index.definition_at(uri, line, col)?;
    if def.def.kind != SymbolKind::Function {
        return None;
    }
    signature_info_from_function_detail(&def.def.detail, def.def.doc.as_deref())
}

fn signature_info_from_function_detail(detail: &str, doc: Option<&str>) -> Option<SignatureInfo> {
    let params = parse_fn_parameter_labels(detail)?;
    Some(SignatureInfo {
        label: detail.trim().to_string(),
        params,
        documentation: doc.map(|value| value.trim().to_string()),
    })
}

fn builtin_signature_info(name: &str) -> Option<SignatureInfo> {
    match name {
        "print" => Some(SignatureInfo {
            label: "fn print(value)".to_string(),
            params: vec!["value".to_string()],
            documentation: Some("Prints a value to stdout.".to_string()),
        }),
        "log" => Some(SignatureInfo {
            label: "fn log(level_or_message, message?, data?)".to_string(),
            params: vec![
                "level_or_message".to_string(),
                "message".to_string(),
                "data".to_string(),
            ],
            documentation: Some("Writes structured log output.".to_string()),
        }),
        "env" => Some(SignatureInfo {
            label: "fn env(name: String) -> String?".to_string(),
            params: vec!["name: String".to_string()],
            documentation: Some("Returns an environment variable value or null.".to_string()),
        }),
        "serve" => Some(SignatureInfo {
            label: "fn serve(port: Int)".to_string(),
            params: vec!["port: Int".to_string()],
            documentation: Some("Starts HTTP server on the given port.".to_string()),
        }),
        "assert" => Some(SignatureInfo {
            label: "fn assert(cond: Bool, message?)".to_string(),
            params: vec!["cond: Bool".to_string(), "message".to_string()],
            documentation: Some("Raises runtime error when condition is false.".to_string()),
        }),
        "asset" => Some(SignatureInfo {
            label: "fn asset(path: String) -> String".to_string(),
            params: vec!["path: String".to_string()],
            documentation: Some("Resolves logical asset path to public URL.".to_string()),
        }),
        _ => None,
    }
}

fn builtin_member_signature_info(base: &str) -> Option<SignatureInfo> {
    match base {
        "svg" => Some(SignatureInfo {
            label: "fn svg.inline(path: String) -> Html".to_string(),
            params: vec!["path: String".to_string()],
            documentation: Some(
                "Loads an SVG by logical name and returns inline Html.".to_string(),
            ),
        }),
        "request" => Some(SignatureInfo {
            label: "fn request.header(name: String) -> String?".to_string(),
            params: vec!["name: String".to_string()],
            documentation: Some(
                "Reads an inbound HTTP request header (case-insensitive), or null.".to_string(),
            ),
        }),
        "response" => Some(SignatureInfo {
            label: "fn response.header(name: String, value: String) -> Unit".to_string(),
            params: vec!["name: String".to_string(), "value: String".to_string()],
            documentation: Some(
                "Appends an HTTP response header for the current route response.".to_string(),
            ),
        }),
        "time" => Some(SignatureInfo {
            label: "fn time.format(epoch: Int, fmt: String) -> String".to_string(),
            params: vec!["epoch: Int".to_string(), "fmt: String".to_string()],
            documentation: Some(
                "Formats Unix epoch milliseconds using a strftime-style format string.".to_string(),
            ),
        }),
        "crypto" => Some(SignatureInfo {
            label: "fn crypto.hash(algo: String, data: Bytes) -> Bytes".to_string(),
            params: vec!["algo: String".to_string(), "data: Bytes".to_string()],
            documentation: Some("Computes a cryptographic digest (sha256/sha512).".to_string()),
        }),
        _ => None,
    }
}

fn find_call_context(program: &Program, cursor: usize) -> Option<CallContext> {
    let mut best = None;
    for item in &program.items {
        collect_call_context_item(item, cursor, &mut best);
    }
    best
}

fn collect_call_context_item(item: &Item, cursor: usize, best: &mut Option<CallContext>) {
    match item {
        Item::Import(_) => {}
        Item::Type(decl) => {
            for field in &decl.fields {
                collect_call_context_type_ref(&field.ty, cursor, best);
                if let Some(default) = &field.default {
                    collect_call_context_expr(default, cursor, best);
                }
            }
        }
        Item::Enum(decl) => {
            for variant in &decl.variants {
                for ty in &variant.payload {
                    collect_call_context_type_ref(ty, cursor, best);
                }
            }
        }
        Item::Fn(decl) => {
            for param in &decl.params {
                collect_call_context_type_ref(&param.ty, cursor, best);
                if let Some(default) = &param.default {
                    collect_call_context_expr(default, cursor, best);
                }
            }
            if let Some(ret) = &decl.ret {
                collect_call_context_type_ref(ret, cursor, best);
            }
            collect_call_context_block(&decl.body, cursor, best);
        }
        Item::Service(decl) => {
            for route in &decl.routes {
                collect_call_context_type_ref(&route.ret_type, cursor, best);
                if let Some(body_ty) = &route.body_type {
                    collect_call_context_type_ref(body_ty, cursor, best);
                }
                collect_call_context_block(&route.body, cursor, best);
            }
        }
        Item::Config(decl) => {
            for field in &decl.fields {
                collect_call_context_type_ref(&field.ty, cursor, best);
                collect_call_context_expr(&field.value, cursor, best);
            }
        }
        Item::Component(decl) => collect_call_context_block(&decl.body, cursor, best),
        Item::App(decl) => collect_call_context_block(&decl.body, cursor, best),
        Item::Migration(decl) => collect_call_context_block(&decl.body, cursor, best),
        Item::Test(decl) => collect_call_context_block(&decl.body, cursor, best),
    }
}

fn collect_call_context_block(block: &Block, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(block.span, cursor) {
        return;
    }
    for stmt in &block.stmts {
        collect_call_context_stmt(stmt, cursor, best);
    }
}

fn collect_call_context_stmt(stmt: &Stmt, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(stmt.span, cursor) {
        return;
    }
    match &stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(ty) = ty {
                collect_call_context_type_ref(ty, cursor, best);
            }
            collect_call_context_expr(expr, cursor, best);
        }
        StmtKind::Assign { target, expr } => {
            collect_call_context_expr(target, cursor, best);
            collect_call_context_expr(expr, cursor, best);
        }
        StmtKind::Return { expr } => {
            if let Some(expr) = expr {
                collect_call_context_expr(expr, cursor, best);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            collect_call_context_expr(cond, cursor, best);
            collect_call_context_block(then_block, cursor, best);
            for (cond, block) in else_if {
                collect_call_context_expr(cond, cursor, best);
                collect_call_context_block(block, cursor, best);
            }
            if let Some(block) = else_block {
                collect_call_context_block(block, cursor, best);
            }
        }
        StmtKind::Match { expr, cases } => {
            collect_call_context_expr(expr, cursor, best);
            for (_, block) in cases {
                collect_call_context_block(block, cursor, best);
            }
        }
        StmtKind::For { iter, block, .. } => {
            collect_call_context_expr(iter, cursor, best);
            collect_call_context_block(block, cursor, best);
        }
        StmtKind::While { cond, block } => {
            collect_call_context_expr(cond, cursor, best);
            collect_call_context_block(block, cursor, best);
        }
        StmtKind::Transaction { block } => collect_call_context_block(block, cursor, best),
        StmtKind::Expr(expr) => collect_call_context_expr(expr, cursor, best),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_call_context_expr(expr: &Expr, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(expr.span, cursor) {
        return;
    }
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            consider_call_context(best, expr.span, callee, args, cursor);
            collect_call_context_expr(callee, cursor, best);
            for arg in args {
                collect_call_context_expr(&arg.value, cursor, best);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_call_context_expr(left, cursor, best);
            collect_call_context_expr(right, cursor, best);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Await { expr } | ExprKind::Box { expr } => {
            collect_call_context_expr(expr, cursor, best);
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            collect_call_context_expr(base, cursor, best);
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            collect_call_context_expr(base, cursor, best);
            collect_call_context_expr(index, cursor, best);
        }
        ExprKind::StructLit { fields, .. } => {
            for field in fields {
                collect_call_context_expr(&field.value, cursor, best);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_call_context_expr(item, cursor, best);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_call_context_expr(key, cursor, best);
                collect_call_context_expr(value, cursor, best);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_call_context_expr(expr, cursor, best);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_call_context_expr(left, cursor, best);
            collect_call_context_expr(right, cursor, best);
        }
        ExprKind::BangChain { expr, error } => {
            collect_call_context_expr(expr, cursor, best);
            if let Some(error) = error {
                collect_call_context_expr(error, cursor, best);
            }
        }
        ExprKind::Spawn { block } => collect_call_context_block(block, cursor, best),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            collect_call_context_expr(cond, cursor, best);
            for child in then_children {
                collect_call_context_expr(child, cursor, best);
            }
            for (branch_cond, branch_children) in else_if {
                collect_call_context_expr(branch_cond, cursor, best);
                for child in branch_children {
                    collect_call_context_expr(child, cursor, best);
                }
            }
            for child in else_children {
                collect_call_context_expr(child, cursor, best);
            }
        }
        ExprKind::HtmlFor {
            iter,
            body_children,
            ..
        } => {
            collect_call_context_expr(iter, cursor, best);
            for child in body_children {
                collect_call_context_expr(child, cursor, best);
            }
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
}

fn collect_call_context_type_ref(ty: &TypeRef, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(ty.span, cursor) {
        return;
    }
    match &ty.kind {
        TypeRefKind::Simple(_) => {}
        TypeRefKind::Generic { args, .. } => {
            for arg in args {
                collect_call_context_type_ref(arg, cursor, best);
            }
        }
        TypeRefKind::Optional(inner) => collect_call_context_type_ref(inner, cursor, best),
        TypeRefKind::Result { ok, err } => {
            collect_call_context_type_ref(ok, cursor, best);
            if let Some(err) = err {
                collect_call_context_type_ref(err, cursor, best);
            }
        }
        TypeRefKind::Refined { args, .. } => {
            for arg in args {
                collect_call_context_expr(arg, cursor, best);
            }
        }
    }
}

fn consider_call_context(
    best: &mut Option<CallContext>,
    span: Span,
    callee: &Expr,
    args: &[CallArg],
    cursor: usize,
) {
    let target = match &callee.kind {
        ExprKind::Ident(ident) => SignatureTarget::Ident {
            name: ident.name.clone(),
            span: ident.span,
        },
        ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
            let base = match &base.kind {
                ExprKind::Ident(base_ident) => Some(base_ident.name.clone()),
                _ => None,
            };
            SignatureTarget::Member {
                base,
                span: name.span,
            }
        }
        _ => SignatureTarget::Other { span: callee.span },
    };
    let candidate = CallContext {
        span,
        target,
        active_arg: call_active_argument(args, cursor),
    };
    let span_len = candidate.span.end.saturating_sub(candidate.span.start);
    let replace = best.as_ref().map_or(true, |current| {
        span_len < current.span.end.saturating_sub(current.span.start)
    });
    if replace {
        *best = Some(candidate);
    }
}

fn call_active_argument(args: &[CallArg], cursor: usize) -> usize {
    if args.is_empty() {
        return 0;
    }
    if cursor <= args[0].span.start {
        return 0;
    }
    for (idx, arg) in args.iter().enumerate() {
        if cursor <= arg.span.end {
            return idx;
        }
        if let Some(next) = args.get(idx + 1) {
            if cursor < next.span.start {
                return idx + 1;
            }
        }
    }
    args.len()
}

#[derive(Clone)]
struct CompletionCandidate {
    kind: u32,
    detail: Option<String>,
    sort_group: u8,
}

pub(crate) fn handle_completion(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return completion_list_json(Vec::new());
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return completion_list_json(Vec::new());
    };
    let offsets = line_offsets(&text);
    let offset = line_col_to_offset(&text, &offsets, line, character);
    let prefix_start = completion_ident_start(&text, offset);
    let prefix = text.get(prefix_start..offset).unwrap_or_default();
    let member_base = completion_member_base(&text, prefix_start);
    let (program, _parse_diags) = parse_source(&text);
    let index = build_workspace_index_cached(state, &uri);
    let current_container = completion_callable_name_at_cursor(&program, offset)
        .or_else(|| index.and_then(|index| completion_container_name(index, &uri, offset)));
    let mut candidates: HashMap<String, CompletionCandidate> = HashMap::new();

    if let Some(base) = member_base {
        if let Some(index) = index {
            for name in index.alias_exports_for_module(&uri, &base) {
                let mut kind = 3u32;
                let mut detail = None;
                if let Some(def) = index
                    .defs
                    .iter()
                    .find(|def| def.def.name == name && is_exported_def_kind(def.def.kind))
                {
                    kind = completion_kind_for_symbol_kind(def.def.kind);
                    if !def.def.detail.is_empty() {
                        detail = Some(def.def.detail.clone());
                    }
                }
                upsert_completion_candidate(&mut candidates, &name, kind, detail, 0);
            }
        }
        for method in builtin_receiver_methods(&base) {
            upsert_completion_candidate(
                &mut candidates,
                method,
                2,
                Some(format!("{base} builtin")),
                0,
            );
        }
    } else {
        if let Some(index) = index {
            for def in &index.defs {
                let sort_group =
                    completion_symbol_sort_group(def, &uri, current_container.as_deref());
                let kind = completion_kind_for_symbol_kind(def.def.kind);
                let detail = if def.def.detail.is_empty() {
                    None
                } else {
                    Some(def.def.detail.clone())
                };
                upsert_completion_candidate(
                    &mut candidates,
                    &def.def.name,
                    kind,
                    detail,
                    sort_group,
                );
            }
        }
        for builtin in COMPLETION_BUILTIN_RECEIVERS {
            upsert_completion_candidate(
                &mut candidates,
                builtin,
                9,
                Some("builtin namespace".to_string()),
                3,
            );
        }
        for builtin in COMPLETION_BUILTIN_FUNCTIONS {
            upsert_completion_candidate(
                &mut candidates,
                builtin,
                3,
                Some("builtin function".to_string()),
                3,
            );
        }
        for builtin in COMPLETION_BUILTIN_TYPES {
            upsert_completion_candidate(
                &mut candidates,
                builtin,
                22,
                Some("builtin type".to_string()),
                3,
            );
        }
        for tag in fusec::html_tags::all_html_tags() {
            upsert_completion_candidate(
                &mut candidates,
                tag,
                3,
                Some("html tag builtin".to_string()),
                3,
            );
        }
        for keyword in COMPLETION_KEYWORDS {
            upsert_completion_candidate(&mut candidates, keyword, 14, None, 4);
        }
        for literal in ["true", "false", "null"] {
            upsert_completion_candidate(&mut candidates, literal, 14, None, 4);
        }
    }

    let mut entries: Vec<(String, CompletionCandidate)> = candidates
        .into_iter()
        .filter(|(label, _)| completion_label_matches(label, prefix))
        .collect();
    entries.sort_by(|(left_label, left), (right_label, right)| {
        left.sort_group
            .cmp(&right.sort_group)
            .then_with(|| left_label.to_lowercase().cmp(&right_label.to_lowercase()))
            .then_with(|| left_label.cmp(right_label))
    });
    if entries.len() > 256 {
        entries.truncate(256);
    }
    let items = entries
        .into_iter()
        .map(|(label, candidate)| {
            completion_item_json(
                &label,
                candidate.kind,
                candidate.detail.as_deref(),
                candidate.sort_group,
            )
        })
        .collect();
    completion_list_json(items)
}

fn completion_item_json(label: &str, kind: u32, detail: Option<&str>, sort_group: u8) -> JsonValue {
    let mut item = BTreeMap::new();
    item.insert("label".to_string(), JsonValue::String(label.to_string()));
    item.insert("kind".to_string(), JsonValue::Number(kind as f64));
    if let Some(detail) = detail {
        item.insert("detail".to_string(), JsonValue::String(detail.to_string()));
    }
    item.insert(
        "sortText".to_string(),
        JsonValue::String(format!("{sort_group:02}_{}", label.to_lowercase())),
    );
    JsonValue::Object(item)
}

fn completion_list_json(items: Vec<JsonValue>) -> JsonValue {
    let mut out = BTreeMap::new();
    out.insert("isIncomplete".to_string(), JsonValue::Bool(false));
    out.insert("items".to_string(), JsonValue::Array(items));
    JsonValue::Object(out)
}

fn upsert_completion_candidate(
    candidates: &mut HashMap<String, CompletionCandidate>,
    label: &str,
    kind: u32,
    detail: Option<String>,
    sort_group: u8,
) {
    use std::collections::hash_map::Entry;
    match candidates.entry(label.to_string()) {
        Entry::Vacant(slot) => {
            slot.insert(CompletionCandidate {
                kind,
                detail,
                sort_group,
            });
        }
        Entry::Occupied(mut slot) => {
            let existing = slot.get();
            if sort_group < existing.sort_group {
                slot.insert(CompletionCandidate {
                    kind,
                    detail,
                    sort_group,
                });
                return;
            }
            if sort_group == existing.sort_group && existing.detail.is_none() && detail.is_some() {
                let existing = slot.get_mut();
                existing.detail = detail;
                existing.kind = kind;
            }
        }
    }
}

fn completion_label_matches(label: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let label_lower = label.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    label_lower.starts_with(&prefix_lower)
}

fn completion_ident_start(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = offset.min(bytes.len());
    while pos > 0 && is_ident_byte(bytes[pos - 1]) {
        pos -= 1;
    }
    pos
}

fn completion_member_base(text: &str, prefix_start: usize) -> Option<String> {
    if prefix_start == 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let dot = prefix_start - 1;
    if bytes.get(dot).copied() != Some(b'.') {
        return None;
    }
    let mut end = dot;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    completion_receiver_base(text, end)
}

fn is_ident_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn completion_receiver_base(text: &str, end: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let end = completion_skip_ws_left(bytes, end);
    if end == 0 {
        return None;
    }
    let tail = bytes[end - 1];

    if is_ident_byte(tail) {
        let mut start = end;
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        if start == end {
            return None;
        }
        let ident = text[start..end].to_string();
        let left = completion_skip_ws_left(bytes, start);
        if left > 0 && bytes[left - 1] == b'.' {
            return completion_receiver_base(text, left - 1);
        }
        return Some(ident);
    }

    if tail == b')' {
        let open = completion_find_matching_left(bytes, end - 1, b'(', b')')?;
        let callee_end = completion_skip_ws_left(bytes, open);
        if callee_end == 0 {
            return None;
        }
        let mut callee_start = callee_end;
        while callee_start > 0 && is_ident_byte(bytes[callee_start - 1]) {
            callee_start -= 1;
        }
        if callee_start == callee_end {
            return None;
        }
        let left = completion_skip_ws_left(bytes, callee_start);
        if left == 0 || bytes[left - 1] != b'.' {
            return None;
        }
        return completion_receiver_base(text, left - 1);
    }

    if tail == b']' {
        let open = completion_find_matching_left(bytes, end - 1, b'[', b']')?;
        return completion_receiver_base(text, open);
    }

    None
}

fn completion_skip_ws_left(bytes: &[u8], mut pos: usize) -> usize {
    while pos > 0 && bytes[pos - 1].is_ascii_whitespace() {
        pos -= 1;
    }
    pos
}

fn completion_find_matching_left(
    bytes: &[u8],
    close_idx: usize,
    open_char: u8,
    close_char: u8,
) -> Option<usize> {
    let mut depth = 0usize;
    let mut idx = close_idx + 1;
    while idx > 0 {
        idx -= 1;
        let ch = bytes[idx];
        if ch == close_char {
            depth += 1;
            continue;
        }
        if ch == open_char {
            if depth == 0 {
                return None;
            }
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn completion_container_name(index: &WorkspaceIndex, uri: &str, offset: usize) -> Option<String> {
    let mut best: Option<(&WorkspaceDef, usize)> = None;
    for def in &index.defs {
        if def.uri != uri || !is_callable_def_kind(def.def.kind) {
            continue;
        }
        if !span_contains(def.def.span, offset) {
            continue;
        }
        let size = def.def.span.end.saturating_sub(def.def.span.start);
        if best.map_or(true, |(_, best_size)| size < best_size) {
            best = Some((def, size));
        }
    }
    best.map(|(def, _)| def.def.name.clone())
}

fn completion_callable_name_at_cursor(program: &Program, cursor: usize) -> Option<String> {
    let mut best: Option<(String, usize)> = None;

    for item in &program.items {
        match item {
            Item::Fn(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.name.clone(), size));
                    }
                }
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    if span_contains(route.body.span, cursor) {
                        let size = route.body.span.end.saturating_sub(route.body.span.start);
                        if best
                            .as_ref()
                            .map_or(true, |(_, best_size)| size < *best_size)
                        {
                            best = Some((decl.name.name.clone(), size));
                        }
                    }
                }
            }
            Item::App(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.value.clone(), size));
                    }
                }
            }
            Item::Migration(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.clone(), size));
                    }
                }
            }
            Item::Test(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.value.clone(), size));
                    }
                }
            }
            Item::Component(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.name.clone(), size));
                    }
                }
            }
            Item::Import(_) | Item::Type(_) | Item::Enum(_) | Item::Config(_) => {}
        }
    }

    best.map(|(name, _)| name)
}

fn completion_symbol_sort_group(def: &WorkspaceDef, uri: &str, container: Option<&str>) -> u8 {
    if def.uri != uri {
        return 2;
    }
    if is_lexical_local_symbol(&def.def) {
        if container.is_some_and(|name| def.def.container.as_deref() == Some(name)) {
            return 0;
        }
        return 1;
    }
    1
}

fn is_lexical_local_symbol(def: &SymbolDef) -> bool {
    if !matches!(def.kind, SymbolKind::Param | SymbolKind::Variable) {
        return false;
    }
    def.detail.starts_with("param ")
        || def.detail.starts_with("let ")
        || def.detail.starts_with("var ")
}

fn completion_kind_for_symbol_kind(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Module => 9,
        SymbolKind::Type | SymbolKind::Config => 22,
        SymbolKind::Enum => 13,
        SymbolKind::EnumVariant => 20,
        SymbolKind::Function
        | SymbolKind::Service
        | SymbolKind::App
        | SymbolKind::Migration
        | SymbolKind::Test => 3,
        SymbolKind::Param | SymbolKind::Variable => 6,
        SymbolKind::Field => 5,
    }
}

fn builtin_receiver_methods(receiver: &str) -> &'static [&'static str] {
    match receiver {
        "db" => &[
            "exec",
            "query",
            "one",
            "from",
            "select",
            "where",
            "all",
            "first",
            "limit",
            "offset",
            "order_by",
            "insert",
            "upsert",
            "update",
            "delete",
            "set",
            "join",
            "left_join",
            "right_join",
            "group_by",
            "having",
            "count",
        ],
        "json" => &["encode", "decode"],
        "html" => &["text", "raw", "node", "render"],
        "svg" => &["inline"],
        "request" => &["header", "cookie"],
        "response" => &["header", "cookie", "delete_cookie"],
        "time" => &["now", "sleep", "format", "parse"],
        "crypto" => &["hash", "hmac", "random_bytes", "constant_time_eq"],
        _ => &[],
    }
}
