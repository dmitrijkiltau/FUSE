use crate::diag::Diagnostics;
use crate::span::Span;
use crate::token::{InterpSegment, Keyword, Punct, Token, TokenKind};

pub fn lex(src: &str, diags: &mut Diagnostics) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut indent_stack: Vec<usize> = vec![0];
    let mut nesting: i32 = 0;
    let mut offset: usize = 0;
    let mut string_resume_at: Option<usize> = None;

    for raw_line in src.split_inclusive('\n') {
        let has_newline = raw_line.ends_with('\n');
        let line = if has_newline {
            &raw_line[..raw_line.len() - 1]
        } else {
            raw_line
        };
        let line_start = offset;
        offset += raw_line.len();
        let line_end = line_start + line.len();

        let indent_active = nesting == 0;
        let mut idx = 0usize;
        let mut col = 0usize;
        let mut saw_tab = false;
        let mut resumed_from_multiline = false;

        if let Some(resume) = string_resume_at {
            if resume >= line_start + raw_line.len() {
                continue;
            }
            if resume > line_start {
                idx = resume - line_start;
                resumed_from_multiline = true;
            }
            string_resume_at = None;
        }

        if !resumed_from_multiline {
            for (i, ch) in line.char_indices() {
                match ch {
                    ' ' => {
                        col += 1;
                        idx = i + ch.len_utf8();
                    }
                    '\t' => {
                        saw_tab = true;
                        col += 1;
                        idx = i + ch.len_utf8();
                    }
                    _ => {
                        idx = i;
                        break;
                    }
                }
            }
        }

        let rest = if idx <= line.len() { &line[idx..] } else { "" };
        let rest_trim = rest.trim_start();
        if rest_trim.is_empty() {
            if resumed_from_multiline && has_newline && nesting == 0 {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    span: Span::new(line_end, line_end),
                });
            }
            continue;
        }
        let postfix_continuation = is_postfix_continuation_line(rest_trim);

        if saw_tab && !resumed_from_multiline {
            diags.error(
                Span::new(line_start, line_start + line.len()),
                "tabs are not allowed for indentation",
            );
        }

        let is_comment = rest_trim.starts_with('#');
        let is_doc_comment = rest_trim.starts_with("##");

        if is_comment && !is_doc_comment {
            if resumed_from_multiline && has_newline && nesting == 0 {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    span: Span::new(line_end, line_end),
                });
            }
            continue;
        }

        if indent_active && !postfix_continuation && !resumed_from_multiline {
            let current = *indent_stack.last().unwrap();
            if col > current {
                indent_stack.push(col);
                tokens.push(Token {
                    kind: TokenKind::Indent,
                    span: Span::new(line_start + col, line_start + col),
                });
            } else if col < current {
                while let Some(&top) = indent_stack.last() {
                    if col < top {
                        indent_stack.pop();
                        tokens.push(Token {
                            kind: TokenKind::Dedent,
                            span: Span::new(line_start + col, line_start + col),
                        });
                    } else {
                        break;
                    }
                }
                let top = *indent_stack.last().unwrap_or(&0);
                if col != top {
                    diags.error(
                        Span::new(line_start + col, line_start + col),
                        "inconsistent indentation",
                    );
                }
            }
        }

        if is_doc_comment {
            let comment_offset = line.len().saturating_sub(rest_trim.len());
            let content = rest_trim.trim_start_matches("##").trim_start().to_string();
            tokens.push(Token {
                kind: TokenKind::DocComment(content),
                span: Span::new(line_start + comment_offset, line_start + line.len()),
            });
            if nesting == 0 {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    span: Span::new(line_start + line.len(), line_start + line.len()),
                });
            }
            continue;
        }

        let mut i = idx;
        let mut line_ends_inside_multiline_string = false;
        while i < line.len() {
            let ch = line[i..].chars().next().unwrap();
            if ch.is_whitespace() {
                i += ch.len_utf8();
                continue;
            }
            if ch == '#' {
                break;
            }

            let start = line_start + i;

            if is_ident_start(ch) {
                let mut j = i + ch.len_utf8();
                while j < line.len() {
                    let c = line[j..].chars().next().unwrap();
                    if is_ident_continue(c) {
                        j += c.len_utf8();
                    } else {
                        break;
                    }
                }
                let text = &line[i..j];
                let kind = if let Some(kw) = Keyword::from_str(text) {
                    TokenKind::Keyword(kw)
                } else if text == "true" {
                    TokenKind::Bool(true)
                } else if text == "false" {
                    TokenKind::Bool(false)
                } else if text == "null" {
                    TokenKind::Null
                } else {
                    TokenKind::Ident(text.to_string())
                };
                tokens.push(Token {
                    kind,
                    span: Span::new(start, line_start + j),
                });
                i = j;
                continue;
            }

            if ch.is_ascii_digit() {
                let mut j = i + ch.len_utf8();
                while j < line.len() {
                    let c = line[j..].chars().next().unwrap();
                    if c.is_ascii_digit() {
                        j += c.len_utf8();
                    } else {
                        break;
                    }
                }
                if j < line.len() && line[j..].starts_with("..") {
                    let int_text = &line[i..j];
                    let value = int_text.parse::<i64>().unwrap_or(0);
                    tokens.push(Token {
                        kind: TokenKind::Int(value),
                        span: Span::new(start, line_start + j),
                    });
                    i = j;
                    continue;
                }
                if j < line.len() && line[j..].starts_with('.') {
                    let mut k = j + 1;
                    let mut saw_digit = false;
                    while k < line.len() {
                        let c = line[k..].chars().next().unwrap();
                        if c.is_ascii_digit() {
                            saw_digit = true;
                            k += c.len_utf8();
                        } else {
                            break;
                        }
                    }
                    if saw_digit {
                        let float_text = &line[i..k];
                        let value = float_text.parse::<f64>().unwrap_or(0.0);
                        tokens.push(Token {
                            kind: TokenKind::Float(value),
                            span: Span::new(start, line_start + k),
                        });
                        i = k;
                        continue;
                    }
                }
                let int_text = &line[i..j];
                let value = int_text.parse::<i64>().unwrap_or(0);
                tokens.push(Token {
                    kind: TokenKind::Int(value),
                    span: Span::new(start, line_start + j),
                });
                i = j;
                continue;
            }

            if ch == '"' {
                let (kind, end) = lex_string_literal(src, start, diags);
                tokens.push(Token {
                    kind,
                    span: Span::new(start, end),
                });
                if end > line_end {
                    string_resume_at = Some(end);
                    line_ends_inside_multiline_string = true;
                    break;
                }
                i = end - line_start;
                continue;
            }

            if let Some((punct, width)) = match_punct(&line[i..]) {
                let end = line_start + i + width;
                tokens.push(Token {
                    kind: TokenKind::Punct(punct),
                    span: Span::new(start, end),
                });
                match punct {
                    Punct::LParen | Punct::LBracket | Punct::LBrace => nesting += 1,
                    Punct::RParen | Punct::RBracket | Punct::RBrace => {
                        if nesting > 0 {
                            nesting -= 1;
                        } else {
                            diags.error(Span::new(start, end), "unmatched closing delimiter");
                        }
                    }
                    _ => {}
                }
                i += width;
                continue;
            }

            diags.error(
                Span::new(start, start + ch.len_utf8()),
                "unexpected character",
            );
            i += ch.len_utf8();
        }

        if nesting == 0 && !line_ends_inside_multiline_string {
            tokens.push(Token {
                kind: TokenKind::Newline,
                span: Span::new(line_end, line_end),
            });
        }
    }

    if nesting > 0 {
        diags.error(Span::new(offset, offset), "unclosed delimiter");
    }

    while indent_stack.len() > 1 {
        indent_stack.pop();
        tokens.push(Token {
            kind: TokenKind::Dedent,
            span: Span::new(offset, offset),
        });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(offset, offset),
    });

    tokens
}

fn lex_string_literal(src: &str, start: usize, diags: &mut Diagnostics) -> (TokenKind, usize) {
    let multiline = src[start..].starts_with("\"\"\"");
    let mut j = if multiline { start + 3 } else { start + 1 };
    let mut out = String::new();
    let mut segments = Vec::new();
    let mut has_interp = false;
    let mut terminated = false;

    while j < src.len() {
        if multiline && src[j..].starts_with("\"\"\"") {
            terminated = true;
            j += 3;
            break;
        }

        let c = src[j..].chars().next().unwrap();
        if !multiline && c == '\n' {
            break;
        }
        if !multiline && c == '"' {
            terminated = true;
            j += 1;
            break;
        }
        if c == '\\' {
            j += 1;
            if j >= src.len() {
                break;
            }
            let esc = src[j..].chars().next().unwrap();
            match esc {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                _ => out.push(esc),
            }
            j += esc.len_utf8();
            continue;
        }
        if c == '$' {
            let next_idx = j + c.len_utf8();
            if next_idx < src.len() {
                let next = src[next_idx..].chars().next().unwrap();
                if next == '{' {
                    has_interp = true;
                    if !out.is_empty() {
                        segments.push(InterpSegment::Text(out));
                        out = String::new();
                    }
                    let expr_start = next_idx + next.len_utf8();
                    let mut k = expr_start;
                    let mut depth = 1;
                    let mut in_string = false;
                    let mut escape = false;
                    while k < src.len() {
                        let ch = src[k..].chars().next().unwrap();
                        if in_string {
                            if escape {
                                escape = false;
                            } else if ch == '\\' {
                                escape = true;
                            } else if ch == '"' {
                                in_string = false;
                            }
                        } else if ch == '"' {
                            in_string = true;
                        } else if ch == '{' {
                            depth += 1;
                        } else if ch == '}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        k += ch.len_utf8();
                    }
                    if depth != 0 {
                        diags.error(Span::new(start, k), "unterminated interpolation");
                        j = k;
                        break;
                    }
                    let expr_src = unescape_fragment(&src[expr_start..k]);
                    segments.push(InterpSegment::Expr {
                        src: expr_src,
                        offset: expr_start,
                    });
                    j = k + 1;
                    continue;
                }
            }
        }
        out.push(c);
        j += c.len_utf8();
    }

    if !terminated {
        diags.error(Span::new(start, j), "unterminated string literal");
    }
    if has_interp {
        if !out.is_empty() {
            segments.push(InterpSegment::Text(out));
        }
        (TokenKind::InterpString(segments), j)
    } else {
        (TokenKind::String(out), j)
    }
}

fn unescape_fragment(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn match_punct(s: &str) -> Option<(Punct, usize)> {
    if s.starts_with("??") {
        return Some((Punct::QuestionQuestion, 2));
    }
    if s.starts_with("?!") {
        return Some((Punct::QuestionBang, 2));
    }
    if s.starts_with("->") {
        return Some((Punct::Arrow, 2));
    }
    if s.starts_with("=>") {
        return Some((Punct::FatArrow, 2));
    }
    if s.starts_with("==") {
        return Some((Punct::EqEq, 2));
    }
    if s.starts_with("!=") {
        return Some((Punct::NotEq, 2));
    }
    if s.starts_with("<=") {
        return Some((Punct::LtEq, 2));
    }
    if s.starts_with(">=") {
        return Some((Punct::GtEq, 2));
    }
    if s.starts_with("..") {
        return Some((Punct::DotDot, 2));
    }
    let ch = s.chars().next()?;
    let punct = match ch {
        '(' => Punct::LParen,
        ')' => Punct::RParen,
        '[' => Punct::LBracket,
        ']' => Punct::RBracket,
        '{' => Punct::LBrace,
        '}' => Punct::RBrace,
        ',' => Punct::Comma,
        ':' => Punct::Colon,
        '.' => Punct::Dot,
        '=' => Punct::Assign,
        '<' => Punct::Lt,
        '>' => Punct::Gt,
        '+' => Punct::Plus,
        '-' => Punct::Minus,
        '*' => Punct::Star,
        '/' => Punct::Slash,
        '%' => Punct::Percent,
        '?' => Punct::Question,
        '!' => Punct::Bang,
        _ => return None,
    };
    Some((punct, ch.len_utf8()))
}

fn is_postfix_continuation_line(rest_trim: &str) -> bool {
    rest_trim.starts_with(".")
        || rest_trim.starts_with("?.")
        || rest_trim.starts_with("?[")
        || rest_trim.starts_with("?!")
}
