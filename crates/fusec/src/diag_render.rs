use std::fs;
use std::path::Path;

use fuse_rt::json;

use crate::diag::{Diag, Level};

pub struct TextDiagnosticStyle {
    pub error_label: String,
    pub warning_label: String,
    pub caret: String,
    pub include_fallback_path: bool,
}

pub fn emit_diag_text(diag: &Diag, fallback: Option<(&Path, &str)>, style: &TextDiagnosticStyle) {
    let level = match diag.level {
        Level::Error => &style.error_label,
        Level::Warning => &style.warning_label,
    };
    if let Some(path) = &diag.path {
        if let Ok(src) = fs::read_to_string(path) {
            let (line, col, line_text) = line_info(&src, diag.span.start);
            eprintln!(
                "{level}: {} ({}:{}:{})",
                diag.message,
                path.display(),
                line,
                col
            );
            eprintln!("  {line_text}");
            eprintln!("  {}{}", " ".repeat(col.saturating_sub(1)), style.caret);
            return;
        }
        eprintln!("{level}: {} ({})", diag.message, path.display());
        return;
    }
    if let Some((path, src)) = fallback {
        let (line, col, line_text) = line_info(src, diag.span.start);
        if style.include_fallback_path {
            eprintln!(
                "{level}: {} ({}:{}:{})",
                diag.message,
                path.display(),
                line,
                col
            );
        } else {
            eprintln!("{level}: {} ({}:{})", diag.message, line, col);
        }
        eprintln!("  {line_text}");
        eprintln!("  {}{}", " ".repeat(col.saturating_sub(1)), style.caret);
        return;
    }
    eprintln!(
        "{level}: {} ({}..{})",
        diag.message, diag.span.start, diag.span.end
    );
}

pub fn diagnostic_json_value(diag: &Diag, fallback: Option<(&Path, &str)>) -> json::JsonValue {
    let mut object = std::collections::BTreeMap::new();
    object.insert(
        "kind".to_string(),
        json::JsonValue::String("diagnostic".to_string()),
    );
    object.insert(
        "level".to_string(),
        json::JsonValue::String(match diag.level {
            Level::Error => "error".to_string(),
            Level::Warning => "warning".to_string(),
        }),
    );
    object.insert(
        "message".to_string(),
        json::JsonValue::String(diag.message.clone()),
    );
    object.insert(
        "span_start".to_string(),
        json::JsonValue::Number(diag.span.start as f64),
    );
    object.insert(
        "span_end".to_string(),
        json::JsonValue::Number(diag.span.end as f64),
    );

    if let Some(path) = &diag.path {
        object.insert(
            "path".to_string(),
            json::JsonValue::String(path.display().to_string()),
        );
        if let Ok(src) = fs::read_to_string(path) {
            let (line, col, _) = line_info(&src, diag.span.start);
            object.insert("line".to_string(), json::JsonValue::Number(line as f64));
            object.insert("column".to_string(), json::JsonValue::Number(col as f64));
        }
        return json::JsonValue::Object(object);
    }

    if let Some((path, src)) = fallback {
        let (line, col, _) = line_info(src, diag.span.start);
        object.insert(
            "path".to_string(),
            json::JsonValue::String(path.display().to_string()),
        );
        object.insert("line".to_string(), json::JsonValue::Number(line as f64));
        object.insert("column".to_string(), json::JsonValue::Number(col as f64));
    }

    json::JsonValue::Object(object)
}

pub fn line_info(src: &str, offset: usize) -> (usize, usize, &str) {
    let offset = offset.min(src.len());
    let mut line = 1usize;
    let mut line_start = 0usize;
    for (idx, byte) in src.bytes().enumerate() {
        if idx >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    let line_end = src[line_start..]
        .find('\n')
        .map(|rel| line_start + rel)
        .unwrap_or(src.len());
    let col = offset.saturating_sub(line_start) + 1;
    let line_text = &src[line_start..line_end];
    (line, col, line_text)
}
