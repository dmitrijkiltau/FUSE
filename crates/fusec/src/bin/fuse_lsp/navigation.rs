use std::collections::{BTreeMap, HashSet};

use fuse_rt::json::JsonValue;

use super::super::{
    LspState, WorkspaceDef, WorkspaceIndex, build_workspace_index_cached,
    extract_include_declaration, extract_position, is_callable_def_kind, location_json,
    range_json, span_range_json,
};

pub(crate) fn handle_definition(
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
    if let Some(target_uri) = index.module_ref_target_at(&uri, line, character) {
        return JsonValue::Array(vec![zero_location_json(target_uri)]);
    }
    if let Some(target_uri) = index.import_path_target_at(&uri, line, character) {
        return JsonValue::Array(vec![zero_location_json(target_uri)]);
    }
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    let Some(def_text) = index.file_text(&def.uri) else {
        return JsonValue::Null;
    };
    let location = location_json(&def.uri, def_text, def.def.span);
    JsonValue::Array(vec![location])
}

pub(crate) fn handle_hover(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
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
    let mut value = format!(
        "**{}** `{}`\n\n```fuse\n{}\n```",
        def.def.kind.hover_label(),
        def.def.name,
        def.def.detail.trim()
    );
    if let Some(doc) = &def.def.doc {
        if !doc.trim().is_empty() {
            value.push_str("\n\n");
            value.push_str(doc.trim());
        }
    }
    let mut contents = BTreeMap::new();
    contents.insert(
        "kind".to_string(),
        JsonValue::String("markdown".to_string()),
    );
    contents.insert("value".to_string(), JsonValue::String(value));
    let mut out = BTreeMap::new();
    out.insert("contents".to_string(), JsonValue::Object(contents));
    if let Some(text) = index.file_text(&def.uri) {
        out.insert("range".to_string(), span_range_json(text, def.def.span));
    }
    JsonValue::Object(out)
}

pub(crate) fn handle_references(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let include_declaration = extract_include_declaration(obj);
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    JsonValue::Array(index.reference_locations(def.id, include_declaration))
}

pub(crate) fn handle_prepare_call_hierarchy(
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
    if !is_callable_def_kind(def.def.kind) {
        return JsonValue::Null;
    }
    let Some(item) = call_hierarchy_item_json(index, &def) else {
        return JsonValue::Null;
    };
    JsonValue::Array(vec![item])
}

pub(crate) fn handle_call_hierarchy_incoming(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(index) = build_workspace_index_for_call_hierarchy(state, obj) else {
        return JsonValue::Null;
    };
    let Some(def_id) = call_hierarchy_target_def_id(index, obj) else {
        return JsonValue::Null;
    };
    let mut result = Vec::new();
    for (from_id, sites) in index.incoming_calls(def_id) {
        let Some(from_def) = index.def_for_target(from_id) else {
            continue;
        };
        let Some(from_item) = call_hierarchy_item_json(index, &from_def) else {
            continue;
        };
        let mut ranges = Vec::new();
        let mut seen = HashSet::new();
        for site in sites {
            if !seen.insert((site.span.start, site.span.end)) {
                continue;
            }
            if let Some(range) = index.span_range_json(&site.uri, site.span) {
                ranges.push(range);
            }
        }
        let mut item = BTreeMap::new();
        item.insert("from".to_string(), from_item);
        item.insert("fromRanges".to_string(), JsonValue::Array(ranges));
        result.push(JsonValue::Object(item));
    }
    JsonValue::Array(result)
}

pub(crate) fn handle_call_hierarchy_outgoing(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(index) = build_workspace_index_for_call_hierarchy(state, obj) else {
        return JsonValue::Null;
    };
    let Some(def_id) = call_hierarchy_target_def_id(index, obj) else {
        return JsonValue::Null;
    };
    let mut result = Vec::new();
    for (to_id, sites) in index.outgoing_calls(def_id) {
        let Some(to_def) = index.def_for_target(to_id) else {
            continue;
        };
        let Some(to_item) = call_hierarchy_item_json(index, &to_def) else {
            continue;
        };
        let mut ranges = Vec::new();
        let mut seen = HashSet::new();
        for site in sites {
            if !seen.insert((site.span.start, site.span.end)) {
                continue;
            }
            if let Some(range) = index.span_range_json(&site.uri, site.span) {
                ranges.push(range);
            }
        }
        let mut item = BTreeMap::new();
        item.insert("to".to_string(), to_item);
        item.insert("fromRanges".to_string(), JsonValue::Array(ranges));
        result.push(JsonValue::Object(item));
    }
    JsonValue::Array(result)
}

fn build_workspace_index_for_call_hierarchy<'a>(
    state: &'a mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> Option<&'a WorkspaceIndex> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let item = params.get("item")?;
    let JsonValue::Object(item) = item else {
        return None;
    };
    let uri = match item.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    build_workspace_index_cached(state, &uri)
}

fn call_hierarchy_target_def_id(
    index: &WorkspaceIndex,
    obj: &BTreeMap<String, JsonValue>,
) -> Option<usize> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let item = params.get("item")?;
    let JsonValue::Object(item) = item else {
        return None;
    };
    if let Some(def_id) = item.get("data").and_then(|value| match value {
        JsonValue::Number(num) if *num >= 0.0 => Some(*num as usize),
        _ => None,
    }) {
        return Some(def_id);
    }
    let uri = match item.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    let selection_range = item.get("selectionRange").or_else(|| item.get("range"))?;
    let JsonValue::Object(selection_range) = selection_range else {
        return None;
    };
    let start = selection_range.get("start")?;
    let JsonValue::Object(start) = start else {
        return None;
    };
    let line = match start.get("line") {
        Some(JsonValue::Number(line)) => *line as usize,
        _ => return None,
    };
    let character = match start.get("character") {
        Some(JsonValue::Number(character)) => *character as usize,
        _ => return None,
    };
    let def = index.definition_at(&uri, line, character)?;
    Some(def.id)
}

fn call_hierarchy_item_json(index: &WorkspaceIndex, def: &WorkspaceDef) -> Option<JsonValue> {
    let text = index.file_text(&def.uri)?;
    let range = span_range_json(text, def.def.span);
    let mut out = BTreeMap::new();
    out.insert("name".to_string(), JsonValue::String(def.def.name.clone()));
    out.insert(
        "kind".to_string(),
        JsonValue::Number(def.def.kind.lsp_kind() as f64),
    );
    out.insert("uri".to_string(), JsonValue::String(def.uri.clone()));
    out.insert("range".to_string(), range.clone());
    out.insert("selectionRange".to_string(), range);
    out.insert("data".to_string(), JsonValue::Number(def.id as f64));
    if !def.def.detail.is_empty() {
        out.insert(
            "detail".to_string(),
            JsonValue::String(def.def.detail.clone()),
        );
    }
    Some(JsonValue::Object(out))
}

fn zero_location_json(uri: &str) -> JsonValue {
    let mut out = BTreeMap::new();
    out.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    out.insert("range".to_string(), range_json(0, 0, 0, 0));
    JsonValue::Object(out)
}
