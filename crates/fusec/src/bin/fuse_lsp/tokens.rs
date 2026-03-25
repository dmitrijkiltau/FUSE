use std::collections::{BTreeMap, HashMap, HashSet};

use fuse_rt::json::JsonValue;
use fusec::ast::{BinaryOp, Block, Expr, ExprKind, Item, Literal, Program, StmtKind, UnaryOp};
use fusec::diag::Level;
use fusec::parse_source;
use fusec::span::Span;

use super::super::{
    LspState, SEM_COMMENT, SEM_ENUM, SEM_ENUM_MEMBER, SEM_FUNCTION, SEM_HTML_ATTRIBUTE,
    SEM_HTML_TAG, SEM_KEYWORD, SEM_NAMESPACE, SEM_NUMBER, SEM_PARAMETER, SEM_PROPERTY, SEM_STRING,
    SEM_TYPE, SEM_VARIABLE, SymbolKind, WorkspaceIndex, build_index_with_program,
    build_workspace_index_cached, extract_text_doc_uri, line_offsets, lsp_range_to_span,
    offset_to_line_col, uri_to_path,
};

pub(crate) fn handle_semantic_tokens(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    semantic_tokens_for_text(state, &uri, &text, None)
}

pub(crate) fn handle_semantic_tokens_range(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
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
    state: &mut LspState,
    uri: &str,
    text: &str,
    range: Option<Span>,
) -> JsonValue {
    let (program, _) = parse_source(text);
    let html_semantic_spans = collect_html_semantic_spans(&program);
    let mut symbol_types: HashMap<(usize, usize), usize> = HashMap::new();
    if let Some(index) = build_workspace_index_cached(state, uri) {
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
        let token_key = (token.span.start, token.span.end);
        let token_type = if html_semantic_spans.attr_name_spans.contains(&token_key) {
            Some(SEM_HTML_ATTRIBUTE)
        } else if html_semantic_spans.dsl_name_spans.contains(&token_key) {
            Some(SEM_HTML_TAG)
        } else {
            match &token.kind {
                fusec::token::TokenKind::InterpString(_) => {
                    collect_interp_string_semantic_rows(
                        text,
                        token.span,
                        &symbol_types,
                        range,
                        &mut rows,
                    );
                    None
                }
                fusec::token::TokenKind::Keyword(fusec::token::Keyword::From) => {
                    semantic_member_token_type(&tokens, idx).or(Some(SEM_KEYWORD))
                }
                fusec::token::TokenKind::Keyword(_) => Some(SEM_KEYWORD),
                fusec::token::TokenKind::String(_) => Some(SEM_STRING),
                fusec::token::TokenKind::Int(_) | fusec::token::TokenKind::Float(_) => {
                    Some(SEM_NUMBER)
                }
                fusec::token::TokenKind::DocComment(_) => Some(SEM_COMMENT),
                fusec::token::TokenKind::Bool(_) | fusec::token::TokenKind::Null => {
                    Some(SEM_KEYWORD)
                }
                fusec::token::TokenKind::Ident(name) => symbol_types
                    .get(&(token.span.start, token.span.end))
                    .copied()
                    .or_else(|| semantic_ident_fallback(&tokens, idx, name))
                    .or(Some(SEM_VARIABLE)),
                _ => None,
            }
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

struct HtmlSemanticSpans {
    attr_name_spans: HashSet<(usize, usize)>,
    dsl_name_spans: HashSet<(usize, usize)>,
}

fn collect_interp_string_semantic_rows(
    text: &str,
    span: Span,
    symbol_types: &HashMap<(usize, usize), usize>,
    range: Option<Span>,
    rows: &mut Vec<SemanticTokenRow>,
) {
    let Some(raw) = text.get(span.start..span.end) else {
        return;
    };
    let quote_len = if raw.starts_with("\"\"\"") { 3 } else { 1 };
    if span.end < span.start.saturating_add(quote_len * 2) {
        return;
    }
    let content_start = span.start + quote_len;
    let content_end = span.end - quote_len;
    let mut chunk_start = content_start;
    let mut cursor = content_start;

    while cursor < content_end {
        let Some(slice) = text.get(cursor..content_end) else {
            break;
        };
        let Some(ch) = slice.chars().next() else {
            break;
        };
        if ch == '\\' {
            cursor += ch.len_utf8();
            if cursor < content_end {
                let Some(escaped) = text.get(cursor..content_end).and_then(|rest| rest.chars().next()) else {
                    break;
                };
                cursor += escaped.len_utf8();
            }
            continue;
        }
        if ch == '$' {
            let expr_open = cursor + ch.len_utf8();
            if expr_open < content_end
                && text
                    .get(expr_open..content_end)
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(|next| next == '{')
            {
                push_semantic_row(rows, Span::new(chunk_start, cursor), SEM_STRING, range);
                let expr_start = expr_open + 1;
                let Some(expr_end) = find_interpolation_expr_end(text, expr_start, content_end) else {
                    return;
                };
                collect_embedded_expr_semantic_rows(
                    text,
                    expr_start,
                    expr_end,
                    symbol_types,
                    range,
                    rows,
                );
                cursor = expr_end + 1;
                chunk_start = cursor;
                continue;
            }
        }
        cursor += ch.len_utf8();
    }

    push_semantic_row(rows, Span::new(chunk_start, content_end), SEM_STRING, range);
}

fn find_interpolation_expr_end(text: &str, expr_start: usize, content_end: usize) -> Option<usize> {
    let mut cursor = expr_start;
    let mut depth = 1usize;
    let mut string_delim: Option<usize> = None;
    let mut escape = false;

    while cursor < content_end {
        let slice = text.get(cursor..content_end)?;
        if string_delim == Some(3) && slice.starts_with("\"\"\"") {
            string_delim = None;
            cursor += 3;
            escape = false;
            continue;
        }
        let ch = slice.chars().next()?;
        if let Some(delim) = string_delim {
            if escape {
                escape = false;
                cursor += ch.len_utf8();
                continue;
            }
            if ch == '\\' {
                escape = true;
                cursor += ch.len_utf8();
                continue;
            }
            if delim == 1 && ch == '"' {
                string_delim = None;
            }
            cursor += ch.len_utf8();
            continue;
        }
        if slice.starts_with("\"\"\"") {
            string_delim = Some(3);
            cursor += 3;
            continue;
        }
        if ch == '"' {
            string_delim = Some(1);
            cursor += ch.len_utf8();
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(cursor);
            }
        }
        cursor += ch.len_utf8();
    }

    None
}

fn collect_embedded_expr_semantic_rows(
    text: &str,
    expr_start: usize,
    expr_end: usize,
    symbol_types: &HashMap<(usize, usize), usize>,
    range: Option<Span>,
    rows: &mut Vec<SemanticTokenRow>,
) {
    let Some(expr_text) = text.get(expr_start..expr_end) else {
        return;
    };
    let mut diags = fusec::diag::Diagnostics::default();
    let mut expr_tokens = fusec::lexer::lex(expr_text, &mut diags);
    for token in &mut expr_tokens {
        token.span.start += expr_start;
        token.span.end += expr_start;
    }
    for (idx, token) in expr_tokens.iter().enumerate() {
        let Some(token_type) = semantic_token_type_for_token(&expr_tokens, idx, symbol_types) else {
            continue;
        };
        push_semantic_row(rows, token.span, token_type, range);
    }
}

fn push_semantic_row(
    rows: &mut Vec<SemanticTokenRow>,
    span: Span,
    token_type: usize,
    range: Option<Span>,
) {
    if span.start >= span.end {
        return;
    }
    if let Some(range) = range {
        if span.end < range.start || span.start > range.end {
            return;
        }
    }
    rows.push(SemanticTokenRow { span, token_type });
}

fn semantic_token_type_for_token(
    tokens: &[fusec::token::Token],
    idx: usize,
    symbol_types: &HashMap<(usize, usize), usize>,
) -> Option<usize> {
    let token = tokens.get(idx)?;
    match &token.kind {
        fusec::token::TokenKind::Keyword(fusec::token::Keyword::From) => {
            semantic_member_token_type(tokens, idx).or(Some(SEM_KEYWORD))
        }
        fusec::token::TokenKind::Keyword(_) => Some(SEM_KEYWORD),
        fusec::token::TokenKind::String(_) => Some(SEM_STRING),
        fusec::token::TokenKind::InterpString(_) => None,
        fusec::token::TokenKind::Int(_) | fusec::token::TokenKind::Float(_) => Some(SEM_NUMBER),
        fusec::token::TokenKind::DocComment(_) => Some(SEM_COMMENT),
        fusec::token::TokenKind::Bool(_) | fusec::token::TokenKind::Null => Some(SEM_KEYWORD),
        fusec::token::TokenKind::Ident(name) => symbol_types
            .get(&(token.span.start, token.span.end))
            .copied()
            .or_else(|| semantic_ident_fallback(tokens, idx, name))
            .or(Some(SEM_VARIABLE)),
        fusec::token::TokenKind::Indent
        | fusec::token::TokenKind::Dedent
        | fusec::token::TokenKind::Newline
        | fusec::token::TokenKind::Eof
        | fusec::token::TokenKind::Punct(_) => None,
    }
}

fn collect_html_semantic_spans(program: &Program) -> HtmlSemanticSpans {
    let component_names = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Component(decl) => Some(decl.name.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut out = HtmlSemanticSpans {
        attr_name_spans: HashSet::new(),
        dsl_name_spans: HashSet::new(),
    };
    for item in &program.items {
        match item {
            Item::Fn(decl) => {
                collect_html_semantic_spans_block(&decl.body, &component_names, &mut out);
            }
            Item::Component(decl) => {
                out.dsl_name_spans
                    .insert((decl.name.span.start, decl.name.span.end));
                collect_html_semantic_spans_block(&decl.body, &component_names, &mut out);
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_html_semantic_spans_block(&route.body, &component_names, &mut out);
                }
            }
            Item::App(decl) => {
                collect_html_semantic_spans_block(&decl.body, &component_names, &mut out);
            }
            Item::Migration(decl) => {
                collect_html_semantic_spans_block(&decl.body, &component_names, &mut out);
            }
            Item::Test(decl) => {
                collect_html_semantic_spans_block(&decl.body, &component_names, &mut out);
            }
            Item::Import(_)
            | Item::Type(_)
            | Item::Enum(_)
            | Item::Config(_)
            | Item::Interface(_)
            | Item::Impl(_) => {}
        }
    }
    out
}

fn collect_html_semantic_spans_block(
    block: &Block,
    component_names: &HashSet<String>,
    out: &mut HtmlSemanticSpans,
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let { expr, .. } | StmtKind::Var { expr, .. } => {
                collect_html_semantic_spans_expr(expr, component_names, out);
            }
            StmtKind::Assign { target, expr } => {
                collect_html_semantic_spans_expr(target, component_names, out);
                collect_html_semantic_spans_expr(expr, component_names, out);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    collect_html_semantic_spans_expr(expr, component_names, out);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                collect_html_semantic_spans_expr(cond, component_names, out);
                collect_html_semantic_spans_block(then_block, component_names, out);
                for (branch_cond, branch_block) in else_if {
                    collect_html_semantic_spans_expr(branch_cond, component_names, out);
                    collect_html_semantic_spans_block(branch_block, component_names, out);
                }
                if let Some(else_block) = else_block {
                    collect_html_semantic_spans_block(else_block, component_names, out);
                }
            }
            StmtKind::Match { expr, cases } => {
                collect_html_semantic_spans_expr(expr, component_names, out);
                for (_, case_block) in cases {
                    collect_html_semantic_spans_block(case_block, component_names, out);
                }
            }
            StmtKind::For { iter, block, .. } => {
                collect_html_semantic_spans_expr(iter, component_names, out);
                collect_html_semantic_spans_block(block, component_names, out);
            }
            StmtKind::While { cond, block } => {
                collect_html_semantic_spans_expr(cond, component_names, out);
                collect_html_semantic_spans_block(block, component_names, out);
            }
            StmtKind::Transaction { block } => {
                collect_html_semantic_spans_block(block, component_names, out);
            }
            StmtKind::Expr(expr) => {
                collect_html_semantic_spans_expr(expr, component_names, out);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn collect_html_semantic_spans_expr(
    expr: &Expr,
    component_names: &HashSet<String>,
    out: &mut HtmlSemanticSpans,
) {
    match &expr.kind {
        ExprKind::Call { callee, args, .. } => {
            let html_dsl_context = match &callee.kind {
                ExprKind::Ident(ident)
                    if fusec::html_tags::is_html_tag(&ident.name)
                        || component_names.contains(&ident.name) =>
                {
                    out.dsl_name_spans
                        .insert((ident.span.start, ident.span.end));
                    true
                }
                _ => false,
            };
            if html_dsl_context {
                for arg in args {
                    if let Some(name) = &arg.name {
                        out.attr_name_spans.insert((name.span.start, name.span.end));
                    }
                }
            }
            collect_html_semantic_spans_expr(callee, component_names, out);
            for arg in args {
                collect_html_semantic_spans_expr(&arg.value, component_names, out);
            }
        }
        ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
            collect_html_semantic_spans_expr(left, component_names, out);
            collect_html_semantic_spans_expr(right, component_names, out);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Await { expr } | ExprKind::Box { expr } => {
            collect_html_semantic_spans_expr(expr, component_names, out);
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            collect_html_semantic_spans_expr(base, component_names, out);
        }
        ExprKind::Index {
            base,
            index: index_expr,
        }
        | ExprKind::OptionalIndex {
            base,
            index: index_expr,
        } => {
            collect_html_semantic_spans_expr(base, component_names, out);
            collect_html_semantic_spans_expr(index_expr, component_names, out);
        }
        ExprKind::StructLit { fields, .. } => {
            for field in fields {
                collect_html_semantic_spans_expr(&field.value, component_names, out);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_html_semantic_spans_expr(item, component_names, out);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_html_semantic_spans_expr(key, component_names, out);
                collect_html_semantic_spans_expr(value, component_names, out);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_html_semantic_spans_expr(expr, component_names, out);
                }
            }
        }
        ExprKind::BangChain { expr, error } => {
            collect_html_semantic_spans_expr(expr, component_names, out);
            if let Some(error) = error {
                collect_html_semantic_spans_expr(error, component_names, out);
            }
        }
        ExprKind::Spawn { block } => {
            collect_html_semantic_spans_block(block, component_names, out);
        }
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            collect_html_semantic_spans_expr(cond, component_names, out);
            for child in then_children {
                collect_html_semantic_spans_expr(child, component_names, out);
            }
            for (branch_cond, branch_children) in else_if {
                collect_html_semantic_spans_expr(branch_cond, component_names, out);
                for child in branch_children {
                    collect_html_semantic_spans_expr(child, component_names, out);
                }
            }
            for child in else_children {
                collect_html_semantic_spans_expr(child, component_names, out);
            }
        }
        ExprKind::HtmlFor {
            iter,
            body_children,
            ..
        } => {
            collect_html_semantic_spans_expr(iter, component_names, out);
            for child in body_children {
                collect_html_semantic_spans_expr(child, component_names, out);
            }
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
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
    matches!(
        name,
        "db" | "json" | "html" | "svg" | "request" | "response" | "http" | "time" | "crypto"
    )
}

fn is_builtin_function_name(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "env"
            | "env_int"
            | "env_float"
            | "env_bool"
            | "serve"
            | "log"
            | "assert"
            | "asset"
    ) || fusec::html_tags::is_html_tag(name)
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

pub(crate) fn handle_inlay_hints(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
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
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let offsets = line_offsets(&text);
    let mut hints = Vec::new();
    let mut seen = HashSet::new();
    for item in &program.items {
        match item {
            Item::Fn(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_inlay_hints_block(
                        index,
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
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Migration(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Test(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            _ => {}
        }
    }
    JsonValue::Array(hints)
}

fn semantic_type_for_symbol_kind(kind: SymbolKind) -> Option<usize> {
    match kind {
        SymbolKind::Module => Some(SEM_NAMESPACE),
        SymbolKind::Type | SymbolKind::Interface | SymbolKind::Config => Some(SEM_TYPE),
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

pub(super) fn load_text_for_uri(state: &LspState, uri: &str) -> Option<String> {
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
            StmtKind::Transaction { block } => {
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
        ExprKind::Call { callee, args, .. } => {
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
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            collect_inlay_hints_expr(index, uri, text, offsets, cond, range, hints, seen);
            for child in then_children {
                collect_inlay_hints_expr(index, uri, text, offsets, child, range, hints, seen);
            }
            for (branch_cond, branch_children) in else_if {
                collect_inlay_hints_expr(
                    index,
                    uri,
                    text,
                    offsets,
                    branch_cond,
                    range,
                    hints,
                    seen,
                );
                for child in branch_children {
                    collect_inlay_hints_expr(index, uri, text, offsets, child, range, hints, seen);
                }
            }
            for child in else_children {
                collect_inlay_hints_expr(index, uri, text, offsets, child, range, hints, seen);
            }
        }
        ExprKind::HtmlFor {
            iter,
            body_children,
            ..
        } => {
            collect_inlay_hints_expr(index, uri, text, offsets, iter, range, hints, seen);
            for child in body_children {
                collect_inlay_hints_expr(index, uri, text, offsets, child, range, hints, seen);
            }
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
    let params = parse_fn_parameter_labels(detail)?;
    let mut names = Vec::new();
    for part in params {
        let name = part.split(':').next().unwrap_or("").trim();
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    Some(names)
}

pub(super) fn parse_fn_parameter_labels(detail: &str) -> Option<Vec<String>> {
    if !detail.starts_with("fn ") {
        return None;
    }
    let open = detail.find('(')?;
    let close = find_matching_paren(detail, open)?;
    if close <= open {
        return Some(Vec::new());
    }
    let params_text = detail[open + 1..close].trim();
    if params_text.is_empty() {
        return Some(Vec::new());
    }
    Some(split_top_level_csv(params_text))
}

fn find_matching_paren(text: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in text[open_idx..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(open_idx + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_csv(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (idx, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ',' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                let part = text[start..idx].trim();
                if !part.is_empty() {
                    out.push(part.to_string());
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
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
        ExprKind::HtmlIf { .. } | ExprKind::HtmlFor { .. } => Some("List".to_string()),
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
