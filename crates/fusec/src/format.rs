pub fn format_source(src: &str) -> String {
    if src.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut last_blank = false;
    for raw_line in src.split_terminator('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed = line.trim_end_matches(|c| c == ' ' || c == '\t');
        if trimmed.is_empty() {
            if !last_blank {
                lines.push(String::new());
                last_blank = true;
            }
            continue;
        }
        last_blank = false;
        let mut indent = String::new();
        let mut rest_index = 0;
        for (idx, ch) in trimmed.char_indices() {
            match ch {
                ' ' => {
                    indent.push(' ');
                    rest_index = idx + ch.len_utf8();
                }
                '\t' => {
                    indent.push(' ');
                    rest_index = idx + ch.len_utf8();
                }
                _ => {
                    rest_index = idx;
                    break;
                }
            }
        }
        let rest = &trimmed[rest_index..];
        let formatted = if rest.trim_start().starts_with('#') {
            rest.to_string()
        } else {
            let spaced = normalize_field_colon_spacing(rest);
            normalize_quotes(&spaced)
        };
        lines.push(format!("{indent}{formatted}"));
    }
    while matches!(lines.last(), Some(line) if line.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

fn normalize_field_colon_spacing(rest: &str) -> String {
    let mut ident = String::new();
    let mut ident_end = None;
    for (idx, ch) in rest.char_indices() {
        if idx == 0 {
            if is_ident_start(ch) {
                ident.push(ch);
                ident_end = Some(idx + ch.len_utf8());
            } else {
                return rest.to_string();
            }
            continue;
        }
        if is_ident_continue(ch) {
            ident.push(ch);
            ident_end = Some(idx + ch.len_utf8());
        } else {
            break;
        }
    }
    let Some(mut cursor) = ident_end else {
        return rest.to_string();
    };
    while let Some(ch) = rest[cursor..].chars().next() {
        if ch == ' ' || ch == '\t' {
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }
    if !rest[cursor..].starts_with(':') {
        return rest.to_string();
    }
    if rest[cursor..].starts_with("::") {
        return rest.to_string();
    }
    cursor += 1;
    while let Some(ch) = rest[cursor..].chars().next() {
        if ch == ' ' || ch == '\t' {
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }
    let tail = &rest[cursor..];
    format!("{ident}: {tail}")
}

fn normalize_quotes(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    let mut in_double = false;
    let mut escape = false;
    while let Some(ch) = chars.next() {
        if in_double {
            if escape {
                escape = false;
                out.push(ch);
                continue;
            }
            if ch == '\\' {
                escape = true;
                out.push(ch);
                continue;
            }
            if ch == '"' {
                in_double = false;
            }
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_double = true;
            out.push(ch);
            continue;
        }
        if ch == '\'' {
            let mut content = String::new();
            let mut closed = false;
            let mut escaped = false;
            while let Some(next) = chars.next() {
                if escaped {
                    content.push(next);
                    escaped = false;
                    continue;
                }
                if next == '\\' {
                    escaped = true;
                    content.push(next);
                    continue;
                }
                if next == '\'' {
                    closed = true;
                    break;
                }
                content.push(next);
            }
            if closed && !content.contains('"') && !content.contains('\\') {
                out.push('"');
                out.push_str(&content);
                out.push('"');
            } else {
                out.push('\'');
                out.push_str(&content);
                if closed {
                    out.push('\'');
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
