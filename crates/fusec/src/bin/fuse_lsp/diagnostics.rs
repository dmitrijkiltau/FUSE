use std::collections::BTreeMap;
use std::io::{self, Write};

use fuse_rt::json::JsonValue;
use fusec::diag::{Diag, Level};
use fusec::parse_source;
use fusec::sema;

use super::super::{
    LspState, build_progressive_snapshot_cached, build_workspace_snapshot_cached,
    json_notification, line_offsets, offset_to_line_col, range_json, uri_to_path,
    workspace_index_key, write_message,
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

    // Use the full workspace snapshot when it is already warm for this revision
    // (no extra build cost), or when the focused file is the workspace entry
    // (the file that anchors the full workspace build anyway).
    // For any other file with a cold cache, go straight to the progressive path
    // so a single-file open in a large workspace does not block on a full build.
    let full_cache_warm = state
        .workspace_cache
        .as_ref()
        .is_some_and(|c| c.docs_revision == state.docs_revision);
    let force_full = std::mem::take(&mut state.workspace_rebuild_pending);
    let focus_is_entry = !full_cache_warm
        && !force_full
        && workspace_index_key(state, uri)
            .map(|k| std::path::PathBuf::from(&k) == focus_key)
            .unwrap_or(false);

    if full_cache_warm || focus_is_entry || force_full {
        if let Some(snapshot) = build_workspace_snapshot_cached(state, uri) {
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
            // File not in full workspace registry — fall through to progressive.
        }
    }

    // Progressive path: load only the focus file and its transitive imports.
    // Used when the cache is cold and the file is not the workspace entry, so
    // the first diagnostics response is not blocked on a full workspace build.
    let snap = build_progressive_snapshot_cached(state, uri)?;
    let module_id = *snap.module_ids_by_path.get(&focus_key)?;
    let (_, sema_diags) = sema::analyze_module(&snap.registry, module_id);
    let mut diags = Vec::new();
    for diag in &snap.loader_diags {
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
            if let Some(code) = &diag.code {
                out.insert("code".to_string(), JsonValue::String(code.clone()));
            }
            out.insert("source".to_string(), JsonValue::String("fusec".to_string()));
            JsonValue::Object(out)
        })
        .collect()
}
