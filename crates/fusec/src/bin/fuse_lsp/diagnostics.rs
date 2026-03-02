use std::collections::BTreeMap;
use std::io::{self, Write};

use fuse_rt::json::JsonValue;
use fusec::diag::{Diag, Level};
use fusec::parse_source;
use fusec::sema;

use super::super::{
    LspState, build_focus_workspace_snapshot, build_workspace_snapshot_cached, json_notification,
    line_offsets, offset_to_line_col, range_json, uri_to_path, write_message,
};

pub(crate) fn publish_diagnostics(
    out: &mut impl Write,
    state: &mut LspState,
    uri: &str,
    text: &str,
) -> io::Result<()> {
    let diags = workspace_diags_for_uri(state, uri).unwrap_or_else(|| {
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

pub(crate) fn publish_empty_diagnostics(out: &mut impl Write, uri: &str) -> io::Result<()> {
    let params = diagnostics_params(uri, Vec::new());
    let notification = json_notification("textDocument/publishDiagnostics", params);
    write_message(out, &notification)
}

fn workspace_diags_for_uri(state: &mut LspState, uri: &str) -> Option<Vec<Diag>> {
    let focus_path = uri_to_path(uri)?;
    let focus_key = focus_path
        .canonicalize()
        .unwrap_or_else(|_| focus_path.clone());
    let snapshot = build_workspace_snapshot_cached(state, uri)?;
    if let Some(module_id) = snapshot.module_ids_by_path.get(&focus_key).copied() {
        let (_, sema_diags) = sema::analyze_module(&snapshot.registry, module_id);
        let mut diags = Vec::new();
        for diag in &snapshot.loader_diags {
            if diag.path.is_none() {
                diags.push(diag.clone());
                continue;
            }
            if let Some(path) = diag.path.as_ref() {
                let key = path.canonicalize().unwrap_or_else(|_| path.clone());
                if key == focus_key {
                    diags.push(diag.clone());
                }
            }
        }
        diags.extend(sema_diags);
        return Some(diags);
    }

    let focus_snapshot = build_focus_workspace_snapshot(state, uri)?;
    let module_id = *focus_snapshot.module_ids_by_path.get(&focus_key)?;
    let (_, sema_diags) = sema::analyze_module(&focus_snapshot.registry, module_id);
    let mut diags = Vec::new();
    for diag in &focus_snapshot.loader_diags {
        if diag.path.is_none() {
            diags.push(diag.clone());
            continue;
        }
        if let Some(path) = diag.path.as_ref() {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if key == focus_key {
                diags.push(diag.clone());
            }
        }
    }
    diags.extend(sema_diags);
    Some(diags)
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
